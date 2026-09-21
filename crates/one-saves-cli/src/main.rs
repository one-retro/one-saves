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
    ///
    /// The input may be a .zip holding a card, or holding loose PlayStation saves, in which
    /// case a card is built to carry them.
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

/// Without `archive`, a file is only ever itself.
// Infallible, but it stands in for one that is not, so it keeps the signature.
#[allow(clippy::unnecessary_wraps)]
#[cfg(not(feature = "archive"))]
fn unwrap_archive(
    input: &std::path::Path,
    bytes: Vec<u8>,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    Ok((bytes, extension_of(input)))
}

fn convert(args: Convert) -> Fallible {
    let (bytes, extension) = unwrap_archive(&args.input, std::fs::read(&args.input)?)?;

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

    let Identified { mut game, system: rom_system } = identify(&args)?;

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

/// Stands in for the above in a build without `rom`, where there is no `--rom` to read.
///
/// It cannot fail, having nothing to read, but it keeps the signature so the caller does not have
/// to know which build it is in.
#[cfg(not(feature = "rom"))]
#[allow(clippy::unnecessary_wraps)]
fn identify(_args: &Convert) -> Result<Identified, Box<dyn std::error::Error>> {
    Ok(Identified::default())
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
    let header = &bundle.header;

    println!("{}", path.display());
    println!("  {} bytes, spec version {}", bytes.len(), one_saves::SPEC_VERSION);
    // What the bundle *is*, which decides how the parts below it should be read. It comes from
    // the container crate rather than from a rule spelled out here, so this listing and any other
    // consumer classify the same file the same way.
    println!("  shape        {}", bundle.shape());
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
        if let Some(name) = &game.title {
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
            println!("  clock        {}", describe_clock(value, &header.extensions));
        } else {
            println!("  extension    {key}");
        }
    }

    println!("  parts        {}", bundle.parts.len());
    for part in &bundle.parts {
        let kind = part.kind.as_str().unwrap_or("save");
        let role = part.role.as_ref().map_or("primary", |r| r.as_str());
        // A save's own product code, where the card carried one. It is what identifies a part
        // whose format writes no name — a Neo Geo card has no filenames at all, and a game that
        // skips the BIOS's title convention leaves `path` empty with nothing else to go on.
        let serial = part.game.as_ref().and_then(|game| game.serial.as_deref()).unwrap_or("");
        let path = part.path.as_deref().unwrap_or("");
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
            "    {id:<5} {kind:10} {role:16} {:>9} bytes  {}  {serial:12} {slot:8} {path}",
            part.payload.len(),
            &short[..16]
        );
        println!("{}", line.trim_end());
    }
    Ok(())
}

/// Renders an `x.1sav.rtc` value: what the clock showed, as a Unix instant.
///
/// Which chip produced it is not part of the value — the key names an instant and nothing else —
/// so the chip is named from whichever sibling key sits beside it.
fn describe_clock(value: &one_saves::dcbor::CBOR, extensions: &one_saves::Extensions) -> String {
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

    // A sibling under the same root is a chip's own state; its last label is the chip's name.
    let chip = extensions
        .keys()
        .filter(|k| k.as_str() != one_saves_convert::rtc::RTC_KEY)
        .find_map(|k| k.as_str().strip_prefix(&format!("{}.", one_saves_convert::rtc::RTC_KEY)));

    let accuracy = map
        .get::<u64, one_saves::dcbor::CBOR>(1)
        .and_then(|a| match a.as_case() {
            one_saves::dcbor::CBORCase::Unsigned(ms) => Some(format!(", ±{ms}ms")),
            _ => None,
        })
        .unwrap_or_default();

    match chip {
        Some(chip) => format!("epoch {instant}{accuracy} ({chip})"),
        None => format!("epoch {instant}{accuracy}"),
    }
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
