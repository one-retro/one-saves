//! The names a ROM keeps its saves under, through the crate's own entry points:
//! `names_its_saves` from a name alone, `save_names` from the ROM, and
//! `SaveNames::loaded` for the ROMs it runs. PICO-8 is the one system that
//! answers, and only with the `pico8` feature: without it a cart is a ROM like
//! any other. A cart in a picture answers only with `pico8-png` as well.

use rom_id::{names_its_saves, save_names};

fn text_cart(code: &str) -> Vec<u8> {
    format!("pico-8 cartridge // http://www.pico-8.com\nversion 41\n__lua__\n{code}\n__gfx__\n0000\n")
        .into_bytes()
}

#[cfg(not(feature = "pico8"))]
#[test]
fn without_pico8_a_cart_names_nothing() {
    assert!(!names_its_saves("porklike-2.p8"));
    assert_eq!(save_names("porklike-2.p8", &text_cart("cartdata(\"porklike\")")), None);
}

#[cfg(feature = "pico8")]
#[test]
fn a_cart_is_told_by_its_name_alone() {
    for name in ["QA_RESET.P8", "p8c/lootslime.p8c"] {
        assert!(names_its_saves(name), "{name}");
    }
    for name in ["Zelda.sfc", "picture.png", "notes.txt", "porklike.p8d.txt"] {
        assert!(!names_its_saves(name), "{name}");
    }
    // A cart in a picture is one to read only where this build reads it.
    assert_eq!(names_its_saves("porklike-2.p8.png"), cfg!(feature = "pico8-png"));
}

#[cfg(all(feature = "pico8", not(feature = "pico8-png")))]
#[test]
fn without_pico8_png_a_cart_in_a_picture_names_nothing() {
    assert_eq!(save_names("porklike-2.p8.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None);
}

#[cfg(feature = "pico8")]
#[test]
fn a_cart_names_the_ids_its_code_saves_under() {
    let names =
        save_names("porklike-2.p8", &text_cart("cartdata(\"porklike\")\nx = dget(0)")).expect("a cart");
    assert_eq!(names.system.slug, "pico-8");
    assert_eq!(names.names, ["porklike"]);
    assert_eq!(names.loads, Vec::<String>::new());
}

#[test]
fn a_rom_of_another_system_names_nothing() {
    assert_eq!(save_names("Zelda.sfc", &vec![0; 0x8000]), None);
    // Named as a cart but not one: nothing, rather than a guess.
    assert_eq!(save_names("broken.p8.png", b"not a picture"), None);
}

#[cfg(feature = "pico8")]
#[test]
fn a_cart_that_never_saves_names_nothing_but_is_still_a_cart() {
    let names = save_names("celeste.p8", &text_cart("function _init() end")).expect("a cart");
    assert_eq!(names.names, Vec::<String>::new());
}

#[cfg(feature = "pico8")]
#[test]
fn a_launcher_finds_the_cart_it_loads_among_those_beside_it() {
    let names = save_names("mot_grandprix2-0.p8", &text_cart("load(\"#mot_grandprix2game\")")).unwrap();
    assert_eq!(names.loads, ["#mot_grandprix2game"]);
    let beside = [
        "mot_grandprix2game-1.p8.png".to_string(),
        "mot_grandprix2game-3.p8.png".to_string(),
        "other.p8".to_string(),
    ];
    assert_eq!(
        names.loaded(&names.loads[0], &beside).map(String::as_str),
        Some("mot_grandprix2game-3.p8.png")
    );
    assert_eq!(names.loaded("#nothing_here", &beside), None);
}
