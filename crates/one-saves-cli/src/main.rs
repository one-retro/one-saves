//! The `1saves` command-line tool.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use one_saves::extension::{Civil, SHOWN_BYTES, SHOWN_CHARS};
use one_saves::{Bundle, Game, Name, Origin, Part, PartKind, Payload, Recorded, Source, TakenWith};
use one_saves_convert::{Format, Profile, card, detect, raw};

mod n64;
mod state;

/// Move retro saves and memory cards in and out of the Universal Saves Format.
#[derive(Parser)]
#[command(name = "1saves", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Wrap a save, a memory card, a save state or a screenshot into a .1saves bundle.
    ///
    /// The input may be a .zip holding a card, or holding loose PlayStation saves, in which
    /// case a card is built to carry them. A RetroArch, mGBA or DuckStation state is recognised
    /// as one, and RetroArch's thumbnail beside it is taken along.
    Convert(Box<Convert>),
    /// Write a bundle back out in the format it came from: a save, a card, an emulator's state
    /// file, or a screenshot's image.
    Extract(Extract),
    /// Make an empty, formatted memory card.
    ///
    /// Written as the card's own file, which is what an emulator or a flash cart loads and what
    /// `extract` writes for a card. Name the output .1saves for a bundle holding it instead.
    New(New),
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
    /// A RetroArch state needs it, since only that core loads the state and the file does not say
    /// which it was. Where RetroArch sorted its states into a folder per core, the folder says.
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
    /// The game's ROM, read for its header fields and hashed for `rom_hashes`. Its header also
    /// says which clock chip the cartridge has, which is what reads a MiSTer HuC-3's clock.
    #[cfg(feature = "rom")]
    #[arg(long, value_name = "FILE")]
    rom: Option<PathBuf>,
    /// A No-Intro, Redump or TOSEC DAT to resolve the ROM's digest into a canonical name.
    #[cfg(feature = "dat")]
    #[arg(long, value_name = "FILE")]
    dat: Option<PathBuf>,
    /// A sidecar clock file written beside the save, such as Gambatte's or mincer_ray's `.rtc`.
    ///
    /// Found automatically next to the input when it is named after it; pass this to point
    /// somewhere else, or to supply one that has been moved away from its save.
    #[arg(long, value_name = "FILE")]
    rtc_file: Option<PathBuf>,
    /// The moment to read the sidecar clock at, in epoch seconds. Defaults to now.
    ///
    /// Gambatte's sidecar stores the instant the clock counts *from*, not what it reads. Its
    /// cartridge clock follows the host's, so the counters at any moment are the time since then,
    /// and the bundle carries them as they stand at this moment, with this moment as the anchor.
    ///
    /// The origin round-trips whatever this is; only the reading moves with it. mincer_ray's
    /// sidecar records its own instant, and takes none.
    #[arg(long, value_name = "EPOCH")]
    rtc_captured_at: Option<i64>,
    /// For an N64 cartridge save, the chip the cartridge has: eeprom-4k, eeprom-16k, sram-256k,
    /// sram-768k or flash-1m.
    ///
    /// Neither a file's bytes nor its length always say, and the chip is what a restore writes.
    /// `--rom` settles it for a homebrew ROM, whose header names it.
    #[arg(long, value_name = "CHIP")]
    n64_save: Option<String>,
    /// For an N64 SRAM or FlashRAM save, the order the file keeps the chip's words in: `console`,
    /// as an EverDrive writes it, or `swapped`, as mupen64plus and Project64 do.
    ///
    /// `--from everdrive` and `--from mupen64plus` say the same, so this is for a file that came
    /// from somewhere else.
    #[arg(long, value_name = "ORDER")]
    n64_order: Option<String>,
    /// For a file holding Controller Paks, keep only this game's notes beside its cartridge, by the
    /// four-character code its ROM header and its notes carry, such as `NKTE`.
    ///
    /// Without it the paks are kept whole, other games' notes and all.
    #[arg(long, value_name = "CODE")]
    n64_game: Option<String>,
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
    ///
    /// For a state, the emulator to rebuild its file for: retroarch, mgba or duckstation. A state
    /// loads only into the core that took it, so it is refused for an emulator that does not run
    /// that core.
    #[arg(long, value_name = "NAME")]
    to: Option<String>,
    /// Extract one part's payload by id, rather than the whole thing.
    #[arg(long, value_name = "ID")]
    part: Option<u64>,
    /// For an N64 save, the emulator or cartridge to write it out for, which decides the files and
    /// their names.
    #[arg(long, value_name = "TARGET")]
    n64: Option<n64::Target>,
    /// For an N64 save, the file to write it onto: a `.srm` for RetroArch, an `.mpk` otherwise.
    ///
    /// What the bundle says nothing about is kept from it, so one game's notes go onto a pak without
    /// taking the others off.
    #[arg(long, value_name = "FILE")]
    onto: Option<PathBuf>,
    /// Write the clock to a sidecar file instead of appending it: Gambatte's for a Game Boy clock,
    /// mincer_ray's for a GBA one.
    #[arg(long, value_name = "FILE")]
    rtc_file: Option<PathBuf>,
    /// Overwrite the output if it exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct New {
    /// The kind of card: ps1, ps2, gc, n64, vmu, saturn or neogeo.
    kind: String,
    /// Where to write it.
    output: PathBuf,
    /// How big a card, for a kind that comes in more than one size. Defaults to the kind's
    /// standard one.
    ///
    /// PS2: 8M, Sony's own, or 16M, 32M or 64M. GameCube: 59, 123, 251, 507, 1019 or 2043 blocks,
    /// 251 unless told. Saturn: 32K, the console's own memory, or a 512K, 1M, 2M or 4M cart. Neo
    /// Geo: 2K, SNK's own, up to 16K in steps of 2K.
    #[arg(long)]
    size: Option<String>,
    /// For a GameCube card, format it as a Japanese console does: for Shift-JIS names.
    ///
    /// A game compares a card's character set with the console's and offers to format a card
    /// that disagrees, so a card for a Japanese console needs this and any other refuses it.
    #[arg(long)]
    shift_jis: bool,
    /// How the card sits in its file, where an emulator wraps it: `wide` for a Saturn volume as
    /// Yabause keeps it, `mister` for a Neo Geo card as the MiSTer core's save. Defaults to
    /// `bare`, the card and nothing else.
    #[arg(long)]
    layout: Option<String>,
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
        Command::New(args) => new_card(&args),
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

/// The clock kept beside a save, and the instant it was read at when the file needs one.
///
/// A clock in its own file has to be found, since the save does not mention it. Gambatte's records
/// an origin rather than a reading, so an instant is what turns one into the other; mincer_ray's
/// records the instant itself, and has none here.
type Sidecar = (std::path::PathBuf, Vec<u8>, Option<i64>);

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
    if bytes.len() != one_saves_convert::rtc::GAMBATTE_RTC_LEN {
        if args.rtc_captured_at.is_some() {
            return Err(format!(
                "--rtc-captured-at was given, but {} records its own instant",
                path.display()
            )
            .into());
        }
        return Ok(Some((path, bytes, None)));
    }
    // The counters are taken as they stand when they are read, which is now. The file's
    // modification time would be a guess at an earlier moment, and one that copying, unzipping or
    // checking out resets.
    let captured_at = if let Some(stated) = args.rtc_captured_at {
        stated
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("the system clock is before 1970: {e}"))?
            .as_secs();
        i64::try_from(now)?
    };
    Ok(Some((path, bytes, Some(captured_at))))
}

fn convert(args: Convert) -> Fallible {
    #[cfg(feature = "archive")]
    let (bytes, extension) = unwrap_archive(&args.input, std::fs::read(&args.input)?)?;
    // Without `archive`, a file is only ever itself.
    #[cfg(not(feature = "archive"))]
    let (bytes, extension) = (std::fs::read(&args.input)?, extension_of(&args.input));

    // `--from` names either a producer (mgba, duckstation) or a format (ps1, raw). A producer
    // says who wrote the bytes; the format is still detected from the bytes themselves.
    // A save state or an image is its own thing, and looks like a save to the detection below: a
    // DuckStation state is a `.sav`, and an mGBA one a PNG.
    if let Some(kind) = state::kind_of(&args.input, &bytes) {
        return state::convert(&args, kind, &bytes);
    }

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
    let Identified { mut game, system: rom_system, clock: rom_clock, n64_save: rom_n64_save } =
        identify(&args)?;
    // Without `rom` there is no `--rom` to read, so there is nothing a ROM could have said.
    #[cfg(not(feature = "rom"))]
    let Identified { mut game, system: rom_system, clock: rom_clock, n64_save: rom_n64_save } =
        Identified::default();

    let sidecar = sidecar_clock(&args)?;
    let sidecar_path = sidecar.as_ref().map(|(path, _, at)| (path.clone(), at.is_some()));
    // A sidecar that records its own instant ignores the one passed beside it.
    let sidecar = sidecar.map(|(_, bytes, at)| (bytes, at.unwrap_or_default()));

    let source = source_of(&args, profile.as_ref())?;
    let system = system_of(&args, rom_system, profile.as_ref(), source.as_ref());

    let mut label = format.label().to_owned();
    let mut bundle = if n64::is_n64(system.as_deref(), &extension, &bytes) {
        let (bundle, read_as) = n64::convert(&args, &bytes, &extension, source, game.clone(), rom_n64_save)?;
        label = read_as;
        bundle
    } else if format == Format::Raw {
        let options = raw::RawOptions {
            system,
            role: args.role.clone(),
            source,
            game: game.clone(),
            description: args.description.clone(),
            split_rtc: !args.keep_rtc_inline,
            rtc_sidecar: sidecar,
            clock_chip: rom_clock.map(ToOwned::to_owned),
            ..raw::RawOptions::default()
        };
        raw::wrap(&bytes, &options)?
    } else {
        let options = card::CardOptions {
            role: args.role.as_deref().map(|role| slug_flag("--role", role)).transpose()?,
            // A format names its own system, except a Neo Geo card's, which can be a Neo Geo CD's.
            system: system.as_deref().map(|system| slug_flag("--system", system)).transpose()?,
            source,
        };
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

    report_clock(sidecar_path.as_ref(), args.rtc_captured_at.is_some(), &bundle);

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
        if backup_ram { "Sega CD backup RAM" } else { label.as_str() },
        encoded.len()
    );
    Ok(())
}

/// Says where a save's clock came from, when it had one.
fn report_clock(sidecar: Option<&(PathBuf, bool)>, instant_given: bool, bundle: &Bundle) {
    if let Some((path, needed_instant)) = sidecar {
        let how = if !needed_instant {
            "the instant it records"
        } else if instant_given {
            "the instant given"
        } else {
            "the time it was read"
        };
        println!("read the clock from {}, using {how}", path.display());
    } else if one_saves_convert::rtc::footer_of(bundle).is_some() {
        println!("split a real-time-clock footer off the save, so its bytes hash the same over time");
    }
}

/// The `source` a bundle carries: what `--from` names, and what the other flags add to it.
///
/// Every layer is checked against the registry before it is written, since `source` sits in the
/// header and a misspelt name changes the content hash as surely as a different one.
/// A flag's value as a slug, or an error naming the flag.
/// The system the bytes are for: what the user said, else what the ROM's header names, else the one
/// system a profile or a core covers, when it covers exactly one.
fn system_of(
    args: &Convert,
    rom_system: Option<&str>,
    profile: Option<&Profile>,
    source: Option<&Source>,
) -> Option<String> {
    let core_system = source
        .and_then(|source| source.core.as_ref())
        .and_then(|core| one_saves_registry::core(core.as_str()))
        .and_then(|core| match core.systems {
            [only] => Some(*only),
            _ => None,
        });
    args.system
        .clone()
        .or_else(|| rom_system.map(ToOwned::to_owned))
        .or_else(|| profile.and_then(Profile::only_system).map(ToOwned::to_owned))
        .or_else(|| core_system.map(ToOwned::to_owned))
}

fn slug_flag(flag: &str, text: &str) -> Result<one_saves::Slug, String> {
    one_saves::Slug::parse(text).map_err(|e| format!("{flag} {text:?} is not a slug: {e}"))
}

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
    /// The key of the clock chip the ROM's header says the cartridge has.
    clock: Option<&'static str>,
    /// The save chip an N64 ROM's header says the cartridge has.
    n64_save: Option<one_saves::extension::N64Save>,
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
    let info = one_saves_convert::rom::identify(&bytes).unwrap_or_default();

    #[cfg(feature = "dat")]
    if let Some(dat) = args.dat.as_deref() {
        let catalog = one_saves_convert::dat::Catalog::open(dat)?;
        if catalog.enrich(&mut game) {
            println!("identified as {:?}", game.title.as_deref().unwrap_or_default());
        } else {
            eprintln!("note: this ROM is not in {}", dat.display());
        }
    }
    Ok(Identified { game: Some(game), system: info.system, clock: info.clock, n64_save: info.n64_save })
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

    // A state rebuilds as its emulator's file, and a screenshot comes out as its image.
    if matches!(bundle.shape(), one_saves::Shape::State | one_saves::Shape::Screenshot) {
        return state::extract(&args, &bundle);
    }

    // An N64 save is several files or one, in either word order, depending on what reads it, so
    // which is the caller's to say.
    if let Some(target) = args.n64 {
        if !n64::is_n64_bundle(&bundle) {
            return Err("--n64 writes an N64 save, and this bundle is not one".into());
        }
        let base = args.output.unwrap_or_else(|| args.input.with_extension(""));
        return n64::extract(&bundle, target, &base, args.onto.as_deref(), args.force);
    }
    if n64::is_n64_bundle(&bundle) && args.to.is_none() {
        return Err("an N64 save is written differently for each emulator; pass --n64 retroarch, \
                    mupen64plus, project64 or everdrive"
            .into());
    }
    if args.onto.is_some() {
        return Err("--onto is for an N64 save, with --n64".into());
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

fn new_card(args: &New) -> Fallible {
    let format =
        Format::from_name(&args.kind).filter(|format| !card::sizes(*format).is_empty()).ok_or_else(|| {
            format!("{:?} is not a kind of memory card: ps1, ps2, gc, n64, vmu, saturn or neogeo", args.kind)
        })?;
    let size = match &args.size {
        Some(name) => card::size(format, name).ok_or_else(|| {
            let sizes: Vec<String> =
                card::sizes(format).iter().map(|size| format!("{} ({})", size.name, size.label)).collect();
            let (last, rest) = sizes.split_last().expect("every card has a size");
            let listed =
                if rest.is_empty() { last.clone() } else { format!("{} or {last}", rest.join(", ")) };
            format!("a {} comes in {listed}", format.label())
        })?,
        None => {
            card::sizes(format).iter().find(|size| size.standard).expect("every card has a standard size")
        }
    };
    let layout = match args.layout.as_deref() {
        Some(name) => card::Layout::from_name(name)
            .ok_or_else(|| format!("{name:?} is not a layout: bare, wide or mister"))?,
        None => card::Layout::Bare,
    };

    // A bundle records the card rather than the file an emulator keeps it in, so a wrapping asked
    // for here would be dropped on the way in. Better said than done quietly.
    let as_bundle = extension_of(&args.output) == one_saves::EXTENSION;
    if as_bundle && layout != card::Layout::Bare {
        return Err(
            "--layout is how an emulator's file wraps a card, and a bundle holds the card itself".into()
        );
    }

    let options = card::BlankOptions { capacity: Some(size.capacity), shift_jis: args.shift_jis, layout };
    let bytes = card::blank(format, &options)?;
    let bytes =
        if as_bundle { card::read(format, &bytes, &card::CardOptions::default())?.to_vec()? } else { bytes };
    write_out(&args.output, &bytes, args.force)?;
    println!("empty {} -> {} ({}, {} bytes)", format.label(), args.output.display(), size.label, bytes.len());
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
    if let Some(seconds) = header.bundled_at {
        field(indent, "bundled", &format!("{} UTC", Civil(seconds)));
    }
    // When a state or a screenshot was taken, and by whom when the player did not take it, are
    // what a list of slots shows, so they go beside when the bundle was made.
    if let Some(seconds) = header.taken_at {
        field(indent, "taken", &format!("{} UTC", Civil(seconds)));
    }
    if let Some(origin) = &header.origin {
        let what = match origin {
            Origin::Auto => "the emulator took it to resume from".to_owned(),
            Origin::Undo => "the emulator kept it to undo a save or a load".to_owned(),
            // Never shown as the player's: that is the one reading a consumer must not give it.
            Origin::Unknown(_) => format!("not an origin spec {} defines", one_saves::SPEC_VERSION),
        };
        field(indent, "origin", &format!("{origin} ({what})"));
    }
    if let Some(taken_with) = &header.taken_with {
        describe_taken_with(indent, taken_with);
    }
    match &header.game {
        Some(game) => describe_game(indent, game),
        // A device and a collection hold saves for many games, so neither has one to identify.
        None if matches!(shape, one_saves::Shape::Device | one_saves::Shape::Collection) => {}
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
    // A state's data and its picture take no role, so they come out of no socket to name.
    let role = match (&part.role, &part.kind) {
        (Some(role), _) => role.as_str(),
        (None, PartKind::State | PartKind::Screenshot) => "",
        (None, _) => "primary",
    };
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

/// What a state was taken with, one entry a line.
///
/// An entry the producer knows applies without knowing its value is said to be unknown, which is
/// not the same as an entry left out: a state that booted a BIOS nobody could name still booted
/// one.
fn describe_taken_with(indent: usize, taken_with: &TakenWith) {
    fn entry<T>(
        lines: &mut Vec<String>,
        name: &str,
        value: Option<&Recorded<T>>,
        show: impl Fn(&T) -> Vec<String>,
    ) {
        match value {
            None => {}
            Some(Recorded::Unknown) => lines.push(format!("{name} unknown")),
            Some(Recorded::Known(value)) => {
                lines.extend(show(value).into_iter().map(|line| format!("{name} {line}")));
            }
        }
    }

    let mut lines = Vec::new();
    entry(&mut lines, "byte order", taken_with.byte_order.as_ref(), |order| vec![order.as_str().to_owned()]);
    entry(&mut lines, "word size", taken_with.word_size.as_ref(), |size| {
        vec![format!("{} bits", size.bits())]
    });
    entry(&mut lines, "firmware", taken_with.firmware.as_ref(), |files| {
        files
            .iter()
            .map(|(file, hashes)| {
                let hashes: Vec<String> = hashes.iter().map(ToString::to_string).collect();
                format!("{} {}", printable(file), hashes.join(" "))
            })
            .collect()
    });
    entry(&mut lines, "language", taken_with.language.as_ref(), |language| vec![printable(language)]);
    entry(&mut lines, "option", taken_with.options.as_ref(), |options| {
        options.iter().map(|(name, value)| format!("{} = {}", printable(name), printable(value))).collect()
    });
    entry(&mut lines, "format version", taken_with.format_version.as_ref(), |version| {
        vec![version.to_string()]
    });
    lines.extend(
        taken_with
            .extensions
            .keys()
            .map(|key| format!("extension {}", describe_extension(&taken_with.extensions, key))),
    );
    lines.extend(unknown_keys(&taken_with.unknown).map(|line| format!("unknown {line}")));

    for (index, line) in lines.iter().enumerate() {
        field(indent, if index == 0 { "taken with" } else { "" }, line);
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
