//! What the interface puts on screen, drawn into a buffer instead of a terminal.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::Picker;

use one_saves_tui::app::{App, Mode};
use one_saves_tui::model::Card;

const PS2: &str = "PS2/Dragon Quest VIII and Tekken 4/PCSX2/Mcd001.ps2";

fn card() -> Card {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(PS2);
    Card::open(path).expect("opens")
}

/// A picker with a fixed cell size, so nothing asks the terminal anything during a test.
fn picker() -> Picker {
    Picker::from_fontsize((8, 16))
}

/// Everything drawn, as one string, which is what a person would be reading.
fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draws");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_card_and_its_saves_are_on_screen() {
    let mut app = App::new(vec![card()], picker());
    let drawn = screen(&mut app, 100, 24);

    assert!(drawn.contains("Mcd001.ps2"), "the card names itself:\n{drawn}");
    // Free space comes from the registry's block size, not from arithmetic in the view.
    assert!(drawn.contains("free"), "and what is left on it");

    for save in ["BASLUS-20328Tekken-4", "BASLUS-21207dq8_0", "BASLUS-21207dq8_1"] {
        assert!(drawn.contains(save), "{save} is listed:\n{drawn}");
    }
    assert!(drawn.contains("blk"), "with what each occupies");
    assert!(drawn.contains("q quit"), "and the keys are shown");
}

#[test]
fn deleting_asks_first_and_names_what_it_would_delete() {
    let mut app = App::new(vec![card()], picker());
    app.ask_delete();
    let drawn = screen(&mut app, 100, 24);

    assert!(drawn.contains("Delete"), "it asks:\n{drawn}");
    assert!(drawn.contains("Tekken"), "and names the save");
    assert!(drawn.contains("[y/n]"), "and says what answers it takes");
    assert!(!app.cards[0].dirty(), "asking is not doing");
}

#[test]
fn an_edited_card_says_so_until_it_is_written() {
    let mut app = App::new(vec![card()], picker());
    assert!(!screen(&mut app, 100, 24).contains('●'));

    app.cards[0].remove(0).expect("removes");
    let drawn = screen(&mut app, 100, 24);
    assert!(drawn.contains('●'), "an unwritten edit is visible:\n{drawn}");
    // The marker leads the title, so it is not what a narrow pane cuts.
    assert!(screen(&mut app, 44, 12).contains('●'), "and survives a narrow pane");
}

#[test]
fn copying_with_one_card_open_says_what_is_missing() {
    let mut app = App::new(vec![card()], picker());
    app.copy();
    assert!(matches!(&app.mode, Mode::Reporting(m) if m.contains("second card")));
}

#[test]
fn a_narrow_terminal_keeps_the_numbers_and_cuts_the_names() {
    // The block count is the column worth keeping: a name can be recognised from most of itself,
    // and a save's size cannot be recognised from part of a number.
    let mut app = App::new(vec![card()], picker());
    let drawn = screen(&mut app, 40, 12);
    assert!(drawn.lines().all(|line| line.chars().count() == 40), "no line overflows");
    assert!(drawn.contains("blk"), "the size column survives a narrow pane:\n{drawn}");
    assert!(drawn.contains('…'), "and a name too long for it reads as cut:\n{drawn}");
}
