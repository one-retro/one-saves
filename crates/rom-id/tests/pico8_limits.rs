//! What a cart built to cost its reader is allowed to cost.
//!
//! Carts come from the Lexaloffle BBS and from wherever a player found them,
//! and a save manager reads every cart on a device for its ids, so reading one
//! has to stay cheap whatever is in it. A cart opens at most a few ids (PICO-8
//! allows 4 in a session, the P8 core keeps 16 blocks), so no more than 64 are
//! kept; a compiled module claiming more than any cart records is refused;
//! following variables and finding where calls end share one budget of work;
//! and a text cart's code is read up to 1 MiB.

#![cfg(feature = "pico8")]

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use rom_id::pico8::{Error, read};

fn text_cart(code: &str) -> Vec<u8> {
    format!("pico-8 cartridge\n__lua__\n{code}\n__gfx__\n0000\n").into_bytes()
}

/// A bare `P8C7` module claiming `count` ids and recording `ids`.
fn module(count: u32, ids: &[&str]) -> Vec<u8> {
    fn bytes(out: &mut Vec<u8>, b: &[u8]) {
        out.extend(u32::try_from(b.len()).unwrap().to_le_bytes());
        out.extend(b);
    }
    let word = |out: &mut Vec<u8>, w: u32| out.extend(w.to_le_bytes());
    let mut out = b"P8C7".to_vec();
    bytes(&mut out, &[0]); // rom
    word(&mut out, 0x0080_0000); // code base
    bytes(&mut out, &[0; 4]); // code
    for _ in 0..7 {
        word(&mut out, 0); // strings, constants, globals, their table, protos, chunks, titles
    }
    word(&mut out, count);
    for id in ids {
        bytes(&mut out, id.as_bytes());
    }
    out.push(0);
    out
}

/// Well under a second on any machine that runs the tests; the point is
/// that it ends, where an unbounded read would not in any useful time.
fn quickly<T>(f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    out
}

#[test]
fn a_cart_naming_hundreds_of_ids_is_read_for_the_first_64() {
    let mut code = String::new();
    for n in 0..300 {
        writeln!(code, "cartdata(\"id_{n}\")").unwrap();
    }
    let cart = quickly(|| read(&text_cart(&code))).unwrap();
    assert_eq!(cart.ids.len(), 64);
    assert_eq!(cart.ids[0], "id_0");
    assert_eq!(cart.ids[63], "id_63");
}

#[test]
fn a_module_claiming_more_ids_than_any_cart_records_is_refused() {
    assert!(matches!(read(&module(4_000_000_000, &[])), Err(Error::Program(_))));
    assert!(matches!(read(&module(5_000, &[])), Err(Error::Program(_))));
    // A real count reads.
    assert_eq!(read(&module(2, &["a", "b"])).unwrap().ids, ["a", "b"]);
}

/// Thousands of variables, each assigned from the others, and a call naming
/// each: every call follows its variable through the whole code, and every
/// assignment it finds through it again. It ends, with whatever it found in
/// the budget, rather than taking forever.
#[test]
fn a_cart_built_to_be_slow_to_follow_ends() {
    let mut code = String::new();
    for n in 0..2_000 {
        writeln!(code, "v{n} = v{} .. v{}", (n + 1) % 2_000, (n + 7) % 2_000).unwrap();
        writeln!(code, "v{n} = \"x\"").unwrap();
    }
    for n in 0..2_000 {
        writeln!(code, "cartdata(v{n})").unwrap();
    }
    quickly(|| read(&text_cart(&code))).unwrap();
}

/// Calls that never close each run to the end of the code, so a cart of
/// nothing else would be scanned to its end once per call.
#[test]
fn a_cart_of_unclosed_calls_ends() {
    let code = "cartdata( ".repeat(100_000);
    let cart = quickly(|| read(&text_cart(&code))).unwrap();
    assert_eq!(cart.ids, Vec::<String>::new());
}

#[test]
fn a_text_cart_with_more_code_than_any_cart_has_is_refused() {
    let code = format!("cartdata(\"big\")\n{}", "-- padding\n".repeat(200_000));
    assert_eq!(read(&text_cart(&code)), Err(Error::Code));
}
