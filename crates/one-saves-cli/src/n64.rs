//! Nintendo 64 saves: one chip's file, a libretro core's `.srm` of every chip and pak, and
//! mupen64plus's `.mpk` of four paks.
//!
//! What sets these apart from every other flat save is that a file does not say which chip it is
//! or which way round its words are, and both decide what a restore writes. So each is the
//! caller's to state where the file cannot prove it: `--n64-save` the chip, `--n64-order` the
//! words, with `--from` and `--rom` settling them where they can.

use std::path::{Path, PathBuf};

use one_saves::extension::N64Save;
use one_saves::{Bundle, Game, Shape, Source};
use one_saves_convert::n64::{self, N64Options, WordOrder};

use crate::{Convert, Fallible, write_out};

/// Where an N64 save is extracted for, which decides the files, their names and their word order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Target {
    /// Mupen64Plus-Next or ParaLLEl N64: one `.srm` holding every chip and all four paks.
    Retroarch,
    /// Standalone mupen64plus: the chip as `.eep`, `.sra` or `.fla`, and all four paks in one `.mpk`.
    Mupen64plus,
    /// Project64: the chip as `.eep`, `.sra` or `.fla`, and one `_Cont_<port>.mpk` per pak.
    Project64,
    /// An EverDrive 64: the chip as `.eep`, `.srm` or `.fla` in the console's word order, and the
    /// pak in port 1 as `.mpk`.
    Everdrive,
}

impl Target {
    fn order(self) -> WordOrder {
        match self {
            Target::Everdrive => WordOrder::Console,
            Target::Retroarch | Target::Mupen64plus | Target::Project64 => WordOrder::Swapped,
        }
    }

    /// The extension this producer gives a chip's file.
    fn chip_extension(self, save: N64Save) -> &'static str {
        match save {
            N64Save::Eeprom4k | N64Save::Eeprom16k => "eep",
            N64Save::Flash1m => "fla",
            N64Save::Sram256k | N64Save::Sram768k if self == Target::Everdrive => "srm",
            N64Save::Sram256k | N64Save::Sram768k => "sra",
        }
    }
}

/// The chips by the names `--n64-save` takes.
fn chip_names() -> String {
    N64Save::ALL.map(N64Save::as_str).join(", ")
}

fn parse_chip(text: &str) -> Result<N64Save, String> {
    N64Save::from_name(text)
        .ok_or_else(|| format!("--n64-save {text:?} is not a chip; one of {}", chip_names()))
}

fn parse_order(text: &str) -> Result<WordOrder, String> {
    match text {
        "console" => Ok(WordOrder::Console),
        "swapped" => Ok(WordOrder::Swapped),
        _ => Err(format!("--n64-order {text:?} is not an order; console or swapped")),
    }
}

/// Whether a file is one of the N64 saves this module reads rather than a flat save or a card.
///
/// A 32 KiB `.mpk` is a single pak, which the card reader already handles, so only the four-pak
/// file is taken here.
pub fn is_n64(system: Option<&str>, extension: &str, bytes: &[u8]) -> bool {
    let four_paks = cfg!(feature = "n64") && bytes.len() == 4 * 32 * 1024 && extension == "mpk";
    four_paks || (system == Some(n64::SYSTEM) && !matches!(extension, "mpk" | "pak" | "cpk"))
}

/// Reads an N64 save into a bundle, and says what it read it as.
pub fn convert(
    args: &Convert,
    bytes: &[u8],
    extension: &str,
    source: Option<Source>,
    game: Option<Game>,
    rom_save: Option<N64Save>,
) -> Result<(Bundle, String), Box<dyn std::error::Error>> {
    let core_name = source.as_ref().and_then(|s| s.core.as_ref()).map(ToString::to_string);
    let stated = args.n64_save.as_deref().map(parse_chip).transpose()?.or(rom_save);
    let options = N64Options { source, game, description: args.description.clone(), created_at: None };

    let (bundle, label) = if let Some(read) = read_whole_console(bytes, extension, stated, &options)? {
        read
    } else {
        let save = match stated {
            Some(save) => save,
            None => chip_of_file(extension, bytes)?,
        };
        let order = match args.n64_order.as_deref() {
            Some(text) => parse_order(text)?,
            // Nothing reverses an EEPROM, so its order is not a question.
            None if save.is_eeprom() => WordOrder::Console,
            None => order_of_producer(args.from.as_deref(), core_name.as_deref()).ok_or(
                "this file's SRAM or FlashRAM words could be either way round; pass --n64-order console \
                 for an EverDrive's, or swapped for mupen64plus's or Project64's (--from everdrive or \
                 --from mupen64plus says the same)",
            )?,
        };
        (n64::wrap(bytes, save, order, &options)?, format!("N64 {save} cartridge save"))
    };

    let Some(code) = args.n64_game.as_deref() else {
        return Ok((bundle, label));
    };
    if bundle.header.shape != Shape::Device {
        return Err(
            "--n64-game cuts one game out of a file holding paks, and this holds one game already".into()
        );
    }
    #[cfg(feature = "n64")]
    return Ok((n64::game_save(&bundle, code)?, format!("{code}'s save out of a {label}")));
    #[cfg(not(feature = "n64"))]
    Err(format!("--n64-game {code} needs Controller Pak support, which this build was compiled without")
        .into())
}

/// A libretro `.srm` or mupen64plus's four-pak `.mpk`, which hold a console's storage rather
/// than one chip's.
#[cfg(feature = "n64")]
fn read_whole_console(
    bytes: &[u8],
    extension: &str,
    save: Option<N64Save>,
    options: &N64Options,
) -> Result<Option<(Bundle, String)>, Box<dyn std::error::Error>> {
    if bytes.len() == n64::SRM_LEN {
        return Ok(Some((n64::read_srm(bytes, save, options)?, "libretro N64 save".into())));
    }
    if bytes.len() == n64::PAKS_LEN && extension == "mpk" {
        return Ok(Some((
            n64::read_paks(bytes, options)?,
            "mupen64plus file of four Controller Paks".into(),
        )));
    }
    Ok(None)
}

#[cfg(not(feature = "n64"))]
fn read_whole_console(
    bytes: &[u8],
    _: &str,
    _: Option<N64Save>,
    _: &N64Options,
) -> Result<Option<(Bundle, String)>, Box<dyn std::error::Error>> {
    if bytes.len() == 0x48800 {
        return Err(
            "this is a libretro N64 save, whose paks this build cannot read; it needs the n64 feature".into(),
        );
    }
    Ok(None)
}

/// The chip a file is, where its name and length settle it.
///
/// They settle it for SRAM and FlashRAM, which come in sizes nothing else does. They settle an
/// EEPROM only once something past the first 512 bytes has been written, which no 4 Kbit chip
/// reaches: Project64 writes each chip only as far as the game has, so a 16 Kbit game's file can
/// be 512 bytes long.
fn chip_of_file(extension: &str, bytes: &[u8]) -> Result<N64Save, String> {
    let hint = || format!("pass --n64-save with one of {}, or --rom with the game's ROM", chip_names());
    match extension {
        "fla" => Ok(N64Save::Flash1m),
        "sra" | "srm" if bytes.len() <= N64Save::Sram256k.size() => Ok(N64Save::Sram256k),
        "sra" | "srm" if bytes.len() == N64Save::Sram768k.size() => Ok(N64Save::Sram768k),
        "eep" if bytes.iter().skip(N64Save::Eeprom4k.size()).any(|&b| b != 0xFF) => Ok(N64Save::Eeprom16k),
        "eep" => Err(format!("this EEPROM save fits a 4 Kbit chip and a 16 Kbit one alike; {}", hint())),
        _ => Err(format!("nothing about this file says which chip the cartridge has; {}", hint())),
    }
}

/// The word order a producer keeps SRAM and FlashRAM in, where `--from` or `--core` names one.
fn order_of_producer(from: Option<&str>, core: Option<&str>) -> Option<WordOrder> {
    let swapped = ["retroarch", "mupen64plus", "mupen64plus-next", "parallel-n64", "project64"];
    match (from, core) {
        (Some("everdrive"), _) => Some(WordOrder::Console),
        (_, Some(core)) if swapped.contains(&core) => Some(WordOrder::Swapped),
        (Some(from), _) if swapped.contains(&from.to_ascii_lowercase().as_str()) => Some(WordOrder::Swapped),
        _ => None,
    }
}

/// Whether a bundle is an N64 save this module writes out, rather than a single pak's card.
pub fn is_n64_bundle(bundle: &Bundle) -> bool {
    bundle.header.card.is_none() && bundle.header.system.as_ref().is_some_and(|s| s.as_str() == n64::SYSTEM)
}

/// Writes an N64 save out as the files `target` keeps it in, named after `base`.
pub fn extract(bundle: &Bundle, target: Target, base: &Path, onto: Option<&Path>, force: bool) -> Fallible {
    let onto = onto.map(std::fs::read).transpose()?;
    let mut written = 0;

    if target == Target::Retroarch {
        #[cfg(feature = "n64")]
        {
            let file = n64::write_srm(bundle, onto.as_deref())?;
            return write(&with_extension(base, "srm"), &file, force, "libretro N64 save");
        }
        #[cfg(not(feature = "n64"))]
        return Err(
            "a libretro N64 save holds paks, which this build cannot write; it needs the n64 feature".into(),
        );
    }

    if let Some((save, normal)) = n64::cartridge(bundle)? {
        let mut file = n64::denormalize(&normal, save, target.order())?;
        // mupen64plus wrote every EEPROM as 2 KiB until 2017, and OpenEmu's build still does, so a
        // 512-byte file is one only the newer reads. Both read the padded one: the newer cuts it to
        // the chip, and rewrites it at 512 bytes the first time the game saves.
        if target == Target::Mupen64plus && save.is_eeprom() {
            file.resize(N64Save::Eeprom16k.size(), 0xFF);
        }
        let path = with_extension(base, target.chip_extension(save));
        write(&path, &file, force, &format!("{save} cartridge save"))?;
        written += 1;
    }
    written += extract_paks(bundle, target, base, onto.as_deref(), force)?;

    if written == 0 {
        return Err("this bundle holds nothing to write".into());
    }
    Ok(())
}

#[cfg(feature = "n64")]
fn extract_paks(
    bundle: &Bundle,
    target: Target,
    base: &Path,
    onto: Option<&[u8]>,
    force: bool,
) -> Result<usize, Box<dyn std::error::Error>> {
    let ports = n64::pak_ports(bundle)?;
    if ports.is_empty() {
        if onto.is_some() {
            eprintln!("note: --onto was given, but this bundle holds no Controller Pak to write onto it");
        }
        return Ok(0);
    }
    match target {
        Target::Mupen64plus => {
            let file = n64::write_paks(bundle, onto)?;
            write(&with_extension(base, "mpk"), &file, force, "file of four Controller Paks")?;
            Ok(1)
        }
        Target::Project64 => {
            // One file per pak, so a file to write onto only makes sense when there is one pak.
            if onto.is_some() && ports.len() > 1 {
                return Err(
                    "--onto names one pak, and this bundle holds several; extract them one at a time".into(),
                );
            }
            for &port in &ports {
                let pak = n64::write_pak(bundle, port, onto)?.expect("a port the bundle names");
                let name =
                    format!("{}_Cont_{port}.mpk", base.file_name().unwrap_or_default().to_string_lossy());
                write(&base.with_file_name(name), &pak, force, &format!("Controller Pak in port {port}"))?;
            }
            Ok(ports.len())
        }
        Target::Everdrive => {
            for &port in ports.iter().filter(|&&port| port != 1) {
                eprintln!("note: an EverDrive keeps one pak, so the pak in port {port} is left out");
            }
            let Some(pak) = n64::write_pak(bundle, 1, onto)? else {
                return Ok(0);
            };
            write(&with_extension(base, "mpk"), &pak, force, "Controller Pak")?;
            Ok(1)
        }
        Target::Retroarch => unreachable!("written as one file before the paks are reached"),
    }
}

#[cfg(not(feature = "n64"))]
fn extract_paks(
    bundle: &Bundle,
    _: Target,
    _: &Path,
    _: Option<&[u8]>,
    _: bool,
) -> Result<usize, Box<dyn std::error::Error>> {
    let paks = bundle
        .parts
        .iter()
        .any(|part| part.role.as_ref().is_some_and(|r| r.as_str().starts_with("controller-pak-")));
    if paks {
        return Err(
            "this bundle holds Controller Paks, which this build cannot write; it needs the n64 feature"
                .into(),
        );
    }
    Ok(0)
}

/// `base` with `extension` on the end, keeping any dots already in the name.
///
/// [`Path::with_extension`] would take `Mario Kart 64 (USA)` apart at nothing, but `Rev. 1` at its
/// dot, so the extension is appended rather than substituted.
fn with_extension(base: &Path, extension: &str) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

fn write(path: &Path, bytes: &[u8], force: bool, what: &str) -> Fallible {
    write_out(path, bytes, force)?;
    println!("-> {} ({what}, {} bytes)", path.display(), bytes.len());
    Ok(())
}
