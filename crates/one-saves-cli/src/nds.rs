//! Nintendo DS saves: the chip in whatever file it arrived in, and the save a DeSmuME state holds.
//!
//! A DS save names no chip, so its length has to be the chip's, and a file does not always say
//! what that is. `--rom` and `--nds-game` say it from the game; otherwise the file has to, and one
//! that cannot is carried as it came, as an `aux` part, with a note saying why.

use std::path::Path;

use one_saves::{Bundle, Game, Source};
use one_saves_convert::layout::SaveMemory;
use one_saves_convert::nds::{self, NdsOptions};

use crate::{Convert, Fallible};

/// The extensions only a DS save takes: DeSmuME's `.dsv`, and the Action Replay DS's `.duc` and
/// `.dss`. The libretro cores' `.srm` and melonDS's `.sav` say nothing on their own.
pub(crate) fn system_of_extension(extension: &str) -> Option<&'static str> {
    matches!(extension, "dsv" | "duc" | "dss").then_some(nds::SYSTEM)
}

/// The system `--nds-game` says the save is for, which only a DS save's game code can be.
pub(crate) fn named_by_game(args: &Convert) -> Option<&'static str> {
    #[cfg(feature = "rom")]
    if args.nds_game.is_some() {
        return Some(nds::SYSTEM);
    }
    #[cfg(not(feature = "rom"))]
    let _ = args;
    None
}

/// Whether a file is a flat save without detecting it: a DS tool's own file, known by its signature
/// whatever it is called, one under an extension only a DS save takes, or one `--system nds` names.
pub(crate) fn is_flat(args: &Convert, bytes: &[u8], extension: &str) -> bool {
    nds::is_container(bytes)
        || system_of_extension(extension).is_some()
        || args.system.as_deref() == Some(nds::SYSTEM)
        || named_by_game(args).is_some()
}

/// Refuses `--in-game-save` for a file that is not a state, which is every file it reaches here.
#[cfg_attr(not(feature = "states"), allow(clippy::unnecessary_wraps))]
pub(crate) fn refuse_in_game_save(args: &Convert) -> Result<(), String> {
    #[cfg(feature = "states")]
    if args.in_game_save {
        return Err("--in-game-save reads the save out of a DeSmuME state, and this file is not one".into());
    }
    #[cfg(not(feature = "states"))]
    let _ = args;
    Ok(())
}

/// Whether a file is read as a DS save: one the caller or the ROM says is, or one in a file of a
/// DS tool's own making.
pub(crate) fn is_nds(system: Option<&str>, bytes: &[u8]) -> bool {
    match system {
        Some(system) => system == nds::SYSTEM,
        None => nds::is_container(bytes),
    }
}

/// How long the card's save memory is, from `--nds-game` or the ROM: `None` where neither says.
#[cfg_attr(not(feature = "rom"), allow(clippy::unnecessary_wraps))]
fn chip_of(args: &Convert, memory: Option<SaveMemory>) -> Result<Option<usize>, String> {
    #[cfg(feature = "rom")]
    if let Some(code) = args.nds_game.as_deref() {
        return match one_saves_convert::rom::nds_save_length(code) {
            Some(Some(length)) => Ok(Some(length)),
            Some(None) => Err(format!("the game {code} keeps no save on its card")),
            None => Err(format!("no DS game is known by the code {code:?}; pass --rom instead")),
        };
    }
    #[cfg(not(feature = "rom"))]
    let _ = args;
    Ok(memory.and_then(SaveMemory::size))
}

/// The options a DS save is read with.
fn options_of(
    args: &Convert,
    source: Option<Source>,
    game: Option<Game>,
    memory: Option<SaveMemory>,
) -> Result<NdsOptions, String> {
    // A game named by its code alone is still named.
    #[cfg(feature = "rom")]
    let game = game.or_else(|| args.nds_game.as_deref().map(|code| Game::default().serial(code)));
    Ok(NdsOptions {
        source,
        game,
        description: args.description.clone(),
        chip: chip_of(args, memory)?,
        file_name: file_name(&args.input),
        ..NdsOptions::default()
    })
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name().map(|name| name.to_string_lossy().into_owned())
}

/// Reads a DS save into a bundle, and says what it was read as.
fn convert(
    args: &Convert,
    bytes: &[u8],
    source: Option<Source>,
    game: Option<Game>,
    memory: Option<SaveMemory>,
) -> Result<(Bundle, String), Box<dyn std::error::Error>> {
    if args.rtc_file.is_some() {
        return Err("a DS card keeps no clock, so there is no clock file to read with it".into());
    }
    let read = nds::read(bytes, &options_of(args, source, game, memory)?)?;
    Ok((read.bundle.clone(), described(&read)))
}

/// Reads a DS save and writes its bundle where the input says.
///
/// A DS card keeps no clock. SkyEmu writes the console's beside the save, which is no part of it,
/// so a `.rtc` there is passed over and said to be.
pub(crate) fn convert_and_write(
    args: &Convert,
    bytes: &[u8],
    source: Option<Source>,
    game: Option<Game>,
    memory: Option<SaveMemory>,
) -> Fallible {
    if args.role.is_some() {
        return Err("a DS card's save has one socket, the card, so --role has nothing to name".into());
    }
    let (bundle, label) = convert(args, bytes, source, game, memory)?;
    if args.input.with_extension("rtc").is_file() && args.rtc_file.is_none() {
        println!("note: the .rtc beside it is the console's clock, which a DS save does not carry");
    }
    crate::write_converted(args, &bundle, &label)
}

/// Reads the in-game save a DeSmuME state holds into a save bundle of its own.
#[cfg(feature = "states")]
pub(crate) fn lift_from_state(
    args: &Convert,
    bytes: &[u8],
    source: Option<Source>,
    game: Option<Game>,
    memory: Option<SaveMemory>,
) -> Result<(Bundle, String), Box<dyn std::error::Error>> {
    let opened = one_saves_convert::state::desmume::in_game_save(bytes)?;
    let read = nds::read_opened(bytes, &opened, &options_of(args, source, game, memory)?)?;
    Ok((read.bundle.clone(), described(&read)))
}

/// What a read came to, for the line `convert` prints, after a note where the file went as `aux`.
fn described(read: &nds::Read) -> String {
    if let Some(why) = &read.why_aux {
        eprintln!(
            "note: kept the file as it came, as an aux part rather than the card's save, because {why}"
        );
    }
    match read.chip {
        Some(chip) => format!("{}, {} bytes of save chip", read.container.label(), chip),
        None => format!("{}, chip not known", read.container.label()),
    }
}
