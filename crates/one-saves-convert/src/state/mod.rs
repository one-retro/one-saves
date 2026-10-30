//! Save states: what an emulator took of a running game, as a [`state`](one_saves::Shape::State)
//! bundle, and back.
//!
//! A state is not a save. The game wrote a save, and anything that runs the game reads it; an
//! emulator took a state, the whole machine at one moment, and only the core that took it loads
//! it. So a state bundle names that core in its `source`, and nothing here hands a state's data to
//! a file for any other.
//!
//! An emulator's state file is a container: a header, the core's data, a screenshot, and sometimes
//! more. A reader moves what the bundle's header has a field for into the header, what an extension
//! key covers into that key, and carries the rest as parts:
//!
//! | Emulator | Its files | In the bundle |
//! | -------- | --------- | ------------- |
//! | RetroArch | `X.state`, `X.state1`…, `X.state.auto`: a `RASTATE` wrapper, usually RZIP-compressed, and a `.png` thumbnail beside it | the `MEM` block as the state part, the thumbnail as the screenshot, every other block an `aux` part named for its tag |
//! | mGBA | `X.ss1`…`X.ss9`, and `X.ss0` to resume from: a PNG whose chunks carry the state, or the state with its extra data after it | the state part with the cheats on it, the image as the screenshot, the in-game save as a save part, `taken_at` and `source` |
//! | DuckStation | `SERIAL_N.sav`, `SERIAL_resume.sav`, and `SERIAL_N.bak` for what a slot held before | `format_version`, the title and serial in `game`, the screenshot as a PNG, and the core's data uncompressed |
//!
//! The state part is always the core's data and nothing around it, uncompressed, so a consumer
//! rebuilding a file knows what it has. A writer rebuilds the emulator's file from the bundle. It
//! need not come back byte for byte, only load.
//!
//! What the file does not say is the caller's to give in [`StateOptions`]: the core RetroArch ran,
//! the ROM the state was taken against, and when it was taken where the emulator kept no record,
//! which is ordinarily the file's modification time.
//!
//! Everything but [`detect`] and the types needs the `states` feature. Without it a state is still
//! recognised, so it is refused as what it is rather than wrapped as a save it is not.

use one_saves::{Bundle, Game, Slug, Source, TakenWith};

use crate::error::{Error, Result};

#[cfg(feature = "states")]
pub mod duckstation;
#[cfg(feature = "states")]
mod image;
#[cfg(feature = "states")]
pub mod mgba;
#[cfg(feature = "states")]
pub mod retroarch;

/// An emulator whose save states this crate reads and writes.
///
/// Every variant is here whichever features are on, so a downstream `match` compiles the same way
/// in every build; what `states` takes away is the reader and the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emulator {
    /// RetroArch, whatever core it ran.
    RetroArch,
    /// Standalone mGBA.
    Mgba,
    /// DuckStation.
    DuckStation,
}

impl Emulator {
    /// Every emulator, in the order they are listed.
    pub const ALL: [Emulator; 3] = [Emulator::RetroArch, Emulator::Mgba, Emulator::DuckStation];

    /// What to call it in a message.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Emulator::RetroArch => "RetroArch state",
            Emulator::Mgba => "mGBA state",
            Emulator::DuckStation => "DuckStation state",
        }
    }

    /// Its slug in the Apps registry, which is also how a caller names it.
    #[must_use]
    pub const fn app(self) -> &'static str {
        match self {
            Emulator::RetroArch => "retroarch",
            Emulator::Mgba => "mgba",
            Emulator::DuckStation => "duckstation",
        }
    }

    /// The emulator an app slug names, if it is one of these.
    #[must_use]
    pub fn from_app(app: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|emulator| emulator.app() == app)
    }

    /// Whether **this build** reads and writes its states.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        cfg!(feature = "states")
    }
}

/// Works out whether a file is a save state, and whose, from its bytes and its name.
///
/// The bytes come first. A RetroArch state opens with RZIP's magic or with `RASTATE`, an mGBA one
/// is a PNG carrying its `gbAs` chunk, and a DuckStation one opens with `DUCC`. Two kinds carry
/// nothing to know them by: a state an older RetroArch wrote bare, which is the core's data alone,
/// and an mGBA one written without a screenshot, which opens with the core's own version word. Those
/// are recognised by their names, `.state` and `.ss1`, and by nothing else.
#[must_use]
pub fn detect(bytes: &[u8], name: &str) -> Option<Emulator> {
    if bytes.starts_with(b"#RZIPv") || bytes.starts_with(b"RASTATE") {
        return Some(Emulator::RetroArch);
    }
    if bytes.starts_with(b"DUCC") {
        return Some(Emulator::DuckStation);
    }
    if is_mgba_png(bytes) {
        return Some(Emulator::Mgba);
    }
    let name = name.to_ascii_lowercase();
    if retroarch_slot(&name).is_some() {
        return Some(Emulator::RetroArch);
    }
    if mgba_slot(&name).is_some() && !bytes.starts_with(PNG_SIGNATURE) {
        return Some(Emulator::Mgba);
    }
    None
}

/// The eight bytes every PNG opens with.
pub(crate) const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Whether a PNG carries mGBA's state chunk, which is what makes it a state rather than a picture.
///
/// Walks the chunk headers without reading what is in them, so a PNG that is only a screenshot
/// costs a pass over its lengths.
fn is_mgba_png(bytes: &[u8]) -> bool {
    let Some(mut rest) = bytes.strip_prefix(PNG_SIGNATURE) else { return false };
    while rest.len() >= 12 {
        let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        if &rest[4..8] == b"gbAs" {
            return true;
        }
        let Some(next) = length.checked_add(12).and_then(|end| rest.get(end..)) else { return false };
        rest = next;
    }
    false
}

/// Where a RetroArch state sits: a numbered slot, or the one it takes to resume from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// A numbered slot, as the player picks one.
    Numbered(u64),
    /// The state the emulator takes on its own to resume from.
    Auto,
}

/// The slot a RetroArch state's name says: `X.state` is slot 0, `X.state3` slot 3, and
/// `X.state.auto` the one it takes on its own.
pub(crate) fn retroarch_slot(name: &str) -> Option<Slot> {
    if name.ends_with(".state.auto") {
        return Some(Slot::Auto);
    }
    let (_, tail) = name.rsplit_once(".state")?;
    if tail.is_empty() {
        return Some(Slot::Numbered(0));
    }
    tail.bytes().all(|byte| byte.is_ascii_digit()).then(|| tail.parse().ok().map(Slot::Numbered)).flatten()
}

/// The slot an mGBA state's name says: `X.ss1` to `X.ss9`, and `X.ss0`, which mGBA keeps to resume
/// from.
pub(crate) fn mgba_slot(name: &str) -> Option<Slot> {
    let (_, tail) = name.rsplit_once(".ss")?;
    match tail.parse::<u64>() {
        Ok(0) if tail.len() == 1 => Some(Slot::Auto),
        Ok(slot) if !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(Slot::Numbered(slot))
        }
        _ => None,
    }
}

/// What a state's header says beyond what the emulator's file does.
///
/// A reader fills in what the file records — mGBA names itself and keeps when it took the state,
/// DuckStation keeps the game's title and serial — and takes the rest from here. Where both say
/// something, the file wins: it is the emulator's own record of the state.
#[derive(Debug, Clone, Default)]
pub struct StateOptions {
    /// What the state loads into: the core that took it, or the app where nothing runs one.
    ///
    /// Required by the format. mGBA and DuckStation name themselves, so for them this only adds
    /// to it, a platform or a version. RetroArch's file does not say which core it ran, so a
    /// RetroArch state needs `core` here.
    pub source: Option<Source>,
    /// The system the game runs on.
    pub system: Option<Slug>,
    /// The game, and the ROM the state was taken against. A state loads only against that ROM, so
    /// its hashes in `rom_hashes` are what finds it on the other side.
    pub game: Option<Game>,
    /// A note, or the name the player gave the state.
    pub description: Option<String>,
    /// When the bundle was assembled.
    pub bundled_at: Option<i64>,
    /// When the state was taken, where the file keeps no record of its own: ordinarily the state
    /// file's modification time.
    pub taken_at: Option<i64>,
    /// What the state was taken with, beyond what the file records.
    pub taken_with: Option<TakenWith>,
}

impl StateOptions {
    /// Options carrying this source.
    #[must_use]
    pub fn source(mut self, source: impl Into<Option<Source>>) -> Self {
        self.source = source.into();
        self
    }

    /// Options carrying this system.
    #[must_use]
    pub fn system(mut self, system: impl Into<Option<Slug>>) -> Self {
        self.system = system.into();
        self
    }

    /// Options carrying this game.
    #[must_use]
    pub fn game(mut self, game: impl Into<Option<Game>>) -> Self {
        self.game = game.into();
        self
    }

    /// Options saying when the state was taken.
    #[must_use]
    pub fn taken_at(mut self, seconds: impl Into<Option<i64>>) -> Self {
        self.taken_at = seconds.into();
        self
    }
}

/// A state file as an emulator left it.
#[derive(Debug, Clone, Copy)]
pub struct StateFile<'a> {
    /// The file's bytes.
    pub bytes: &'a [u8],
    /// Its name, which is where every emulator here records the slot.
    pub name: &'a str,
    /// A screenshot the emulator keeps beside the file rather than in it, which is RetroArch's
    /// `.png` thumbnail.
    pub screenshot: Option<&'a [u8]>,
}

/// A state file rebuilt for its emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuilt {
    /// What to call it: the slot is in the name, so this is the name the emulator looks for.
    pub name: String,
    /// The file.
    pub bytes: Vec<u8>,
    /// A screenshot the emulator keeps beside the file, to be written as `name` and `.png`.
    pub screenshot: Option<Vec<u8>>,
    /// The parts the emulator's file has no place for, by id.
    ///
    /// The file still loads without them: they are what the bundle carried beside the core's
    /// data, such as an in-game save RetroArch keeps in a file of its own, or a JPEG screenshot
    /// where it keeps a PNG.
    pub left_out: Vec<u64>,
}

/// Reads a state file into a state bundle.
///
/// # Errors
///
/// [`Error::Unsupported`] in a build without `states`; otherwise whatever the emulator's reader
/// finds wrong with the file.
pub fn read(emulator: Emulator, file: &StateFile<'_>, options: &StateOptions) -> Result<Bundle> {
    #[cfg(feature = "states")]
    {
        match emulator {
            Emulator::RetroArch => retroarch::read(file, options),
            Emulator::Mgba => mgba::read(file, options),
            Emulator::DuckStation => duckstation::read(file, options),
        }
    }
    #[cfg(not(feature = "states"))]
    {
        let _ = (file, options);
        Err(Error::Unsupported { format: emulator.label(), feature: "states" })
    }
}

/// Rebuilds an emulator's state file from a state bundle.
///
/// `stem` is what the file is named after, ordinarily the game's ROM without its extension; the
/// slot goes after it. DuckStation names its states after the game's serial instead, and takes
/// `stem` only for a game with none. Either is reduced to one file-name component first, so
/// [`Rebuilt::name`] never names a directory, however the bundle spells its serial.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, or that loads into a core this
/// emulator does not run; [`Error::Unsupported`] in a build without `states`.
pub fn write(emulator: Emulator, bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    #[cfg(feature = "states")]
    {
        match emulator {
            Emulator::RetroArch => retroarch::write(bundle, stem),
            Emulator::Mgba => mgba::write(bundle, stem),
            Emulator::DuckStation => duckstation::write(bundle, stem),
        }
    }
    #[cfg(not(feature = "states"))]
    {
        let _ = (bundle, stem);
        Err(Error::Unsupported { format: emulator.label(), feature: "states" })
    }
}

/// Text made safe to name a file by: one path component, never a directory, never `.` or `..`.
///
/// A serial or a ROM's name comes out of the bundle, which is anyone's to write, so a separator or
/// a control character in one becomes `_` rather than a path the caller then writes to.
#[cfg_attr(not(feature = "states"), allow(dead_code))]
pub(crate) fn component(text: &str) -> String {
    let safe: String =
        text.chars().map(|c| if matches!(c, '/' | '\\' | ':') || c.is_control() { '_' } else { c }).collect();
    if safe.is_empty() || safe.chars().all(|c| c == '.') { "state".to_owned() } else { safe }
}

/// The emulator a state bundle's `source` says it was taken in, where it is one of these.
///
/// The app decides it, since the file is the app's. A bundle naming only a core is mGBA's or
/// DuckStation's when the core is, since each runs only its own; a bare core otherwise says nothing
/// about which app to rebuild a file for.
#[must_use]
pub fn emulator_of(bundle: &Bundle) -> Option<Emulator> {
    let source = bundle.header.source.as_ref()?;
    match (&source.app, &source.core) {
        (Some(app), _) => Emulator::from_app(app.as_str()),
        (None, Some(core)) => {
            Emulator::from_app(core.as_str()).filter(|emulator| *emulator != Emulator::RetroArch)
        }
        (None, None) => None,
    }
}

/// The state part and everything else a state bundle holds, sorted for a writer.
#[cfg(feature = "states")]
pub(crate) struct Contents<'a> {
    pub state: &'a one_saves::Part,
    pub screenshot: Option<&'a one_saves::Part>,
    pub saves: Vec<&'a one_saves::Part>,
    pub aux: Vec<&'a one_saves::Part>,
}

#[cfg(feature = "states")]
impl<'a> Contents<'a> {
    /// Takes a bundle apart, refusing one that is not a state.
    ///
    /// The bundle has already been validated wherever it came from, so a state holds exactly one
    /// state part; this does not re-check what [`Bundle::validate`] does.
    pub fn of(bundle: &'a Bundle) -> Result<Self> {
        use one_saves::{PartKind, Shape};

        if bundle.header.shape != Shape::State {
            return Err(Error::NotConvertible(format!(
                "this is a {} bundle, and only a state rebuilds an emulator's state file",
                bundle.header.shape
            )));
        }
        let mut state = None;
        let mut contents =
            Contents { state: &bundle.parts[0], screenshot: None, saves: Vec::new(), aux: Vec::new() };
        for part in &bundle.parts {
            match part.kind {
                PartKind::State => state = Some(part),
                PartKind::Screenshot => contents.screenshot = Some(part),
                PartKind::Save => contents.saves.push(part),
                _ => contents.aux.push(part),
            }
        }
        contents.state =
            state.ok_or_else(|| Error::NotConvertible("this state holds no `state` part".into()))?;
        Ok(contents)
    }
}

/// Refuses a bundle that loads into another core than the one an emulator runs.
///
/// A consumer must not hand a state's data to any core but the one `source` names, and rebuilding
/// an emulator's file is exactly that.
#[cfg(feature = "states")]
pub(crate) fn require_core(bundle: &Bundle, core: &str, emulator: &str) -> Result<()> {
    let source = bundle.header.source.as_ref();
    let named = source.and_then(|source| source.core.as_ref()).map(one_saves::Name::as_str);
    let app = source.and_then(|source| source.app.as_ref()).map(one_saves::Name::as_str);
    match (named, app) {
        (Some(named), _) if named == core => Ok(()),
        // Where nothing names a core, the app is what the state loads into.
        (None, Some(app)) if app == core => Ok(()),
        (Some(other), _) | (None, Some(other)) => Err(Error::NotConvertible(format!(
            "this state loads into {other}, and {emulator} runs {core}; a state loads only into the \
             core that took it"
        ))),
        (None, None) => Err(Error::NotConvertible("this state names nothing it loads into".into())),
    }
}

/// Puts a reader's findings and the caller's options together into a state's header.
#[cfg(feature = "states")]
pub(crate) fn header(options: &StateOptions, found: Found) -> one_saves::Header {
    use one_saves::{Header, Shape};

    // The file is the emulator's own record, so what it says wins; the options fill the gaps.
    let mut source = options.source.clone().unwrap_or_default();
    if let Some(app) = found.app {
        source.app = Some(one_saves::Name::parse(app).expect("an app slug is a name"));
    }
    if let Some(core) = found.core {
        source.core = Some(one_saves::Name::parse(core).expect("a core slug is a name"));
    }
    if found.app_version.is_some() {
        source.app_version = found.app_version;
    }
    if found.core_version.is_some() {
        source.core_version = found.core_version;
    }
    if source.device_kind.is_none() {
        source.device_kind = Slug::parse("emulator").ok();
    }

    let game = match (options.game.clone(), found.game) {
        (Some(mut game), Some(found)) => {
            game.title = game.title.or(found.title);
            game.serial = game.serial.or(found.serial);
            game.rom_filename = game.rom_filename.or(found.rom_filename);
            game.system = game.system.or(found.system);
            // A ROM the caller hashed has every digest; the file's are only where it gave none.
            if game.rom_hashes.is_empty() {
                game.rom_hashes = found.rom_hashes;
            }
            Some(game)
        }
        (game, found) => game.or(found),
    };

    let taken_with = match (options.taken_with.clone(), found.taken_with) {
        (Some(mut given), Some(found)) => {
            given.firmware = given.firmware.or(found.firmware);
            given.format_version = found.format_version.or(given.format_version);
            Some(given)
        }
        (given, found) => given.or(found),
    };

    let mut header = Header::new(Shape::State)
        .system(options.system.clone().or(found.system))
        .game(game)
        .source(source)
        .bundled_at(options.bundled_at)
        .taken_at(found.taken_at.or(options.taken_at))
        .taken_with(taken_with)
        .origin(found.origin)
        .extensions(found.extensions);
    header.description.clone_from(&options.description);
    header
}

/// What a reader found in an emulator's file that belongs in the header.
#[cfg(feature = "states")]
#[derive(Default)]
pub(crate) struct Found {
    pub app: Option<&'static str>,
    pub core: Option<&'static str>,
    pub app_version: Option<String>,
    pub core_version: Option<String>,
    pub system: Option<Slug>,
    pub game: Option<Game>,
    pub taken_at: Option<i64>,
    pub taken_with: Option<TakenWith>,
    pub origin: Option<one_saves::Origin>,
    /// The header's keys, such as the clock a save the emulator kept beside the data carried.
    pub extensions: one_saves::Extensions,
}

/// The origin and the slot a name's slot says.
#[cfg(feature = "states")]
pub(crate) fn origin_and_slot(slot: Option<Slot>) -> (Option<one_saves::Origin>, Option<u64>) {
    match slot {
        Some(Slot::Numbered(slot)) => (None, Some(slot)),
        Some(Slot::Auto) => (Some(one_saves::Origin::Auto), None),
        None => (None, None),
    }
}

/// The slot to name a rebuilt file by, from the state part's `slot` and the bundle's `origin`.
#[cfg(feature = "states")]
pub(crate) fn slot_of(bundle: &Bundle, state: &one_saves::Part) -> Option<Slot> {
    match (&bundle.header.origin, state.slot) {
        (Some(one_saves::Origin::Auto), _) => Some(Slot::Auto),
        (_, Some(slot)) => Some(Slot::Numbered(slot)),
        (_, None) => None,
    }
}

/// Numbers a state's parts in the order a reader found them: the state, its screenshot, then the
/// rest.
#[cfg(feature = "states")]
pub(crate) fn number(parts: Vec<one_saves::Part>) -> Vec<one_saves::Part> {
    parts
        .into_iter()
        .enumerate()
        .map(|(id, mut part)| {
            part.id = id as u64;
            part
        })
        .collect()
}

/// A state part over the core's data, in the slot the emulator gave it.
#[cfg(feature = "states")]
pub(crate) fn state_part(data: Vec<u8>, slot: Option<u64>) -> one_saves::Part {
    one_saves::Part::new(0, data).kind(one_saves::PartKind::State).slot(slot)
}

/// A screenshot part over a PNG or a JPEG, kept as the emulator wrote it.
#[cfg(feature = "states")]
pub(crate) fn screenshot_part(image: Vec<u8>) -> Result<one_saves::Part> {
    let content_type = crate::screenshot::image_type(&image).ok_or_else(|| Error::NotThisFormat {
        format: "screenshot",
        why: "it is neither a PNG nor a JPEG".into(),
    })?;
    Ok(one_saves::Part::new(0, image).kind(one_saves::PartKind::Screenshot).content_type(content_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_from_the_bundle_never_leaves_its_directory() {
        assert_eq!(component("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(component("C:\\Windows"), "C__Windows");
        assert_eq!(component(".."), "state");
        assert_eq!(component(""), "state");
        assert_eq!(component("Sonic CD (USA)"), "Sonic CD (USA)");
    }

    #[test]
    fn a_retroarch_name_says_its_slot() {
        assert_eq!(retroarch_slot("sonic cd (usa).state"), Some(Slot::Numbered(0)));
        assert_eq!(retroarch_slot("sonic cd (usa).state12"), Some(Slot::Numbered(12)));
        assert_eq!(retroarch_slot("sonic cd (usa).state.auto"), Some(Slot::Auto));
        assert_eq!(retroarch_slot("sonic cd (usa).state.png"), None, "the thumbnail beside one");
        assert_eq!(retroarch_slot("sonic cd (usa).srm"), None);
    }

    #[test]
    fn an_mgba_name_says_its_slot() {
        assert_eq!(mgba_slot("emerald.ss1"), Some(Slot::Numbered(1)));
        assert_eq!(mgba_slot("emerald.ss0"), Some(Slot::Auto), "the one mGBA resumes from");
        assert_eq!(mgba_slot("emerald.ss"), None);
        assert_eq!(mgba_slot("emerald.ssx"), None);
    }

    #[test]
    fn a_state_is_known_by_its_bytes_before_its_name() {
        assert_eq!(detect(b"#RZIPv\x01#....", "x.bin"), Some(Emulator::RetroArch));
        assert_eq!(detect(b"RASTATE\x01MEM ", "x.bin"), Some(Emulator::RetroArch));
        assert_eq!(detect(b"DUCC\x5d\0\0\0", "SLUS-00594_1.sav"), Some(Emulator::DuckStation));
        assert_eq!(
            detect(b"\0\0\0\x01", "emerald.ss1"),
            Some(Emulator::Mgba),
            "a bare mGBA state, by its name"
        );
        assert_eq!(
            detect(b"\0\0\0\0", "game.state3"),
            Some(Emulator::RetroArch),
            "a bare core state, by its name"
        );
        assert_eq!(detect(b"\0\0\0\0", "game.sav"), None, "a save is not a state");
        // A PNG that is only a picture is a screenshot, whatever it is called.
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\0\0\0\0IEND\xaeB`\x82", "game.ss1"), None);
    }
}
