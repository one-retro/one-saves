//! PICO-8 carts: the ids a cart keeps its saves under.
//!
//! A PICO-8 save is not named for the cart. A cart calls `cartdata("id")`, and every runtime that
//! keeps saves files them under that id: official PICO-8, fake-08 and picante8 as `<id>.p8d.txt`
//! in a folder every cart shares, the P8 core for the MiSTer as a 512-byte block per id in the
//! cart's own `.sav`. So until a cart has saved, nothing beside it says which saves are its, and a
//! save manager holding the save under its id has nowhere to put it. The cart says: this reads the
//! code out of a cart and finds the ids it asks for.
//!
//! - **A string passed as it is**: `cartdata("porklike")`, `cartdata"porklike"`.
//! - **A variable or a `..` chain**, followed to what the code assigns the variable:
//!   `cartdata(id)` after `id = "mot_gpp"`. An assignment is read as far as a string, a number or
//!   another such variable joined by `..` goes, so `v = v + 1` reads as `v = v`, and one that does
//!   not start that way, `id = stat(6)`, is passed over rather than held against the variable.
//!   What is read is kept only when every value is an id, so a variable that holds text besides
//!   gives none.
//!
//! A small Lua scanner skips comments and strings, so a cart that only mentions `cartdata` in a
//! comment asks for nothing. A call whose argument is computed some other way (a table lookup,
//! `stat()`) is not followed. Those are the rules the 1Retro catalogue's harvest of the Lexaloffle
//! BBS found its ids by, the passing over included, so a cart read here and the catalogue agree on
//! what the cart is.
//!
//! # The three kinds of cart
//!
//! - **`.p8`**: text, the code between `__lua__` and the next section.
//! - **`.p8.png`**: a picture with the cart in the two low bits of each channel of each pixel
//!   (alpha, red, green, blue, most significant first). The first `0x8000` bytes are the cart, and
//!   its code is at `0x4300`, as text, or compressed in the old (`:c:\0`) or the new (`\0pxa`)
//!   format. Read with the `pico8-png` feature, which brings the PNG decoder; without it such a
//!   cart answers [`Error::Unsupported`].
//! - **`.p8c`**: a program the P8 core's compiler built, which keeps no source but records the ids
//!   it found when it compiled each cart, its own and those of the carts it bundles for `load()`.
//!   Read for module format `P8C7`; a program in a format this does not know answers
//!   [`Error::Program`] rather than a guess.

use std::borrow::Cow;
use std::collections::BTreeSet;

#[cfg(feature = "pico8-png")]
mod image;

/// Without `pico8-png` a cart in a picture is named as one and refused, rather than read.
#[cfg(not(feature = "pico8-png"))]
mod image {
    use super::Error;

    pub(super) fn code(_png_bytes: &[u8]) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported)
    }
}

/// The most values a variable is followed through before it is given up on.
const MAX_CANDIDATES: usize = 16;

/// How deep a variable is followed through the variables it is built from.
const MAX_DEPTH: usize = 3;

/// The longest id PICO-8 saves under.
const MAX_ID_LEN: usize = 64;

/// The most ids a cart is read for. PICO-8 lets a cart open 4 in a session and the P8 core keeps
/// 16 blocks, so a cart naming more is one built to cost its reader: what is past this is not read.
const MAX_IDS: usize = 64;

/// The most ids a compiled module may claim to record before it is taken for a malformed one.
const MAX_MODULE_IDS: usize = 1024;

/// The longest code a text cart is read for. PICO-8's own limit is 65,535 characters; this is room
/// for a cart well past that without reading a file of any size.
const MAX_CODE: usize = 1 << 20;

/// How many tokens a cart's calls and variables are followed through, in all, before the rest is
/// given up on. Following a variable scans the code for its assignments, and a value built from
/// other variables scans again for each, so without a limit a cart could be written to make that
/// arbitrarily slow. Real carts spend a tiny fraction of this.
const WORK: usize = 4_000_000;

/// What is left of [`WORK`] for one cart.
struct Budget(std::cell::Cell<usize>);

impl Budget {
    fn new() -> Self {
        Budget(std::cell::Cell::new(WORK))
    }

    /// Spend `n` steps; `false` once there are not that many left, and from then on.
    fn spend(&self, n: usize) -> bool {
        if let Some(left) = self.0.get().checked_sub(n) {
            self.0.set(left);
            true
        } else {
            self.0.set(0);
            false
        }
    }
}

/// What a cart's code says about its saves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cart {
    /// The ids it asks `cartdata()` for, each once, in the order the code first names them.
    pub ids: Vec<String>,
    /// The carts it names to `load()` as a string, as it names them: `#mot_airpicogame`,
    /// `solitomb_title`, `level2.p8`. A name built while the cart runs is not followed. A program
    /// the P8 compiler built names none: the carts it loads are inside it, and their ids are in
    /// [`ids`](Self::ids).
    pub loads: Vec<String>,
}

/// Why a cart could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an image that holds a cart.
    Image(String),
    /// The cart's code stops partway, or points at something before its start.
    Code,
    /// Not a program the P8 compiler built, or one in a module format this does not read.
    Program(&'static str),
    /// None of the three kinds of cart.
    NotACart,
    /// A cart in a picture, `.p8.png`, which this build was compiled without the `pico8-png`
    /// feature to read.
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Image(why) => write!(f, "not a PICO-8 cart image: {why}"),
            Error::Code => f.write_str("the cart's code is cut short or malformed"),
            Error::Program(why) => write!(f, "not a P8 program this reads: {why}"),
            Error::NotACart => f.write_str("not a PICO-8 cart"),
            Error::Unsupported => f.write_str(
                "this is a PICO-8 cart in a picture, and this build has the `pico8-png` feature off, \
                 so it can name the cart and not read it",
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Whether `id` is one PICO-8 saves under: 1 to 64 of `a`-`z`, `0`-`9`, `_` and `-`.
///
/// The manual lists the first three; the changelog added `-` in v0.1.12. fake-08 aborts a cart that
/// asks for any other, and official PICO-8 does not save it.
#[must_use]
pub fn is_cartdata_id(id: &str) -> bool {
    (1..=MAX_ID_LEN).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Whether `file_name` names a cart: a `.p8`, a `.p8.png` or a `.p8c`. Only a picture named
/// `.p8.png` is one; any other `.png` is a picture.
#[must_use]
pub fn is_cart_name(file_name: &str) -> bool {
    ends_in(file_name, b".p8") || ends_in(file_name, b".p8.png") || ends_in(file_name, b".p8c")
}

/// Whether this build reads the cart named `file_name`: a `.p8` or a `.p8c` always, and a
/// `.p8.png` with the `pico8-png` feature.
pub(crate) fn reads_cart_named(file_name: &str) -> bool {
    ends_in(file_name, b".p8")
        || ends_in(file_name, b".p8c")
        || (cfg!(feature = "pico8-png") && ends_in(file_name, b".p8.png"))
}

/// Whether `file_name` ends in `ext`, ignoring case.
fn ends_in(file_name: &str, ext: &[u8]) -> bool {
    let name = file_name.as_bytes();
    name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// What `cart`, a cart as its bytes, says about its saves. The bytes say which kind of cart it is;
/// the name is not consulted.
pub fn read(cart: &[u8]) -> Result<Cart, Error> {
    if is_program(cart) {
        return Ok(Cart { ids: program_ids(cart)?, loads: Vec::new() });
    }
    let code = code_bytes(cart)?;
    if code.len() > MAX_CODE {
        return Err(Error::Code);
    }
    // Most carts call neither, and scanning one is most of the work.
    if !contains(&code, b"cartdata") && !contains(&code, b"load") {
        return Ok(Cart::default());
    }
    let tokens = lex(&code);
    let budget = Budget::new();
    Ok(Cart { ids: cartdata_ids_in(&tokens, &budget), loads: string_arguments(&tokens, b"load", &budget) })
}

/// Which of `file_names`, the carts in one folder, `load(request)` runs, by the rule the P8 core
/// bundles a program's carts by: the names compared without folders, a leading `#` or their
/// extensions, ignoring case; the exact name first, and otherwise, for a request with no version,
/// the highest `-<n>` of it, which is how PICO-8 keeps a BBS cart under its id.
#[must_use]
pub fn load_target<'a>(request: &str, file_names: &'a [String]) -> Option<&'a String> {
    let want = load_stem(request);
    if want.is_empty() {
        return None;
    }
    if let Some(exact) = file_names.iter().find(|name| load_stem(name) == want) {
        return Some(exact);
    }
    if version_of(&want).is_some() {
        return None;
    }
    file_names
        .iter()
        .filter_map(|name| {
            let stem = load_stem(name);
            let version = stem
                .strip_prefix(want.as_str())?
                .strip_prefix('-')
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))?
                .parse::<u64>()
                .unwrap_or(u64::MAX);
            Some((version, name))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, name)| name)
}

/// A cart's name as `load()` compares it: `games/Foo-3.p8.png` is `foo-3`.
fn load_stem(name: &str) -> String {
    let mut s = name.rsplit(['/', '\\']).next().unwrap_or(name);
    s = s.strip_prefix('#').unwrap_or(s);
    loop {
        let lower = s.to_ascii_lowercase();
        match [".png", ".p8", ".rom", ".p8c", ".lua"].into_iter().find(|ext| lower.ends_with(ext)) {
            Some(ext) => s = &s[..s.len() - ext.len()],
            None => break,
        }
    }
    s.to_ascii_lowercase()
}

/// The `-<n>` a BBS cart's name ends in.
fn version_of(stem: &str) -> Option<&str> {
    let (_, version) = stem.rsplit_once('-')?;
    (!version.is_empty() && version.bytes().all(|b| b.is_ascii_digit())).then_some(version)
}

/// The Lua code in `cart`, a `.p8` or a `.p8.png`, as its bytes.
fn code_bytes(cart: &[u8]) -> Result<Vec<u8>, Error> {
    if cart.starts_with(b"pico-8 cartridge") {
        return Ok(text_cart_code(cart));
    }
    if !cart.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(Error::NotACart);
    }
    image::code(cart)
}

/// Each byte as the character of the same number, which is how PICO-8 reads its code.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// A `.p8`'s code: every line between `__lua__` and the next section.
fn text_cart_code(cart: &[u8]) -> Vec<u8> {
    let mut out: Vec<&[u8]> = Vec::new();
    let mut inside = false;
    for line in cart.split(|&b| b == b'\n') {
        let tag = line.trim_ascii();
        if tag.len() > 4 && tag.starts_with(b"__") && tag.ends_with(b"__") {
            inside = tag == b"__lua__";
            continue;
        }
        if inside {
            out.push(line);
        }
    }
    out.join(&b'\n')
}

// ---- Programs the P8 compiler built

/// The module format a program's modules are read in. The compiler bumps it whenever anything in a
/// module moves, and nothing says which fields an older or newer one has, so only this one is read.
const MODULE_MAGIC: &[u8; 4] = b"P8C7";

/// What follows the runtime's first word in a program image.
const IMAGE_MAGIC: &[u8; 4] = b"P8X1";

/// What starts the table of the carts a program bundles.
const BUNDLE_MAGIC: &[u8; 4] = b"P8B1";

/// The most carts a bundle table is read for, as the P8 core reads it.
const MAX_BUNDLED: usize = 256;

/// Whether `bytes` is a program the P8 compiler built: an image, or a module on its own.
fn is_program(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(IMAGE_MAGIC.as_slice()) || bytes.get(..3) == Some(b"P8C".as_slice())
}

/// Little-endian words and length-prefixed byte strings, as a program is written.
struct Words<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Words<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Program("truncated module"))?;
        let s = self.data.get(self.at..end).ok_or(Error::Program("truncated module"))?;
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<usize, Error> {
        let word = u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes"));
        usize::try_from(word).map_err(|_| Error::Program("a length past this machine's"))
    }

    fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let n = self.u32()?;
        self.take(n)
    }

    /// Steps over `n` length-prefixed byte strings.
    fn skip_bytes(&mut self, n: usize) -> Result<(), Error> {
        for _ in 0..n {
            self.bytes()?;
        }
        Ok(())
    }
}

/// The ids every cart in a program records: its own, then the bundled carts', each once.
fn program_ids(image: &[u8]) -> Result<Vec<String>, Error> {
    let mut ids = Ids::default();
    for module in program_modules(image)? {
        for id in module_ids(module)? {
            ids.keep(id);
        }
    }
    Ok(ids.list)
}

/// The modules of a program: a bare module is its only one; an image holds its own module where its
/// header says, and after it, at the next 16 bytes, the table of the carts it bundles.
fn program_modules(image: &[u8]) -> Result<Vec<&[u8]>, Error> {
    if image.starts_with(b"P8C") {
        return Ok(vec![image]);
    }
    let word = |at: usize| {
        image.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")) as usize)
    };
    let bad = Error::Program("bad image header");
    let (offset, len) = (word(8).ok_or(bad.clone())?, word(12).ok_or(bad.clone())?);
    let end = offset.checked_add(len).ok_or(bad.clone())?;
    let own = image.get(offset..end).ok_or(bad)?;
    let table_at = end.div_ceil(16) * 16;
    let Some(table) = image.get(table_at..).filter(|t| t.starts_with(BUNDLE_MAGIC)) else {
        return Ok(vec![own]);
    };
    // The first entry is the program's own cart, so the table alone lists them all.
    let mut words = Words { data: table, at: 4 };
    let count = words.u32()?.min(MAX_BUNDLED);
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        words.bytes()?;
        let (at, len) = (words.u32()?, words.u32()?);
        let end = at.checked_add(len).ok_or(Error::Program("bad bundle"))?;
        modules.push(image.get(at..end).ok_or(Error::Program("bad bundle"))?);
    }
    Ok(modules)
}

/// The ids a module records, read past everything before them. A module written before the
/// compiler recorded them ends before them, and records none.
fn module_ids(module: &[u8]) -> Result<Vec<String>, Error> {
    if !module.starts_with(MODULE_MAGIC) {
        return Err(Error::Program("a module format other than P8C7"));
    }
    let mut w = Words { data: module, at: 4 };
    w.bytes()?; // the cart's ROM
    w.u32()?; // where the code loads
    w.bytes()?; // the code
    let strings = w.u32()?;
    w.skip_bytes(strings)?;
    w.u32()?; // the constant table
    let globals = w.u32()?;
    w.take(globals.checked_mul(4).ok_or(Error::Program("truncated module"))?)?;
    w.u32()?; // the globals table
    let protos = w.u32()?;
    for _ in 0..protos {
        w.take(8)?; // entry, fast entry
        w.take(3)?; // parameters, varargs, stack
        let upvalues = usize::from(w.u8()?);
        w.take(upvalues * 2)?;
        let children = w.u32()?;
        w.take(children.checked_mul(4).ok_or(Error::Program("truncated module"))?)?;
        w.u32()?; // line
        w.bytes()?; // name
    }
    let chunks = w.u32()?;
    w.take(chunks.checked_mul(4).ok_or(Error::Program("truncated module"))?)?;
    let titles = w.u32()?;
    w.skip_bytes(titles)?;
    if w.at == module.len() {
        return Ok(Vec::new());
    }
    let count = w.u32()?;
    if count > MAX_MODULE_IDS {
        return Err(Error::Program("more ids than any cart records"));
    }
    let mut ids = Ids::default();
    for _ in 0..count {
        ids.keep(latin1(w.bytes()?));
    }
    Ok(ids.list)
}

/// Legal ids, each once, in the order first met, at most [`MAX_IDS`] of them.
#[derive(Default)]
struct Ids {
    list: Vec<String>,
    seen: BTreeSet<String>,
}

impl Ids {
    fn keep(&mut self, id: String) {
        if self.list.len() < MAX_IDS && is_cartdata_id(&id) && self.seen.insert(id.clone()) {
            self.list.push(id);
        }
    }
}

// ---- Reading the code

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ident,
    Str,
    Num,
    Punct,
}

/// A token, borrowed from the code unless it is a string with an escape in it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token<'a> {
    kind: Kind,
    text: Cow<'a, [u8]>,
}

impl Token<'_> {
    fn is(&self, kind: Kind, text: &[u8]) -> bool {
        self.kind == kind && *self.text == *text
    }

    fn punct(&self, text: &[u8]) -> bool {
        self.is(Kind::Punct, text)
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Whether `needle` is anywhere in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle, 0).is_some()
}

/// Where `close` next starts in `src` at or after `from`.
fn find(src: &[u8], close: &[u8], from: usize) -> Option<usize> {
    src.get(from..)?.windows(close.len()).position(|w| w == close).map(|at| at + from)
}

/// The end of a long bracket opening at `open` (the first `[`), as `(where its body starts, the
/// bracket that closes it)`, or `None` when it is a lone `[`.
fn long_bracket(src: &[u8], open: usize) -> Option<(usize, Vec<u8>)> {
    let mut k = open + 1;
    while src.get(k) == Some(&b'=') {
        k += 1;
    }
    (src.get(k) == Some(&b'[')).then(|| {
        let mut close = vec![b']'];
        close.extend(std::iter::repeat_n(b'=', k - open - 1));
        close.push(b']');
        (k + 1, close)
    })
}

/// Identifiers, strings, numbers and punctuation, comments left out.
fn lex(src: &[u8]) -> Vec<Token<'_>> {
    fn token(kind: Kind, text: &[u8]) -> Token<'_> {
        Token { kind, text: Cow::Borrowed(text) }
    }
    let n = src.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < n {
        let c = src[i];
        if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        } else if src[i..].starts_with(b"--") || src[i..].starts_with(b"//") {
            if src.get(i + 2) == Some(&b'[')
                && let Some((body, close)) = long_bracket(src, i + 2)
            {
                i = find(src, &close, body).map_or(n, |end| end + close.len());
                continue;
            }
            i = find(src, b"\n", i).map_or(n, |end| end + 1);
        } else if c == b'"' || c == b'\'' {
            let start = i + 1;
            let mut j = start;
            let mut escaped: Option<Vec<u8>> = None;
            while j < n && src[j] != c && src[j] != b'\n' {
                if src[j] == b'\\' && j + 1 < n {
                    let text = escaped.get_or_insert_with(|| src[start..j].to_vec());
                    text.push(if src[j + 1] == b'n' { b'\n' } else { src[j + 1] });
                    j += 2;
                    continue;
                }
                if let Some(text) = escaped.as_mut() {
                    text.push(src[j]);
                }
                j += 1;
            }
            tokens.push(Token {
                kind: Kind::Str,
                text: escaped.map_or(Cow::Borrowed(&src[start..j]), Cow::Owned),
            });
            i = j + 1;
        } else if c == b'['
            && let Some((body, close)) = long_bracket(src, i)
        {
            let end = find(src, &close, body);
            let mut text = &src[body..end.unwrap_or(n)];
            if text.first() == Some(&b'\n') {
                text = &text[1..];
            }
            tokens.push(token(Kind::Str, text));
            i = end.map_or(n, |end| end + close.len());
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < n
                && (is_word(src[j]) && src[j] != b'_'
                    || src[j] == b'.' && src.get(j + 1).is_some_and(u8::is_ascii_digit))
            {
                j += 1;
            }
            tokens.push(token(Kind::Num, &src[i..j]));
            i = j;
        } else if is_word(c) {
            let mut j = i;
            while j < n && is_word(src[j]) {
                j += 1;
            }
            tokens.push(token(Kind::Ident, &src[i..j]));
            i = j;
        } else {
            tokens.push(token(Kind::Punct, &src[i..=i]));
            i += 1;
        }
    }
    tokens
}

fn cartdata_ids_in(tokens: &[Token<'_>], budget: &Budget) -> Vec<String> {
    let mut ids = Ids::default();
    for args in calls(tokens, b"cartdata", budget) {
        if let [only] = args
            && only.kind == Kind::Str
        {
            ids.keep(latin1(&only.text));
        } else if !args.is_empty()
            && let Some(values) = resolve_chain(args, tokens, 0, budget)
            // A variable used for something else besides yields a value that cannot be an id, and
            // then nothing it holds is trusted.
            && values.iter().all(|v| is_cartdata_id(v))
        {
            for value in values {
                ids.keep(value);
            }
        }
        if ids.list.len() >= MAX_IDS {
            break;
        }
    }
    ids.list
}

/// The string each call of the builtin `name` passes as its first argument, for the calls that pass
/// one as it is, each once.
fn string_arguments(tokens: &[Token<'_>], name: &[u8], budget: &Budget) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for args in calls(tokens, name, budget) {
        if let [only] = args
            && only.kind == Kind::Str
        {
            let text = latin1(&only.text);
            if seen.insert(text.clone()) {
                found.push(text);
            }
            if found.len() >= MAX_IDS {
                break;
            }
        }
    }
    found
}

/// The first argument of each call of the builtin `name`, as its tokens, for as many calls as
/// `budget` pays to find the end of.
fn calls<'t, 'a>(tokens: &'t [Token<'a>], name: &[u8], budget: &Budget) -> Vec<&'t [Token<'a>]> {
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !token.is(Kind::Ident, name) {
            continue;
        }
        // `function cartdata(...)`, `x.cartdata`, `x:cartdata`: a definition or a method, not the
        // builtin.
        if i > 0 && matches!(&*tokens[i - 1].text, b"function" | b"." | b":" | b"local") {
            continue;
        }
        let Some(next) = tokens.get(i + 1) else {
            continue;
        };
        if next.kind == Kind::Str {
            // `cartdata"id"`
            found.push(&tokens[i + 1..=i + 1]);
            continue;
        }
        if !next.punct(b"(") {
            // A reference (`local f = cartdata`), not a call.
            continue;
        }
        let mut depth = 0;
        let mut j = i + 1;
        while j < tokens.len() {
            if tokens[j].punct(b"(") {
                depth += 1;
            } else if tokens[j].punct(b")") {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        // An unclosed call runs to the end of the code, and a cart of nothing but those would be
        // scanned to the end once for each.
        if !budget.spend(j - i) {
            break;
        }
        found.push(first_argument(&tokens[i + 2..j]));
    }
    found
}

/// The tokens of a call's first argument.
fn first_argument<'t, 'a>(args: &'t [Token<'a>]) -> &'t [Token<'a>] {
    let mut depth = 0i32;
    for (k, token) in args.iter().enumerate() {
        if token.kind != Kind::Punct {
            continue;
        }
        match &*token.text {
            b"(" | b"[" | b"{" => depth += 1,
            b")" | b"]" | b"}" => depth -= 1,
            b"," if depth == 0 => return &args[..k],
            _ => {}
        }
    }
    args
}

/// What Lua's concatenation makes of a decimal integer; `None` for any other number.
fn number_text(text: &[u8]) -> Option<String> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let first = text.iter().position(|&b| b != b'0').unwrap_or(text.len() - 1);
    Some(latin1(&text[first..]))
}

/// `a .. b .. c` as its operands, or `None` if it is anything else.
fn split_chain<'t, 'a>(expr: &'t [Token<'a>]) -> Option<Vec<&'t [Token<'a>]>> {
    let mut operands = Vec::new();
    let mut k = 0;
    while k < expr.len() {
        match expr[k].kind {
            Kind::Str | Kind::Num => {
                operands.push(&expr[k..=k]);
                k += 1;
            }
            Kind::Ident => {
                let start = k;
                k += 1;
                while k + 1 < expr.len() && expr[k].punct(b".") && expr[k + 1].kind == Kind::Ident {
                    k += 2;
                }
                operands.push(&expr[start..k]);
            }
            Kind::Punct => return None,
        }
        if k == expr.len() {
            return Some(operands);
        }
        if !(k + 1 < expr.len() && expr[k].punct(b".") && expr[k + 1].punct(b".")) {
            return None;
        }
        k += 2;
    }
    None
}

const KEYWORDS: [&[u8]; 7] = [b"local", b"function", b"end", b"if", b"return", b"for", b"while"];

/// Every value the code assigns `name` by a plain `name = <chain>`, or `None` when it never does.
/// `field` when `name` is reached as `x.name`.
fn assigned_values(
    name: &[u8],
    field: bool,
    tokens: &[Token<'_>],
    depth: usize,
    budget: &Budget,
) -> Option<BTreeSet<String>> {
    // The whole code is scanned for each variable followed, which is what the budget is for.
    if !budget.spend(tokens.len()) {
        return None;
    }
    let mut values = BTreeSet::new();
    let mut found = false;
    for k in 0..tokens.len().saturating_sub(2) {
        if !tokens[k].is(Kind::Ident, name) || !tokens[k + 1].punct(b"=") {
            continue;
        }
        if tokens[k + 2].punct(b"=") {
            // `==`
            continue;
        }
        if !field && k > 0 && (tokens[k - 1].punct(b".") || tokens[k - 1].punct(b":")) {
            continue;
        }
        let mut end = k + 2;
        while end < tokens.len()
            && (matches!(tokens[end].kind, Kind::Str | Kind::Num)
                || tokens[end].kind == Kind::Ident && !KEYWORDS.contains(&&*tokens[end].text)
                || tokens[end].punct(b"."))
        {
            // The next statement starts at an operand that does not follow `..`.
            if end > k + 2 && tokens[end].kind != Kind::Punct && !tokens[end - 1].punct(b".") {
                break;
            }
            end += 1;
        }
        let Some(rhs) = resolve_chain(&tokens[k + 2..end], tokens, depth + 1, budget) else {
            continue;
        };
        found = true;
        values.extend(rhs);
        // A variable holding more values than a chain is followed through gives up any chain it is
        // in, so the rest of its assignments need not be read, nor kept.
        if values.len() > MAX_CANDIDATES {
            return None;
        }
    }
    found.then_some(values)
}

/// `value`, cut short once it is longer than any id. A value that long is no id, and stays none
/// whatever is joined to it, so what is past the cut can never matter; keeping it would let a cart
/// join a long string to itself until the joins alone cost gigabytes.
fn capped(mut value: String) -> String {
    if value.len() > MAX_ID_LEN {
        let mut cut = MAX_ID_LEN + 1;
        while !value.is_char_boundary(cut) {
            cut += 1;
        }
        value.truncate(cut);
    }
    value
}

/// Every value `expr`, a chain of strings, numbers and variables, can be, each [`capped`].
fn resolve_chain(
    expr: &[Token<'_>],
    tokens: &[Token<'_>],
    depth: usize,
    budget: &Budget,
) -> Option<BTreeSet<String>> {
    if depth > MAX_DEPTH || expr.is_empty() {
        return None;
    }
    let mut results = BTreeSet::from([String::new()]);
    for operand in split_chain(expr)? {
        let first = &operand[0];
        let options: BTreeSet<String> = match first.kind {
            Kind::Str => BTreeSet::from([capped(latin1(&first.text))]),
            Kind::Num => BTreeSet::from([capped(number_text(&first.text)?)]),
            _ => {
                let last = &operand[operand.len() - 1];
                assigned_values(&last.text, operand.len() > 1, tokens, depth, budget)
                    .filter(|values| !values.is_empty())?
            }
        };
        // Each joined value is made one at a time and the chain given up as soon as there are too
        // many, rather than all of them made first: the set only grows, so the answer is the same.
        let mut joined = BTreeSet::new();
        for a in &results {
            for b in &options {
                joined.insert(capped(format!("{a}{b}")));
                if joined.len() > MAX_CANDIDATES {
                    return None;
                }
            }
        }
        results = joined;
    }
    Some(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No ids.
    const NONE: [&str; 0] = [];

    fn ids_in(code: &str) -> Vec<String> {
        cartdata_ids_in(&lex(code.as_bytes()), &Budget::new())
    }

    #[test]
    fn a_string_passed_as_it_is() {
        assert_eq!(ids_in(r#"cartdata("porklike")"#), ["porklike"]);
        assert_eq!(ids_in(r#"cartdata"porklike""#), ["porklike"]);
        assert_eq!(ids_in("cartdata('porklike')"), ["porklike"]);
        assert_eq!(ids_in("cartdata([[porklike]])"), ["porklike"]);
    }

    #[test]
    fn a_mention_is_not_a_call() {
        assert_eq!(ids_in(r#"-- cartdata("old_id")"#), NONE);
        assert_eq!(ids_in(r#"--[[ cartdata("old_id") ]] x=1"#), NONE);
        assert_eq!(ids_in(r#"// cartdata("old_id")"#), NONE);
        assert_eq!(ids_in(r#"print("cartdata(\"old_id\")")"#), NONE);
        assert_eq!(ids_in(r"function cartdata(x) end"), NONE);
        assert_eq!(ids_in(r#"obj:cartdata("x")"#), NONE);
        assert_eq!(ids_in("local f = cartdata"), NONE);
    }

    #[test]
    fn a_variable_is_followed_to_what_it_is_given() {
        assert_eq!(ids_in("id = \"mot_gpp\"\ncartdata(id)"), ["mot_gpp"]);
        assert_eq!(ids_in("v = 2\nname = \"game_v\"..v\ncartdata(name)"), ["game_v2"]);
        assert_eq!(ids_in("g = {}\ng.save = \"a_b\"\ncartdata(g.save)"), ["a_b"]);
        // Assigned only something that is not a chain, the variable gives nothing.
        assert_eq!(ids_in("id = stat(6)\ncartdata(id)"), NONE);
        // Beside a chain, such an assignment is passed over, and an assignment is read as far as
        // a chain goes: the harvest reads both the same way.
        assert_eq!(ids_in("id = \"ok\"\nid = stat(6)\ncartdata(id)"), ["ok"]);
        assert_eq!(ids_in("v = 1\nv = v + 1\ncartdata(\"g\"..v)"), ["g1"]);
    }

    #[test]
    fn a_value_longer_than_any_id_is_kept_only_as_far_as_it_takes_to_be_none() {
        let code = format!("x = \"{}\"\ny = \"b\"\ny = \"c\"", "a".repeat(10_000));
        let tokens = lex(code.as_bytes());
        let values = resolve_chain(&lex(b"x .. y"), &tokens, 0, &Budget::new()).unwrap();
        // Both joins are cut at the same place, past which nothing could be an id.
        assert_eq!(values.len(), 1);
        assert!(values.iter().all(|v| v.len() == MAX_ID_LEN + 1 && !is_cartdata_id(v)));
        assert_eq!(ids_in(&format!("{code}\ncartdata(x .. y)")), NONE);
        // An id at the limit is still whole.
        let longest = "a".repeat(MAX_ID_LEN);
        assert_eq!(ids_in(&format!("x = \"{longest}\"\ncartdata(x)")), [longest]);
        // A cut never splits a character, and a code byte of 0x80 or more reads as one of two bytes.
        assert_eq!(capped(format!("{}\u{e9}", "a".repeat(MAX_ID_LEN))).len(), MAX_ID_LEN + 2);
    }

    #[test]
    fn a_variable_with_more_values_than_a_chain_takes_is_given_up_at_once() {
        let assigned = |n: usize| (0..n).map(|i| format!("y = \"b{i}\"\n")).collect::<Vec<_>>().concat();
        let read = |n: usize| assigned_values(b"y", false, &lex(assigned(n).as_bytes()), 0, &Budget::new());
        assert_eq!(read(MAX_CANDIDATES).map(|values| values.len()), Some(MAX_CANDIDATES));
        assert_eq!(read(MAX_CANDIDATES + 1), None);
        assert_eq!(ids_in(&format!("{}cartdata(y)", assigned(MAX_CANDIDATES + 1))), NONE);
        // Used for text besides: a value that is no id spoils the rest.
        assert_eq!(ids_in("id = \"ok\"\nid = \"Not An Id\"\ncartdata(id)"), NONE);
    }

    #[test]
    fn an_illegal_id_is_no_save() {
        assert_eq!(ids_in(r#"cartdata("Capitals")"#), NONE);
        assert_eq!(ids_in(r#"cartdata("has space")"#), NONE);
        assert!(is_cartdata_id("kai-pumpkin-2-v1-1"));
        assert!(!is_cartdata_id(&"a".repeat(65)));
        assert!(!is_cartdata_id(""));
    }

    #[test]
    fn each_id_once_in_the_order_named() {
        assert_eq!(ids_in(r#"cartdata("b") cartdata("a") cartdata("b")"#), ["b", "a"]);
    }

    #[test]
    fn a_text_cart_keeps_its_code_in_its_lua_section() {
        let cart = b"pico-8 cartridge // http://www.pico-8.com\nversion 41\n__lua__\ncartdata(\"qa_reset_counter\")\nn=dget(0)+1\n__gfx__\n0000\n";
        assert_eq!(read(cart).unwrap().ids, ["qa_reset_counter"]);
    }

    #[test]
    fn a_launcher_names_the_carts_it_loads() {
        let cart = b"pico-8 cartridge\n__lua__\nload(\"#mot_airpicogame\")\nload\"level2.p8\" load(name) load('#mot_airpicogame')\n-- load(\"#old\")\n";
        let cart = read(cart).unwrap();
        assert_eq!(cart.ids, NONE);
        assert_eq!(cart.loads, ["#mot_airpicogame", "level2.p8"]);
    }

    #[test]
    fn a_load_finds_its_cart_as_the_p8_core_does() {
        let names: Vec<String> = [
            "mot_airpicogame-2.p8.png",
            "mot_airpicogame-3.p8.png",
            "mot_airpico-1.p8.png",
            "level2.p8",
            "Solitomb_Title-2.p8.png",
        ]
        .map(String::from)
        .to_vec();
        let target = |request| load_target(request, &names).map(String::as_str);
        assert_eq!(target("#mot_airpicogame"), Some("mot_airpicogame-3.p8.png"));
        assert_eq!(target("#mot_airpicogame-2"), Some("mot_airpicogame-2.p8.png"));
        assert_eq!(target("#mot_airpicogame-9"), None);
        assert_eq!(target("#mot_airpico"), Some("mot_airpico-1.p8.png"));
        assert_eq!(target("level2.p8"), Some("level2.p8"));
        assert_eq!(target("solitomb_title"), Some("Solitomb_Title-2.p8.png"));
        assert_eq!(target("#"), None);
    }

    #[test]
    fn carts_are_named_p8_p8_png_and_p8c() {
        assert!(is_cart_name("lootslime-1.p8.png"));
        assert!(is_cart_name("QA_RESET.P8"));
        assert!(is_cart_name("lootslime.p8c"));
        assert!(!is_cart_name("boot.rom"));
        assert!(!is_cart_name("picture.png"));
        // A cart in a picture is a cart either way, and read only with `pico8-png`.
        assert!(reads_cart_named("QA_RESET.P8") && reads_cart_named("lootslime.p8c"));
        assert_eq!(reads_cart_named("lootslime-1.p8.png"), cfg!(feature = "pico8-png"));
        assert!(!reads_cart_named("picture.png"));
    }

    #[test]
    fn bytes_that_are_no_kind_of_cart_are_refused() {
        assert_eq!(read(b"not a cart"), Err(Error::NotACart));
    }

    #[cfg(not(feature = "pico8-png"))]
    #[test]
    fn without_pico8_png_a_cart_in_a_picture_is_named_and_refused() {
        assert_eq!(read(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Err(Error::Unsupported));
    }

    /// Writes the P8 compiler's module, as `p8_core::native::Module::to_bytes` does, with `ids`
    /// recorded, or ending before them for `None`.
    fn module(ids: Option<&[&str]>) -> Vec<u8> {
        fn bytes(out: &mut Vec<u8>, b: &[u8]) {
            out.extend(u32::try_from(b.len()).unwrap().to_le_bytes());
            out.extend(b);
        }
        let word = |out: &mut Vec<u8>, w: u32| out.extend(w.to_le_bytes());
        let mut out = MODULE_MAGIC.to_vec();
        bytes(&mut out, &[1, 2, 3]); // rom
        word(&mut out, 0x0080_0000); // code base
        bytes(&mut out, &[0; 16]); // code
        word(&mut out, 2); // strings
        bytes(&mut out, b"cartdata");
        bytes(&mut out, b"porklike");
        word(&mut out, 0); // constant table
        word(&mut out, 1); // globals
        word(&mut out, 0);
        word(&mut out, 4); // globals table
        word(&mut out, 1); // protos
        word(&mut out, 0); // entry
        word(&mut out, 0); // fast
        out.extend([0, 0, 8, 1]); // params, vararg, stack, upvalues
        out.extend([1, 0]); // the upvalue
        word(&mut out, 1); // children
        word(&mut out, 0);
        word(&mut out, 7); // line
        bytes(&mut out, b"_init");
        word(&mut out, 1); // chunks
        word(&mut out, 0);
        word(&mut out, 1); // titles
        bytes(&mut out, b"porklike");
        if let Some(ids) = ids {
            word(&mut out, u32::try_from(ids.len()).unwrap());
            for id in ids {
                bytes(&mut out, id.as_bytes());
            }
            out.push(0); // dynamic
        }
        out
    }

    /// An image: a runtime with the header the core reads, the program's module, and a bundle table
    /// of `bundled` carts after it, the first the program's own.
    fn image(own: &[u8], bundled: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0u8; 64];
        out[4..8].copy_from_slice(IMAGE_MAGIC);
        out[8..12].copy_from_slice(&64u32.to_le_bytes());
        out[12..16].copy_from_slice(&u32::try_from(own.len()).unwrap().to_le_bytes());
        out.extend(own);
        if bundled.is_empty() {
            return out;
        }
        out.resize(out.len().div_ceil(16) * 16, 0);
        let table_len: usize = 8 + bundled.iter().map(|(name, _)| 12 + name.len()).sum::<usize>();
        let mut at = out.len() + table_len;
        let mut table = BUNDLE_MAGIC.to_vec();
        table.extend(u32::try_from(bundled.len()).unwrap().to_le_bytes());
        for (name, module) in bundled {
            table.extend(u32::try_from(name.len()).unwrap().to_le_bytes());
            table.extend(name.as_bytes());
            table.extend(u32::try_from(at).unwrap().to_le_bytes());
            table.extend(u32::try_from(module.len()).unwrap().to_le_bytes());
            at += module.len();
        }
        out.extend(table);
        for (_, module) in bundled {
            out.extend(module);
        }
        out
    }

    #[test]
    fn a_program_records_the_ids_it_was_compiled_with() {
        let own = module(Some(&["porklike", "Not An Id", "porklike"]));
        assert_eq!(read(&own).unwrap(), Cart { ids: vec!["porklike".into()], loads: vec![] });
        assert_eq!(read(&image(&own, &[])).unwrap().ids, ["porklike"]);
    }

    #[test]
    fn a_program_answers_for_every_cart_it_bundles() {
        let own = module(Some(&["mot_gpp"]));
        let game = module(Some(&["mot_gpp", "mot_gpp_records"]));
        let program = image(&own, &[("mot_grandprix2", own.clone()), ("mot_grandprix2game", game)]);
        assert_eq!(read(&program).unwrap().ids, ["mot_gpp", "mot_gpp_records"]);
    }

    #[test]
    fn a_program_from_before_the_ids_records_none() {
        assert_eq!(read(&module(None)).unwrap().ids, NONE);
    }

    #[test]
    fn a_module_format_this_does_not_know_is_refused() {
        let mut other = module(Some(&["porklike"]));
        other[3] = b'9';
        assert!(matches!(read(&other), Err(Error::Program(_))));
        let mut short = module(Some(&["porklike"]));
        short.truncate(40);
        assert!(matches!(read(&short), Err(Error::Program(_))));
    }
}
