//! Save states and screenshots: an emulator's state file with the thumbnail beside it, and an image
//! on its own.
//!
//! A state loads only into the core that took it, so the one thing a converter must get right is
//! which core that was. mGBA and DuckStation name themselves. RetroArch's file names nothing, so
//! the core comes from `--core`, or from the folder RetroArch files the state in when it sorts
//! states by core, which is named for the core.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use one_saves::{Bundle, Name, Shape};
use one_saves_convert::screenshot::{self, ScreenshotOptions};
use one_saves_convert::state::{self, Emulator, StateFile, StateOptions};

use crate::{Convert, Extract, Fallible, Identified, slug_flag, write_out};

/// What a file is, when it is one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// An emulator's save state.
    State(Emulator),
    /// A PNG or a JPEG on its own.
    Screenshot,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::State(emulator) => emulator.label(),
            Kind::Screenshot => "screenshot",
        }
    }
}

/// Whether a file is a state or an image, by its bytes and then its name.
///
/// Asked before any save format, since a DuckStation state is a `.sav` and an mGBA one a PNG.
pub(crate) fn kind_of(input: &Path, bytes: &[u8]) -> Option<Kind> {
    let name = input.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(emulator) = state::detect(bytes, &name) {
        return Some(Kind::State(emulator));
    }
    screenshot::image_type(bytes).map(|_| Kind::Screenshot)
}

/// `path` with `suffix` after its whole name, rather than in place of its extension: `X.state3`
/// becomes `X.state3.1saves`, since the slot is in what an extension swap would drop.
fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// When a file was last written, as whole epoch seconds: when the emulator took the state, where
/// it kept no record of its own.
fn modified(path: &Path) -> Option<i64> {
    let since = std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_secs()).ok()
}

/// The core a folder is named for, as RetroArch names the folders it sorts states into by core.
fn core_from_folder(input: &Path) -> Option<&'static str> {
    let folder = input.parent()?.file_name()?.to_str()?;
    one_saves_registry::core_by_any_name(folder)
        .filter(|core| core.kind == one_saves_registry::CoreKind::Libretro)
        .map(|core| core.slug)
}

pub(crate) fn convert(args: &Convert, kind: Kind, bytes: &[u8]) -> Fallible {
    let profile = args.from.as_deref().and_then(one_saves_convert::profile);
    #[cfg(feature = "rom")]
    let Identified { game, system: rom_system, .. } = crate::identify(args)?;
    #[cfg(not(feature = "rom"))]
    let Identified { game, system: rom_system, .. } = Identified::default();

    let mut source = crate::source_of(args, profile.as_ref())?;
    if kind == Kind::State(Emulator::RetroArch)
        && source.as_ref().is_none_or(|source| source.core.is_none())
        && let Some(core) = core_from_folder(&args.input)
    {
        let found = source.get_or_insert_default();
        found.core = Some(Name::parse(core)?);
        println!("{}: took the core {core} from the folder RetroArch filed it in", args.input.display());
    }
    if kind == Kind::State(Emulator::RetroArch) && source.as_ref().is_none_or(|source| source.core.is_none())
    {
        return Err("a RetroArch state loads only into the core that took it, and the file does not say \
                    which; pass --core, as RetroArch names it"
            .into());
    }
    let system = crate::system_of(args, rom_system, profile.as_ref(), source.as_ref())
        .map(|system| slug_flag("--system", &system))
        .transpose()?;
    let name = args.input.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let taken_at = modified(&args.input);

    let bundle = match kind {
        Kind::State(emulator) => {
            // RetroArch keeps its thumbnail beside the state, named for it.
            let beside = appended(&args.input, ".png");
            let thumbnail = (emulator == Emulator::RetroArch).then(|| std::fs::read(&beside).ok()).flatten();
            if thumbnail.is_some() {
                println!("{}: took the screenshot beside it", beside.display());
            }
            let options = StateOptions {
                source,
                system,
                game,
                description: args.description.clone(),
                taken_at,
                ..StateOptions::default()
            };
            let file = StateFile { bytes, name: &name, screenshot: thumbnail.as_deref() };
            state::read(emulator, &file, &options)?
        }
        Kind::Screenshot => {
            let options = ScreenshotOptions {
                source,
                system,
                game,
                description: args.description.clone(),
                taken_at,
                ..ScreenshotOptions::default()
            };
            screenshot::read(bytes, &options)?
        }
    };

    let output =
        args.output.clone().unwrap_or_else(|| appended(&args.input, &format!(".{}", one_saves::EXTENSION)));
    let encoded = bundle.to_vec()?;
    write_out(&output, &encoded, args.force)?;

    let mut what = vec![kind.label().to_owned()];
    if let Some(slot) = bundle.parts.first().and_then(|part| part.slot) {
        what.push(format!("slot {slot}"));
    }
    if let Some(origin) = &bundle.header.origin {
        what.push(format!("origin {origin}"));
    }
    if let Some(core) = bundle.header.source.as_ref().and_then(|source| source.core.as_ref())
        && matches!(kind, Kind::State(_))
    {
        what.push(format!("loads into {core}"));
    }
    println!(
        "{} -> {} ({}, {} part(s), {} bytes)",
        args.input.display(),
        output.display(),
        what.join(", "),
        bundle.parts.len(),
        encoded.len()
    );
    Ok(())
}

/// What a rebuilt state file is named after: the game's ROM, else the bundle's own name with the
/// slot taken off it.
fn stem_of(bundle: &Bundle, input: &Path) -> String {
    if let Some(rom) = bundle.header.game.as_ref().and_then(|game| game.rom_filename.as_deref()) {
        return Path::new(rom)
            .file_stem()
            .map_or_else(|| rom.to_owned(), |stem| stem.to_string_lossy().into_owned());
    }
    let name = input.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let name = name.strip_suffix(&format!(".{}", one_saves::EXTENSION)).unwrap_or(&name);
    // `X.state3`, `X.state.auto` and `X.ss1` were named for the slot as well as the game.
    for marker in [".state", ".ss"] {
        if let Some((stem, tail)) = name.rsplit_once(marker)
            && (tail.is_empty() || tail == ".auto" || tail.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return stem.to_owned();
        }
    }
    Path::new(name).file_stem().map_or_else(|| name.to_owned(), |stem| stem.to_string_lossy().into_owned())
}

pub(crate) fn extract(args: &Extract, bundle: &Bundle) -> Fallible {
    if bundle.header.shape == Shape::Screenshot {
        let (image, content_type) = screenshot::write(bundle)?;
        let extension = screenshot::extension(&content_type).unwrap_or("bin");
        let output = args.output.clone().unwrap_or_else(|| args.input.with_extension(extension));
        write_out(&output, &image, args.force)?;
        println!("{} -> {} ({content_type}, {} bytes)", args.input.display(), output.display(), image.len());
        return Ok(());
    }

    let emulator = match args.to.as_deref() {
        Some(name) => Emulator::from_app(name).ok_or_else(|| {
            format!("a state is rebuilt for retroarch, mgba or duckstation, and {name:?} is none of them")
        })?,
        None => state::emulator_of(bundle).ok_or(
            "this state was not taken by an emulator this build rebuilds files for; pass --to retroarch, \
             mgba or duckstation to say which file to write",
        )?,
    };
    let rebuilt = state::write(emulator, bundle, &stem_of(bundle, &args.input))?;
    // The name is built from what the bundle says, which is anyone's to write; the library keeps it
    // to one component, and this makes sure of it before joining it onto a directory.
    let mut components = Path::new(&rebuilt.name).components();
    if !matches!((components.next(), components.next()), (Some(std::path::Component::Normal(_)), None)) {
        return Err(format!(
            "refusing to write a state named {:?}, which is not a plain file name",
            rebuilt.name
        )
        .into());
    }

    // A directory takes the emulator's own name for the file, which is where its slot is; anything
    // else is the file's path as given.
    let output = match &args.output {
        Some(path) if path.is_dir() => path.join(&rebuilt.name),
        Some(path) => path.clone(),
        None => args.input.parent().unwrap_or(Path::new("")).join(&rebuilt.name),
    };
    write_out(&output, &rebuilt.bytes, args.force)?;
    let core = bundle.header.source.as_ref().and_then(|source| source.core.as_ref());
    let into = core.map_or_else(String::new, |core| format!(", for {core}"));
    println!(
        "{} -> {} ({}{into}, {} bytes)",
        args.input.display(),
        output.display(),
        emulator.label(),
        rebuilt.bytes.len()
    );
    if let Some(png) = &rebuilt.screenshot {
        let beside = appended(&output, ".png");
        write_out(&beside, png, args.force)?;
        println!("screenshot -> {} ({} bytes)", beside.display(), png.len());
    }
    if !rebuilt.left_out.is_empty() {
        let ids: Vec<String> = rebuilt.left_out.iter().map(ToString::to_string).collect();
        eprintln!(
            "note: {} keeps no place for part(s) {}; take them out with --part",
            emulator.label(),
            ids.join(", ")
        );
    }
    Ok(())
}
