//! The `1saves` command-line tool.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use one_saves::extension::{Civil, SHOWN_BYTES, SHOWN_CHARS};
use one_saves::{Bundle, Game, Name, Part, PartKind, Payload, Source};
use one_saves_convert::{Format, Profile, card, detect, raw};

/// Move retro saves and memory cards in and out of the Universal Saves Format.
#[derive(Parser)]
#[command(name = "1saves", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Wrap a save or memory card into a .1saves bundle.
    ///
    /// The input may be a .zip holding a card, or holding loose PlayStation saves, in which
    /// case a card is built to carry them.
    Convert(Box<Convert>),
    /// Write a bundle back out in the format it came from.
    Extract(Extract),
    /// Show what a bundle is and what it holds.
    Inspect(FileArg),
    /// Check a bundle's structure and every part's digest.
    Verify(FileArg),
    /// Print a bundle's content hash and file hash.
    Hash(FileArg),
    /// List the emulator profiles `--from` accepts.
    Profiles,
}

#[derive(Args)]
struct FileArg {
    /// The bundle to read.
    file: PathBuf,
}

#[derive(Args)]
struct Convert {
    /// The save or card to read.
    input: PathBuf,
    /// Where to write the bundle. Defaults to the input with a .1saves extension.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// The app, core or device the save came from, or the input format.
    #[arg(long, value_name = "NAME")]
    from: Option<String>,
    /// The system these bytes are a save for, as a registry slug.
    #[arg(long, value_name = "SLUG")]
    system: Option<String>,
    /// The socket they came out of, as a registry slug. Absent means `primary`.
    #[arg(long, value_name = "SLUG")]
    role: Option<String>,
    /// The version of the app `--from` names, recorded as `source.app_version`.
    #[arg(long, value_name = "VERSION")]
    app_version: Option<String>,
    /// The emulator core the app ran, for an app such as RetroArch that runs many.
    ///
    /// A listed core is written as its registry slug, so `mGBA` and `mgba` both write `mgba`. One
    /// the registry does not list has to be given as its owner's reverse-DNS name.
    #[arg(long, value_name = "NAME")]
    core: Option<String>,
    /// The version of the core, recorded as `source.core_version`.
    #[arg(long, value_name = "VERSION")]
    core_version: Option<String>,
    /// The operating system or firmware the app ran on, such as `muos` or `windows`.
    ///
    /// Name the most specific one: a handheld running KNULLI is `knulli`, not `linux`.
    #[arg(long, value_name = "NAME")]
    platform: Option<String>,
    /// The version of the platform, recorded as `source.platform_version`.
    #[arg(long, value_name = "VERSION")]
    platform_version: Option<String>,
    /// A human-readable note about this bundle.
    #[arg(long)]
    description: Option<String>,
    /// The game's ROM, read for its header fields and hashed for `rom_hashes`.
    #[cfg(feature = "rom")]
    #[arg(long, value_name = "FILE")]
    rom: Option<PathBuf>,
    /// A No-Intro, Redump or TOSEC DAT to resolve the ROM's digest into a canonical name.
    #[cfg(feature = "dat")]
    #[arg(long, value_name = "FILE")]
    dat: Option<PathBuf>,
    /// A sidecar clock file written beside the save, such as Gambatte's `.rtc`.
    ///
    /// Found automatically next to the input when it is named after it; pass this to point
    /// somewhere else, or to supply one that has been moved away from its save.
    #[arg(long, value_name = "FILE")]
    rtc_file: Option<PathBuf>,
    /// When the sidecar clock was written, in epoch seconds.
    ///
    /// A sidecar stores the instant the clock counts *from*, not what it reads, so turning one
    /// into the other needs to know when it was taken — and the file does not say. Without this
    /// the file's modification time is used, which is right only for a file an emulator wrote in
    /// place: copying, unzipping or checking it out of git all reset it.
    ///
    /// The origin round-trips either way; it is the reading that depends on getting this right.
    #[arg(long, value_name = "EPOCH")]
    rtc_captured_at: Option<i64>,
    /// Keep an appended real-time-clock footer inside the save rather than splitting it out.
    ///
    /// Splitting is the default because the footer carries a live timestamp, so leaving it inline
    /// gives the save a content hash that changes every time the clock does.
    #[arg(long)]
    keep_rtc_inline: bool,
    /// Overwrite the output if it exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct Extract {
    /// The bundle to read.
    input: PathBuf,
    /// Where to write the result.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// The format to write. Defaults to what the bundle says it is.
    #[arg(long, value_name = "NAME")]
    to: Option<String>,
    /// Extract one part's payload by id, rather than the whole thing.
    #[arg(long, value_name = "ID")]
    part: Option<u64>,
    /// Write the clock to a sidecar file, as Gambatte keeps it, instead of appending it.
    #[arg(long, value_name = "FILE")]
    rtc_file: Option<PathBuf>,
    /// Overwrite the output if it exists.
    #[arg(long)]
    force: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("1saves: {error}");
            // Errors nest — a bad part inside a nested bundle inside a card — and the chain is
            // where the actual fault usually is.
            let mut source = std::error::Error::source(&*error);
            while let Some(inner) = source {
                eprintln!("  caused by: {inner}");
                source = inner.source();
            }
            ExitCode::FAILURE
        }
    }
}

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn run(cli: Cli) -> Fallible {
    match cli.command {
        Command::Convert(args) => convert(*args),
        Command::Extract(args) => extract(args),
        Command::Inspect(args) => inspect(&args.file),
        Command::Verify(args) => verify(&args.file),
        Command::Hash(args) => hash(&args.file),
        Command::Profiles => {
            profiles();
            Ok(())
        }
    }
}

fn extension_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// Refuses to clobber an existing file unless told to.
fn write_out(path: &Path, bytes: &[u8], force: bool) -> Fallible {
    if path.exists() && !force {
        return Err(format!("{} already exists; pass --force to overwrite", path.display()).into());
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// The bytes to convert and the extension to detect them by, with any archive taken off first.
///
/// A card shared as an archive is read out of it rather than through a temporary directory. An
/// archive of loose saves comes back as a card built to carry them, which is a card that never
/// existed before now — so it is said out loud rather than passed off as what was read.
#[cfg(feature = "archive")]
fn unwrap_archive(
    input: &std::path::Path,
    bytes: Vec<u8>,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    if !one_saves_convert::archive::is_archive(&bytes) {
        return Ok((bytes, extension_of(input)));
    }
    let unpacked = one_saves_convert::archive::unpack(&bytes)?;
    if unpacked.assembled {
        eprintln!("{}: built a card from {}", input.display(), unpacked.source);
    }
    Ok((unpacked.bytes, unpacked.extension))
}

/// The clock kept beside a save, and the instant it was read.
///
/// A clock in its own file has to be found, since the save does not mention it. The file records
/// an origin rather than a reading, so an instant is what turns one into the other.
type Sidecar = (std::path::PathBuf, Vec<u8>, i64);

fn sidecar_clock(args: &Convert) -> Result<Option<Sidecar>, Box<dyn std::error::Error>> {
    let path = args.rtc_file.clone().or_else(|| {
        let beside = args.input.with_extension("rtc");
        beside.exists().then_some(beside)
    });
    let Some(path) = path else {
        if args.rtc_captured_at.is_some() {
            return Err("--rtc-captured-at was given, but there is no sidecar clock to apply it to".into());
        }
        return Ok(None);
    };

    let bytes = std::fs::read(&path)?;
    // Falling back to the modification time is a guess, and one that gets worse the further the
    // file has travelled: copying, unzipping and checking out all reset it. It is only the right
    // answer for a file the emulator wrote in place.
    let captured_at = if let Some(stated) = args.rtc_captured_at {
        stated
    } else {
        let mtime = std::fs::metadata(&path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("{} has a modification time before 1970: {e}", path.display()))?
            .as_secs();
        i64::try_from(mtime)?
    };
    Ok(Some((path, bytes, captured_at)))
}

fn convert(args: Convert) -> Fallible {
    #[cfg(feature = "archive")]
    let (bytes, extension) = unwrap_archive(&args.input, std::fs::read(&args.input)?)?;
    // Without `archive`, a file is only ever itself.
    #[cfg(not(feature = "archive"))]
    let (bytes, extension) = (std::fs::read(&args.input)?, extension_of(&args.input));

    // `--from` names either a producer (mgba, duckstation) or a format (ps1, raw). A producer
    // says who wrote the bytes; the format is still detected from the bytes themselves.
    let profile = args.from.as_deref().and_then(one_saves_convert::profile);
    let named_format = args.from.as_deref().and_then(Format::from_name);
    let format = match named_format {
        Some(format) => format,
        None => detect::detect(&bytes, &extension)?,
    };

    if format == Format::Bundle {
        return Err("this file is already a bundle".into());
    }

    #[cfg(feature = "rom")]
    let Identified { mut game, system: rom_system } = identify(&args)?;
    // Without `rom` there is no `--rom` to read, so there is nothing a ROM could have said.
    #[cfg(not(feature = "rom"))]
    let Identified { mut game, system: rom_system } = Identified::default();

    let sidecar = sidecar_clock(&args)?;
    let sidecar_path = sidecar.as_ref().map(|(path, _, _)| path.clone());
    let sidecar = sidecar.map(|(_, bytes, at)| (bytes, at));

    let source = source_of(&args, profile.as_ref())?;
    // A profile or a core that covers exactly one system settles `system` when the user did not say.
    let core_system = source
        .as_ref()
        .and_then(|source| source.core.as_ref())
        .and_then(|core| one_saves_registry::core(core.as_str()))
        .and_then(|core| match core.systems {
            [only] => Some(*only),
            _ => None,
        });
    let system = args
        .system
        .clone()
        .or_else(|| rom_system.map(ToOwned::to_owned))
        .or_else(|| profile.as_ref().and_then(Profile::only_system).map(ToOwned::to_owned))
        .or_else(|| core_system.map(ToOwned::to_owned));

    let mut bundle = if format == Format::Raw {
        let options = raw::RawOptions {
            system,
            role: args.role.clone(),
            source,
            game: game.clone(),
            description: args.description.clone(),
            split_rtc: !args.keep_rtc_inline,
            rtc_sidecar: sidecar,
            ..raw::RawOptions::default()
        };
        raw::wrap(&bytes, &options)?
    } else {
        let mut options = card::CardOptions { source, ..card::CardOptions::default() };
        if let Some(role) = &args.role {
            options.role = Some(
                one_saves::Slug::parse(role).map_err(|e| format!("--role {role:?} is not a slug: {e}"))?,
            );
        }
        card::read(format, &bytes, &options)?
    };
    if let Some(description) = args.description {
        bundle.header.description = Some(description);
    }
    // On a card, a header `game` would claim the whole card is that game, which is only true when
    // every save on it is. Naming one is the caller's doing, so it is honoured rather than guessed.
    if bundle.header.card.is_some()
        && let Some(game) = game.take()
    {
        bundle.header.game = Some(game);
    }

    let output = args.output.unwrap_or_else(|| args.input.with_extension(one_saves::EXTENSION));
    let encoded = bundle.to_vec()?;
    write_out(&output, &encoded, args.force)?;

    if let Some(path) = &sidecar_path {
        let how = if args.rtc_captured_at.is_some() {
            "the instant given".to_owned()
        } else {
            "its modification time (pass --rtc-captured-at if that is not when it was written)".to_owned()
        };
        println!("read the clock from {}, using {how}", path.display());
    } else if one_saves_convert::rtc::footer_of(&bundle).is_some() {
        println!("split a real-time-clock footer off the save, so its bytes hash the same over time");
    }

    // A backup RAM is a flat save to this build, but saying "flat cartridge save" about one would
    // be wrong twice over: the internal RAM is not a cartridge, and the cart is not a game. What
    // the bundle says settles it rather than the bytes, so a volume the caller filed under another
    // system with `--system` is described as what was written and not as what it looks like.
    let backup_ram = raw::is_segacd_bram(&bytes)
        && bundle.header.system.as_ref().is_some_and(|system| system.as_str() == "sega-cd");

    // A console with two sockets is the case an absent role gets wrong: it means `primary`, which
    // says this is the only place a save could have come from, and on a Sega CD it is not.
    if args.role.is_none() && backup_ram {
        eprintln!("note: a Sega CD carries internal backup RAM and a Backup RAM Cart at once; pass");
        eprintln!("      --role internal or --role ram-cart to say which socket this came out of");
    }

    let saves = bundle.parts.iter().filter(|p| p.kind == PartKind::Bundle).count();
    let what = if saves > 0 { format!("{saves} save(s)") } else { format!("{} part(s)", bundle.parts.len()) };
    println!(
        "{} -> {} ({}, {what}, {} bytes)",
        args.input.display(),
        output.display(),
        if backup_ram { "Sega CD backup RAM" } else { format.label() },
        encoded.len()
    );
    Ok(())
}

/// The `source` a bundle carries: what `--from` names, and what the other flags add to it.
///
/// Every layer is checked against the registry before it is written, since `source` sits in the
/// header and a misspelt name changes the content hash as surely as a different one.
fn source_of(
    args: &Convert,
    profile: Option<&Profile>,
) -> Result<Option<Source>, Box<dyn std::error::Error>> {
    let mut source = profile.map(Profile::source).unwrap_or_default();

    if let Some(text) = &args.core {
        let core =
            listed_name(text, "--core", |name| one_saves_registry::core_by_any_name(name).map(|c| c.slug))?;
        if let Some(named) = &source.core
            && named != &core
        {
            return Err(format!("--from names the core {named}, and --core says {core}").into());
        }
        // A listed app says what it runs, so a core it cannot have run is a slip rather than news.
        let app = source.app.as_ref().and_then(|app| one_saves_registry::app(app.as_str()));
        if let Some(app) = app
            && let Some(listed) = one_saves_registry::core(core.as_str())
            && !app.runs(listed)
        {
            return Err(format!(
                "{} does not run {}, so --core {text} cannot be right",
                app.name, listed.name
            )
            .into());
        }
        source.core = Some(core);
    }
    if let Some(text) = &args.platform {
        let platform = listed_name(text, "--platform", |name| {
            one_saves_registry::platform_by_any_name(name).map(|p| p.slug)
        })?;
        source.platform = Some(platform);
    }

    // A version says which build of something, so it needs the something. `--from mgba` names the
    // core rather than an app, which is the slip this catches most.
    for (flag, version, named, what) in [
        ("--app-version", &args.app_version, source.app.is_some(), "an app"),
        ("--core-version", &args.core_version, source.core.is_some(), "a core"),
        ("--platform-version", &args.platform_version, source.platform.is_some(), "a platform"),
    ] {
        if version.is_some() && !named {
            let hint = if flag == "--app-version" && source.core.is_some() {
                "; --from names a core here, so its version is --core-version"
            } else {
                ""
            };
            return Err(format!(
                "{flag} was given, but nothing names {what} for it to be the version of{hint}"
            )
            .into());
        }
    }
    source.app_version.clone_from(&args.app_version);
    source.core_version.clone_from(&args.core_version);
    source.platform_version.clone_from(&args.platform_version);

    Ok((!source.is_empty()).then_some(source))
}

/// The name to write for a core or platform someone typed.
///
/// A listed one is written as its registry slug, whatever it was called. One the registry does not
/// list may only be a reverse-DNS name, which is its owner's to mint, so a bare word that matches
/// nothing is refused rather than written as a slug nobody registered.
fn listed_name(
    text: &str,
    flag: &str,
    lookup: impl Fn(&str) -> Option<&'static str>,
) -> Result<Name, String> {
    if let Some(slug) = lookup(text) {
        return Name::parse(slug).map_err(|e| format!("the registry slug {slug:?} is malformed: {e}"));
    }
    match Name::parse(text) {
        Ok(name @ Name::ReverseDns(_)) => Ok(name),
        Ok(Name::Slug(_)) => Err(format!(
            "{flag} {text:?} is not one the registry lists; an unlisted one is written as its owner's \
             reverse-DNS name"
        )),
        Err(e) => Err(format!("{flag} {text:?} is neither listed nor a well-formed name: {e}")),
    }
}

/// What a ROM and a catalog were able to say about the save.
///
/// A build without the `rom` feature has neither flag to say it with, so both stay empty and the
/// bundle carries whatever `--system` and `--from` gave it.
#[derive(Default)]
struct Identified {
    /// The `game` map, when a ROM was given.
    game: Option<one_saves::Game>,
    /// The system the ROM's header names, as a registry slug.
    system: Option<&'static str>,
}

/// Works out what game a save belongs to, from a ROM's header and a catalog.
///
/// A ROM says what the save cannot: its header gives a title, a system and often a product code,
/// and its digests are what a catalog is keyed on.
#[cfg(feature = "rom")]
fn identify(args: &Convert) -> Result<Identified, Box<dyn std::error::Error>> {
    let Some(rom_path) = args.rom.as_deref() else {
        return Ok(Identified::default());
    };
    let bytes = std::fs::read(rom_path)?;
    let filename = rom_path.file_name().and_then(|n| n.to_str());
    // The header is the whole of it without `dat`; a catalog is the only thing that revises it.
    #[cfg_attr(not(feature = "dat"), allow(unused_mut))]
    let mut game = one_saves_convert::rom::game_from_rom(&bytes, filename);
    let system = one_saves_convert::rom::identify(&bytes).and_then(|info| info.system);

    #[cfg(feature = "dat")]
    if let Some(dat) = args.dat.as_deref() {
        let catalog = one_saves_convert::dat::Catalog::open(dat)?;
        if catalog.enrich(&mut game) {
            println!("identified as {:?}", game.title.as_deref().unwrap_or_default());
        } else {
            eprintln!("note: this ROM is not in {}", dat.display());
        }
    }
    Ok(Identified { game: Some(game), system })
}

fn extract(args: Extract) -> Fallible {
    let bytes = std::fs::read(&args.input)?;
    let bundle = Bundle::from_slice(&bytes)?;

    // A part named by id is a straight payload dump, whatever the bundle is.
    if let Some(id) = args.part {
        let payload = raw::unwrap_part(&bundle, id)?;
        let output = args.output.unwrap_or_else(|| args.input.with_extension(format!("part{id}.bin")));
        write_out(&output, &payload, args.force)?;
        println!("part {id} -> {} ({} bytes)", output.display(), payload.len());
        return Ok(());
    }

    // Otherwise the bundle says what it is: a card writes back as that card, and anything else
    // is a flat save.
    let card_format = bundle.header.card.as_ref().map(|c| c.format.as_str().to_owned());
    let target = match args.to.as_deref() {
        Some(name) => Format::from_name(name).ok_or_else(|| format!("unknown format {name:?}"))?,
        None => match card_format.as_deref() {
            // A slug nothing here knows is not a card this build can lay out, and rebuilding one
            // it cannot lay out would corrupt it. A slug it does know but was not compiled with
            // gets a different message, out of the writer below.
            Some(slug) => Format::from_card_format(slug)
                .ok_or_else(|| format!("this bundle is a {slug}, which this build cannot write"))?,
            None => Format::Raw,
        },
    };

    let payload = match target {
        Format::Raw => raw::unwrap(&bundle)?,
        Format::Bundle => return Err("extracting a bundle as a bundle is a copy".into()),
        // A PS2 dump's spare area is regenerated rather than stored, so writing one back is a
        // choice the caller makes rather than something the bundle records.
        card => card::write(card, &bundle)?,
    };
    // `Format::Raw` covers every flat save and names the commonest of them, so a backup RAM would
    // otherwise go back out as `.srm`. The bundle knows better.
    let is_backup_ram = bundle.header.system.as_ref().is_some_and(|s| s.as_str() == "sega-cd");
    let suffix = if target == Format::Raw && is_backup_ram { "brm" } else { target.extension() };

    // A clock kept in its own file is written back to one, and left off the save: the two forms
    // are alternatives, and a save carrying both would have its clock read twice over.
    let payload = match &args.rtc_file {
        Some(path) => {
            let sidecar = one_saves_convert::rtc::sidecar_of(&bundle)
                .ok_or("this bundle carries no clock to write to a sidecar")?;
            write_out(path, &sidecar, args.force)?;
            println!("clock -> {} ({} bytes)", path.display(), sidecar.len());
            raw::unwrap_save_only(&bundle)?
        }
        None => payload,
    };

    let output = args.output.unwrap_or_else(|| args.input.with_extension(suffix));
    write_out(&output, &payload, args.force)?;
    println!(
        "{} -> {} ({}, {} bytes)",
        args.input.display(),
        output.display(),
        if target == Format::Raw && is_backup_ram { "Sega CD backup RAM" } else { target.label() },
        payload.len()
    );
    Ok(())
}

fn inspect(path: &Path) -> Fallible {
    let bytes = std::fs::read(path)?;
    let bundle = Bundle::from_slice(&bytes)?;

    println!("{}", path.display());
    println!("  {} bytes, spec version {}", bytes.len(), one_saves::SPEC_VERSION);
    let mut inflatable = MAX_INFLATED;
    describe(&bundle, 2, 0, &mut inflatable);
    Ok(())
}

/// How wide the labels down the left of a listing are: `content type` and a space.
const LABEL_WIDTH: usize = 13;

/// How many bundles deep a listing goes beneath the file's own.
///
/// The shapes this version defines nest three deep at most — a collection holding a device holding
/// a card holding a save — and validation holds a bundle to that. It does not look inside a shape
/// it does not know or a compressed part, though, so a file built to nest without end is cut off
/// here rather than taking the stack with it. One level past the defined shapes leaves room for a
/// later version's.
const MAX_DEPTH: usize = 4;

/// The most one listing inflates, across every compressed `bundle` part it opens.
///
/// Inflating a part stops at what it claims, and the claim is the file's to make, so the claims
/// need a bound of their own. One for the whole listing rather than one a part, because a file can
/// hold any number of parts each claiming just under a per-part cap, and an inflated bundle can
/// hold thousands more. This is a dozen of the largest cards there are, 16 MiB GameCube ones: it
/// declines a bomb and no real collection.
const MAX_INFLATED: u64 = 192 << 20;

/// One line of a listing: a label, and a value lined up after it.
///
/// An empty label continues the field above, which is how a field's further facts sit beneath its
/// first line. Trimmed because a value can be empty: a game can hold nothing its first line names,
/// only a resolver, say, which is listed beneath it.
fn field(indent: usize, label: &str, value: &str) {
    println!("{}", format!("{:indent$}{label:LABEL_WIDTH$}{value}", "").trim_end());
}

/// Everything a bundle says about itself, then each of its parts beneath it.
///
/// A part that is itself a bundle is described the same way under its own line, so a card lists
/// what each save's own header says — its label and icon among it — and not only the card's
/// record of it. `depth` counts the bundles above this one, up to [`MAX_DEPTH`], and `inflatable`
/// is what is left of [`MAX_INFLATED`].
fn describe(bundle: &Bundle, indent: usize, depth: usize, inflatable: &mut u64) {
    let header = &bundle.header;

    // What the bundle *is*, which decides how the parts below it should be read. It comes from
    // the container crate rather than from a rule spelled out here, so this listing and any other
    // consumer classify the same file the same way.
    let shape = bundle.shape();
    if shape.is_known() {
        field(indent, "shape", shape.as_str());
    } else {
        field(indent, "shape", &format!("{shape} (not a shape spec {} defines)", one_saves::SPEC_VERSION));
    }
    if let Some(system) = &header.system {
        field(indent, "system", &system_name(system));
    }
    if let Some(card) = &header.card {
        field(indent, "card", &format!("{}, capacity {} bytes", card.format, card.capacity));
        if let Some(area) = &card.system_area {
            field(indent, "", &format!("system area {}", shown_bytes(area)));
        }
        for line in unknown_keys(&card.unknown) {
            field(indent, "", &format!("unknown {line}"));
        }
    }
    // Where the bytes came from goes ahead of which game they are thought to be: the one is a
    // record of what happened, the other a set of clues.
    if let Some(source) = &header.source {
        describe_source(indent, source);
    }
    if let Some(seconds) = header.created_at {
        field(indent, "created", &format!("{} UTC", Civil(seconds)));
    }
    match &header.game {
        Some(game) => describe_game(indent, game),
        None => field(indent, "game", "unidentified"),
    }
    if let Some(description) = &header.description {
        field(indent, "description", &printable(description));
    }
    for key in header.extensions.keys() {
        field(indent, "extension", &describe_extension(&header.extensions, key));
    }
    for line in unknown_keys(&header.unknown) {
        field(indent, "unknown", &line);
    }

    field(indent, "parts", &bundle.parts.len().to_string());
    for part in &bundle.parts {
        describe_part(part, indent + 2, depth, inflatable);
    }
}

/// A part's line, and beneath it everything else the part says, down to the bundle it holds if it
/// holds one. `depth` is the depth of the bundle the part is in.
fn describe_part(part: &Part, indent: usize, depth: usize, inflatable: &mut u64) {
    let kind = part.kind.as_str().unwrap_or("save");
    let role = part.role.as_ref().map_or("primary", |r| r.as_str());
    // A save's own product code, where the card carried one. It is what identifies a part
    // whose format writes no name — a Neo Geo card has no filenames at all, and a game that
    // skips the BIOS's title convention leaves `path` empty with nothing else to go on.
    let serial = printable(part.game.as_ref().and_then(|game| game.serial.as_deref()).unwrap_or(""));
    let path = printable(part.path.as_deref().unwrap_or(""));
    let slot = part.slot.map_or_else(String::new, |s| format!("slot {s}"));
    // A card can hold 200 saves, so the id is padded as a token rather than as a number: a
    // bracket that moves is worse to read down a column than one that does not.
    let id = format!("[{}]", part.id);
    // The head of the digest is enough to see at a glance that two parts hold the same bytes,
    // which is the question a reader comparing two bundles is usually asking.
    let digest = part.sha256.to_string();
    let short = digest.strip_prefix("sha256:").unwrap_or(&digest);
    // Every column is fixed-width except the last, and `path` is last because it is the one
    // with no bound worth padding to: a GameCube filename runs to 32 characters where a VMU's
    // stops at 12. Padding it would misalign every line a long name appears on, so it goes
    // where nothing follows it instead.
    //
    // Trimmed because the tail columns are all optional — a flat save has no serial, no slot
    // and no path — and the line would otherwise end in the padding for all three.
    let line = format!(
        "{:indent$}{id:<5} {kind:10} {role:16} {:>9} bytes  {}  {serial:12} {slot:8} {path}",
        "",
        part.payload.len(),
        &short[..16]
    );
    println!("{}", line.trim_end());

    // What the part says about itself beyond its columns, one field a line, under the id.
    let indent = indent + 6;
    // Validation reads an embedded nested bundle but leaves a compressed one alone, so opening one
    // can still fail: on a build without `zstd`, or on a payload that does not inflate to a bundle.
    // The error is escaped like any other text out of the file, since it can quote some.
    let open = || {
        part.bytes()
            .and_then(|bytes| Bundle::from_slice(&bytes))
            .map_err(|e| format!("cannot be opened: {}", printable(&e.to_string())))
    };
    let inner = match &part.payload {
        _ if !matches!(part.kind, PartKind::Bundle) => None,
        _ if depth >= MAX_DEPTH => Some(Err(format!("not listed, being more than {MAX_DEPTH} deep"))),
        Payload::Compressed { size, .. } if *size > *inflatable => Some(Err(format!(
            "not inflated, claiming {size} bytes where this listing has {inflatable} left to inflate"
        ))),
        // Charged whether or not it inflates to a bundle: the work is done either way.
        Payload::Compressed { size, .. } => {
            *inflatable -= size;
            Some(open())
        }
        Payload::Embedded(_) => Some(open()),
    };
    let held = inner.as_ref().and_then(|inner| inner.as_ref().ok()).map(|inner| &inner.header);

    if let Some(source) = &part.source {
        describe_source(indent, source);
    }
    // On a `bundle` part, `game` and `system` are index copies of the header it holds, which is
    // listed below. A copy that agrees with it says nothing new; one that does not is worth seeing.
    if let Some(game) = &part.game
        && held.is_none_or(|held| held.game.as_ref() != Some(game))
    {
        describe_game(indent, game);
    }
    if let Some(system) = &part.system
        && held.is_none_or(|held| held.system.as_ref() != Some(system))
    {
        field(indent, "system", &system_name(system));
    }
    if let Some(binding) = &part.binding {
        let listed = if one_saves_registry::is_binding(binding.as_str()) { "" } else { " (unlisted)" };
        field(indent, "binding", &format!("{binding}{listed}"));
    }
    if let Some(content_type) = &part.content_type {
        field(indent, "content type", &printable(content_type));
    }
    if let Payload::Compressed { bytes, .. } = &part.payload {
        field(indent, "encoding", &format!("zstd, {} bytes stored", bytes.len()));
    }
    if let Some(dirent) = &part.dirent {
        field(indent, "dirent", &shown_bytes(dirent));
    }
    for key in part.extensions.keys() {
        field(indent, "extension", &describe_extension(&part.extensions, key));
    }
    for line in unknown_keys(&part.unknown) {
        field(indent, "unknown", &line);
    }
    match inner {
        Some(Ok(inner)) => describe(&inner, indent + 2, depth + 1, inflatable),
        Some(Err(why)) => field(indent, "bundle", &why),
        None => {}
    }
}

/// Who produced some bytes: the app, the core it ran, the platform it ran on and what kind of
/// producer it is on one line, each with its version, and the rest beneath.
fn describe_source(indent: usize, source: &Source) {
    let app = layer("app", "", source.app.as_ref(), source.app_version.as_deref(), |slug| {
        one_saves_registry::app(slug).map(|app| app.name)
    });
    let core = layer("core", "core ", source.core.as_ref(), source.core_version.as_deref(), |slug| {
        one_saves_registry::core(slug).map(|core| core.name)
    });
    let platform = layer(
        "platform",
        "platform ",
        source.platform.as_ref(),
        source.platform_version.as_deref(),
        |slug| one_saves_registry::platform(slug).map(|platform| platform.name),
    );
    let kind = source.device_kind.as_ref().map(|kind| {
        let listed = if one_saves_registry::is_device_kind(kind.as_str()) { "" } else { " (unlisted)" };
        format!("{kind}{listed}")
    });
    let bits: Vec<String> = [app, core, platform, kind].into_iter().flatten().collect();
    field(indent, "source", &bits.join(", "));
    if let Some(fingerprint) = &source.fingerprint {
        field(indent, "", &format!("fingerprint {}", printable(fingerprint)));
    }
    for line in unknown_keys(&source.unknown) {
        field(indent, "", &format!("unknown {line}"));
    }
}

/// One layer of a `source`: the name with what its registry calls it, and the version beside it.
///
/// A listed name has to be written as its slug, so a slug is always one to look up, and one that
/// finds nothing is said to be unlisted. The app goes unlabelled, being the producer itself.
fn layer(
    what: &str,
    label: &str,
    name: Option<&Name>,
    version: Option<&str>,
    lookup: impl Fn(&str) -> Option<&'static str>,
) -> Option<String> {
    let name = name.map(|name| match name.as_slug() {
        Some(slug) => format!("{name} ({})", lookup(slug.as_str()).unwrap_or("unlisted")),
        None => name.to_string(),
    });
    match (name, version.map(printable)) {
        (Some(name), Some(version)) => Some(format!("{label}{name} {version}")),
        (Some(name), None) => Some(format!("{label}{name}")),
        // A version of nothing named is odd, but it is what the bundle says.
        (None, Some(version)) => Some(format!("{what} version {version}")),
        (None, None) => None,
    }
}

/// The clues to a game: what a person knows it by on the first line, and the rest beneath.
fn describe_game(indent: usize, game: &Game) {
    // Everything a producer wrote as free text goes through `printable`; slugs and names have
    // a grammar that keeps them safe to print already.
    let bits: Vec<String> = [&game.title, &game.serial, &game.rom_filename]
        .into_iter()
        .flatten()
        .map(|text| printable(text))
        .collect();
    field(indent, "game", &bits.join(", "));
    // Which system's release this is, which need not be the system that loads the save.
    if let Some(system) = &game.system {
        field(indent, "", &format!("system {}", system_name(system)));
    }
    for hash in &game.rom_hashes {
        field(indent, "", &hash.to_string());
    }
    for (resolver, id) in &game.game_id {
        let id = match id {
            one_saves::GameId::Uint(n) => n.to_string(),
            one_saves::GameId::Text(t) => printable(t),
        };
        field(indent, "", &format!("{resolver} = {id}"));
    }
    // Clues none of the fields above name, such as a ROM header's revision byte.
    for key in game.extensions.keys() {
        field(indent, "", &format!("extension {}", describe_extension(&game.extensions, key)));
    }
    for line in unknown_keys(&game.unknown) {
        field(indent, "", &format!("unknown {line}"));
    }
}

/// A system slug, and the name the registry knows it by.
fn system_name(system: &one_saves::Slug) -> String {
    format!("{system} ({})", one_saves_registry::system(system.as_str()).map_or("unlisted", |s| s.name))
}

/// Integer keys from a later minor version, one `key N = value` each.
///
/// Nothing here knows what such a key means, so its value is CBOR in diagnostic notation, cut at
/// the length an extension value is.
fn unknown_keys(keys: &one_saves::UnknownKeys) -> impl Iterator<Item = String> + '_ {
    keys.iter().map(|(key, value)| {
        let text = value.diagnostic_flat();
        let text = match text.char_indices().nth(SHOWN_CHARS - 1) {
            Some((cut, _)) if text.chars().nth(SHOWN_CHARS).is_some() => format!("{}…", &text[..cut]),
            _ => text,
        };
        format!("key {key} = {}", printable(&text))
    })
}

/// Bytes the format keeps opaque, as the head of them in hex and how many there are.
///
/// The form a long byte string in an extension value takes, so the two read alike in one listing.
fn shown_bytes(bytes: &[u8]) -> String {
    let mut head = String::new();
    for byte in bytes.iter().take(SHOWN_BYTES) {
        write!(head, "{byte:02x}").expect("a String takes any write");
    }
    let more = if bytes.len() > SHOWN_BYTES { "…" } else { "" };
    format!("h'{head}{more}' ({} bytes)", bytes.len())
}

/// An extension key and its value on one line: the value's own line where it is typed, its CBOR
/// in diagnostic notation where it is not, cut short either way.
fn describe_extension(extensions: &one_saves::Extensions, key: &one_saves::ReverseDnsName) -> String {
    match extensions.display(key) {
        Some(value) => format!("{key} = {}", printable(&value.to_string())),
        None => key.to_string(),
    }
}

/// Text out of a bundle, made safe to write to a terminal.
///
/// A bundle is whatever file someone handed over, and a title in it can hold an escape sequence
/// that moves the cursor, retitles the window or rewrites what was printed before it. Every
/// control character is written as its escape instead, the way `{:?}` would.
fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            out.extend(character.escape_default());
        } else {
            out.push(character);
        }
    }
    out
}

fn verify(path: &Path) -> Fallible {
    let bytes = std::fs::read(path)?;
    let bundle = Bundle::from_slice(&bytes)?;

    // Decoding already ran every structural rule; what is left is the digests, which are checked
    // against the bytes rather than trusted.
    let mut bad = 0;
    for part in &bundle.parts {
        match part.verify() {
            Ok(true) => {}
            Ok(false) => {
                bad += 1;
                eprintln!("  part {}: sha256 does not match its payload", part.id);
            }
            Err(e) => {
                bad += 1;
                eprintln!("  part {}: {e}", part.id);
            }
        }
    }

    // Re-encoding must reproduce the file byte for byte. That is the property the content hash
    // rests on, and the one most likely to be quietly wrong.
    if bundle.to_vec()? != bytes {
        bad += 1;
        eprintln!("  this file is not the deterministic encoding of what it says");
    }

    if bad > 0 {
        return Err(format!("{bad} problem(s) in {}", path.display()).into());
    }
    println!("{}: ok ({} part(s))", path.display(), bundle.parts.len());
    Ok(())
}

fn hash(path: &Path) -> Fallible {
    let bytes = std::fs::read(path)?;
    let bundle = Bundle::from_slice(&bytes)?;
    println!("file    {}", one_saves::HashValue::sha256_of(&bytes));
    match bundle.content_hash() {
        Ok(content) => println!("content {content}"),
        Err(e) => println!("content unavailable: {e}"),
    }
    Ok(())
}

fn profiles() {
    println!("{:<14} {:<18} {:<12} {:<18} SYSTEMS", "NAME", "DEVICE KIND", "APP", "CORE");
    for profile in one_saves_convert::profile::profiles() {
        let systems =
            if profile.systems.is_empty() { "(any)".to_owned() } else { profile.systems.join(", ") };
        println!(
            "{:<14} {:<18} {:<12} {:<18} {systems}",
            profile.key,
            profile.device_kind,
            profile.app.unwrap_or("-"),
            profile.core.unwrap_or("-")
        );
    }
    println!(
        "\nA dash is a name neither registry lists, which a bundle leaves out rather than invent one for.\n\
         Any of the {} apps and {} cores in the registry also works by slug or name, and --core and\n\
         --platform name the core an app ran and the firmware it ran on.",
        one_saves_registry::apps().len(),
        one_saves_registry::cores().len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_from_a_bundle_cannot_drive_the_terminal() {
        assert_eq!(printable("FINAL FANTASY VII"), "FINAL FANTASY VII");
        assert_eq!(printable("ＴＥＫＫＥＮ　４"), "ＴＥＫＫＥＮ　４");
        assert_eq!(printable("\u{1b}]0;owned\u{7}\u{1b}[2J"), r"\u{1b}]0;owned\u{7}\u{1b}[2J");
        assert_eq!(printable("two\nlines\u{9b}"), r"two\nlines\u{9b}");
    }
}
