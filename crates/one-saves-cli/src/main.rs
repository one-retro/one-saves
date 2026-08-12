//! The `1saves` command-line tool.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use one_saves::{Bundle, PartKind};
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
    Convert(Convert),
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
    /// The emulator, core or device the save came from, or the input format.
    #[arg(long, value_name = "NAME")]
    from: Option<String>,
    /// The system these bytes are a save for, as a registry slug.
    #[arg(long, value_name = "SLUG")]
    system: Option<String>,
    /// The socket they came out of, as a registry slug. Absent means `primary`.
    #[arg(long, value_name = "SLUG")]
    role: Option<String>,
    /// The version of the producing software, recorded as `source.app_version`.
    #[arg(long, value_name = "VERSION")]
    app_version: Option<String>,
    /// A human-readable note about this bundle.
    #[arg(long)]
    description: Option<String>,
    /// The game's ROM, read for its header fields and hashed for `rom_hashes`.
    #[arg(long, value_name = "FILE")]
    rom: Option<PathBuf>,
    /// A No-Intro, Redump or TOSEC DAT to resolve the ROM's digest into a canonical name.
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
        Command::Convert(args) => convert(args),
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

fn convert(args: Convert) -> Fallible {
    let bytes = std::fs::read(&args.input)?;

    // `--from` names either a producer (mgba, duckstation) or a format (ps1, raw). A producer
    // says who wrote the bytes; the format is still detected from the bytes themselves.
    let profile = args.from.as_deref().and_then(one_saves_convert::profile);
    let named_format = args.from.as_deref().and_then(Format::from_name);
    let format = match named_format {
        Some(format) => format,
        None => detect::detect(&bytes, &extension_of(&args.input))?,
    };

    if format == Format::Bundle {
        return Err("this file is already a bundle".into());
    }

    let mut game = identify(args.rom.as_deref(), args.dat.as_deref())?;

    // A clock kept in its own file has to be found, since the save does not mention it. The file
    // records an origin rather than a reading, so an instant is what turns one into the other.
    let sidecar_path = args.rtc_file.clone().or_else(|| {
        let beside = args.input.with_extension("rtc");
        beside.exists().then_some(beside)
    });
    if args.rtc_captured_at.is_some() && sidecar_path.is_none() {
        return Err("--rtc-captured-at was given, but there is no sidecar clock to apply it to".into());
    }

    let sidecar = match &sidecar_path {
        Some(path) => {
            let bytes = std::fs::read(path)?;
            // Falling back to the modification time is a guess, and one that gets worse the
            // further the file has travelled: copying, unzipping and checking out all reset it.
            // It is only the right answer for a file the emulator wrote in place.
            let captured_at = if let Some(stated) = args.rtc_captured_at {
                stated
            } else {
                let mtime = std::fs::metadata(path)?
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| format!("{} has a modification time before 1970: {e}", path.display()))?
                    .as_secs();
                i64::try_from(mtime)?
            };
            Some((bytes, captured_at))
        }
        None => None,
    };

    let source = profile.as_ref().map(|p| p.source(args.app_version.clone()));
    // A profile that covers exactly one system settles `system` when the user did not say.
    let rom_system = args
        .rom
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|rom| one_saves_convert::rom::identify(&rom))
        .and_then(|info| info.system);
    let system = args
        .system
        .clone()
        .or_else(|| rom_system.map(ToOwned::to_owned))
        .or_else(|| profile.as_ref().and_then(Profile::only_system).map(ToOwned::to_owned));

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
            options.role =
                one_saves::Slug::parse(role).map_err(|e| format!("--role {role:?} is not a slug: {e}"))?;
        }
        read_card(format, &bytes, &options)?
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

    let saves = bundle.parts.iter().filter(|p| p.kind == PartKind::Bundle).count();
    let what = if saves > 0 { format!("{saves} save(s)") } else { format!("{} part(s)", bundle.parts.len()) };
    println!(
        "{} -> {} ({}, {what}, {} bytes)",
        args.input.display(),
        output.display(),
        format.label(),
        encoded.len()
    );
    Ok(())
}

/// Works out what game a save belongs to, from a ROM's header and a catalog.
///
/// A ROM says what the save cannot: its header gives a title and often a product code, and its
/// digests are what a catalog is keyed on.
fn identify(
    rom: Option<&Path>,
    dat: Option<&Path>,
) -> Result<Option<one_saves::Game>, Box<dyn std::error::Error>> {
    let Some(rom_path) = rom else {
        return Ok(None);
    };
    let bytes = std::fs::read(rom_path)?;
    let filename = rom_path.file_name().and_then(|n| n.to_str());
    let mut game = one_saves_convert::rom::game_from_rom(&bytes, filename);

    if let Some(dat) = dat {
        let catalog = one_saves_convert::dat::Catalog::open(dat)?;
        if catalog.enrich(&mut game) {
            println!("identified as {:?}", game.name.as_deref().unwrap_or_default());
        } else {
            eprintln!("note: this ROM is not in {}", dat.display());
        }
    }
    Ok(Some(game))
}

fn read_card(format: Format, bytes: &[u8], options: &card::CardOptions) -> one_saves_convert::Result<Bundle> {
    match format {
        Format::Ps1Card => card::ps1::read(bytes, options),
        Format::N64Pak => card::n64::read(bytes, options),
        Format::GcCard => card::gc::read(bytes, options),
        Format::Vmu => card::vmu::read(bytes, options),
        Format::Ps2Card => card::ps2::read(bytes, options),
        Format::Raw | Format::Bundle => unreachable!("handled by the caller"),
    }
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
            Some("ps1-mc") => Format::Ps1Card,
            Some("n64-cpak") => Format::N64Pak,
            Some("gc-mc") => Format::GcCard,
            Some("vmu") => Format::Vmu,
            Some("ps2-mc") => Format::Ps2Card,
            Some(other) => {
                return Err(format!("this bundle is a {other}, which this build cannot write").into());
            }
            None => Format::Raw,
        },
    };

    let (payload, suffix) = match target {
        Format::Raw => (raw::unwrap(&bundle)?, "srm".to_owned()),
        Format::Ps1Card => (card::ps1::write(&bundle)?, "mcr".to_owned()),
        Format::N64Pak => (card::n64::write(&bundle)?, "mpk".to_owned()),
        Format::GcCard => (card::gc::write(&bundle)?, "raw".to_owned()),
        Format::Vmu => (card::vmu::write(&bundle)?, "bin".to_owned()),
        // A dump's spare area is regenerated rather than stored, so writing one back is a
        // choice the caller makes with --to ps2-ecc rather than something the bundle records.
        Format::Ps2Card => (card::ps2::write(&bundle)?, "ps2".to_owned()),
        Format::Bundle => return Err("extracting a bundle as a bundle is a copy".into()),
    };

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

    let output = args.output.unwrap_or_else(|| args.input.with_extension(&suffix));
    write_out(&output, &payload, args.force)?;
    println!(
        "{} -> {} ({}, {} bytes)",
        args.input.display(),
        output.display(),
        target.label(),
        payload.len()
    );
    Ok(())
}

fn inspect(path: &Path) -> Fallible {
    let bytes = std::fs::read(path)?;
    let bundle = Bundle::from_slice(&bytes)?;
    let header = &bundle.header;

    println!("{}", path.display());
    println!("  {} bytes, spec version {}", bytes.len(), one_saves::SPEC_VERSION);
    if let Some(system) = &header.system {
        let name = one_saves_registry::system(system.as_str()).map_or("unlisted", |s| s.name);
        println!("  system       {system} ({name})");
    }
    if let Some(card) = &header.card {
        println!("  card         {}, capacity {} bytes", card.format, card.capacity);
        if let Some(area) = &card.system_area {
            println!("               system area {} bytes", area.len());
        }
    }
    if let Some(game) = &header.game {
        let mut bits = Vec::new();
        if let Some(name) = &game.name {
            bits.push(name.clone());
        }
        if let Some(serial) = &game.serial {
            bits.push(serial.clone());
        }
        if let Some(file) = &game.rom_filename {
            bits.push(file.clone());
        }
        println!("  game         {}", bits.join(", "));
        for hash in &game.rom_hashes {
            println!("               {hash}");
        }
        for (resolver, id) in &game.game_id {
            let id = match id {
                one_saves::GameId::Uint(n) => n.to_string(),
                one_saves::GameId::Text(t) => t.clone(),
            };
            println!("               {resolver} = {id}");
        }
    } else {
        println!("  game         unidentified");
    }
    if let Some(source) = &header.source {
        let app = source.app.as_ref().map_or_else(|| "?".to_owned(), ToString::to_string);
        let kind = source.device_kind.as_ref().map_or_else(|| "?".to_owned(), ToString::to_string);
        let version = source.app_version.as_deref().unwrap_or("");
        println!("  source       {app} {version} ({kind})");
    }
    if let Some(description) = &header.description {
        println!("  description  {description}");
    }
    for (key, value) in &header.extensions {
        if key.as_str() == one_saves_convert::rtc::RTC_KEY {
            println!("  clock        {}", describe_clock(value));
        } else {
            println!("  extension    {key}");
        }
    }

    println!("  parts        {}", bundle.parts.len());
    for part in &bundle.parts {
        let kind = part.kind.as_str().unwrap_or("save");
        let role = part.role.as_ref().map_or("primary", |r| r.as_str());
        let path = part.path.as_deref().unwrap_or("");
        let slot = part.slot.map_or_else(String::new, |s| format!("slot {s}"));
        // The head of the digest is enough to see at a glance that two parts hold the same bytes,
        // which is the question a reader comparing two bundles is usually asking.
        let digest = part.sha256.to_string();
        let short = digest.strip_prefix("sha256:").unwrap_or(&digest);
        println!(
            "    [{}] {kind:10} {role:16} {:>9} bytes  {}  {path} {slot}",
            part.id,
            part.payload.len(),
            &short[..16]
        );
    }
    Ok(())
}

/// Renders an `x.1sav.rtc` value, which is a Unix instant and the clock that produced it.
fn describe_clock(value: &one_saves::dcbor::CBOR) -> String {
    let Some(map) = value.as_map() else {
        return "unreadable".to_owned();
    };
    let instant = map
        .get::<u64, one_saves::dcbor::CBOR>(0)
        .and_then(|tagged| match tagged.as_case() {
            // Tag 1 over whole seconds, which is the only shape the key admits.
            one_saves::dcbor::CBORCase::Tagged(_, inner) => match inner.as_case() {
                one_saves::dcbor::CBORCase::Unsigned(n) => Some(n.to_string()),
                one_saves::dcbor::CBORCase::Negative(n) => Some(format!("-{}", i128::from(*n) + 1)),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| "?".to_owned());
    let clock = map
        .get::<u64, one_saves::dcbor::CBOR>(1)
        .and_then(|c| c.as_text().map(ToOwned::to_owned))
        .unwrap_or_else(|| "?".to_owned());
    format!("{clock}, read at epoch {instant}")
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
    println!("{:<14} {:<18} {:<28} SYSTEMS", "NAME", "DEVICE KIND", "APP");
    for profile in one_saves_convert::profile::profiles() {
        let systems =
            if profile.systems.is_empty() { "(any)".to_owned() } else { profile.systems.join(", ") };
        println!("{:<14} {:<18} {:<28} {systems}", profile.key, profile.device_kind, profile.app);
    }
    println!(
        "\nAny of the {} cores in the registry also works by slug or name.",
        one_saves_registry::cores().len()
    );
}
