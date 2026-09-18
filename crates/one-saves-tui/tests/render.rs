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

/// A picker with a fixed cell size and a fixed protocol, so a test asks the terminal nothing and
/// a drawn icon is cells rather than an escape sequence carrying a base64 PNG.
fn picker() -> Picker {
    let mut picker = Picker::from_fontsize((8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Halfblocks);
    picker
}

/// Everything drawn with the spacing taken out, for asserting on content.
///
/// A wide character occupies two cells and the second is left blank, so reading the buffer cell by
/// cell puts a gap inside every full-width word. What a person sees has no gap.
fn text(app: &mut App, width: u16, height: u16) -> String {
    screen(app, width, height).replace(' ', "")
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
    let content = text(&mut app, 100, 24);

    assert!(drawn.contains("Mcd001.ps2"), "the card names itself:\n{drawn}");
    // Free space comes from the registry's block size, not from arithmetic in the view.
    assert!(drawn.contains("free"), "and what is left on it");

    // The names come from `x.1sav.label` — what the console's browser shows — rather than from
    // the directory, which holds `BASLUS-20328Tekken-4`.
    assert!(content.contains("ＴＥＫＫＥＮ"), "a save is listed by what the console calls it:\n{drawn}");
    assert!(!content.contains("BASLUS"), "and not by its directory name");
    assert!(drawn.contains("blk"), "with what each occupies");
    assert!(drawn.contains("q quit"), "and the keys are shown");
}

#[test]
fn deleting_asks_first_and_names_what_it_would_delete() {
    let mut app = App::new(vec![card()], picker());
    app.ask_delete();
    let drawn = screen(&mut app, 100, 24);
    let content = text(&mut app, 100, 24);

    assert!(content.contains("Delete"), "it asks:\n{drawn}");
    assert!(content.contains("ＴＥＫＫＥＮ"), "and names the save as the console does");
    assert!(content.contains("[y/n]"), "and says what answers it takes");
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
fn a_full_width_title_is_measured_in_cells_rather_than_characters() {
    // A PS2 browser line is full width, where every character takes two terminal cells. Counting
    // characters would run the name column to twice its width and push the size off the screen.
    let mut app = App::new(vec![card()], picker());
    app.step(1); // onto a Dragon Quest save, whose title is long as well as wide
    let drawn = screen(&mut app, 76, 10);

    assert!(drawn.lines().all(|line| line.chars().count() == 76), "no line overflows");
    assert!(drawn.contains(" blk"), "the size column survives a wide title:\n{drawn}");
    assert!(drawn.contains('…'), "and the title reads as cut");
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
