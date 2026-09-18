//! What the interface puts on screen, drawn into a buffer instead of a terminal.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::Picker;

use crossterm::event::KeyCode;
use one_saves_tui::app::{App, Mode, Pending};
use one_saves_tui::model::Card;

const PS2: &str = "PS2/Dragon Quest VIII and Tekken 4/PCSX2/Mcd001.ps2";
const PS1: &str = "PS1/Gran Turismo/DuckStation/shared_card_1.mcd";

fn card() -> Card {
    open(PS2)
}

fn open(rest: &str) -> Card {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(rest);
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
    assert!(content.contains("Yes"), "and offers buttons rather than naming keys");
    assert!(content.contains("No"));
    // Over the top of everything, because it is the only thing that takes a key.
    assert!(drawn.contains('╔'), "asked in a box rather than on the status line:\n{drawn}");
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
    assert!(app.status.as_ref().is_some_and(|said| said.contains("second card")));
    // And it is a report rather than a question: nothing is waiting on an answer.
    assert!(matches!(app.mode, Mode::Browsing));
}

/// Adding a save to a card is not a question; landing on top of one is.
#[test]
fn a_copy_only_asks_when_a_save_of_that_name_is_already_there() {
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    let one = Card::open(ps1.join("Gran Turismo/DuckStation/shared_card_1.mcd")).expect("opens");
    let two = Card::open(ps1.join("Castlevania Symphony of the Night/DuckStation/shared_card_2.mcd"))
        .expect("opens");

    // Different saves: it just happens, and says so.
    let mut app = App::new(vec![one, two], picker());
    app.copy();
    assert!(matches!(app.mode, Mode::Browsing), "no question for a save that fits");
    assert!(app.status.as_ref().is_some_and(|said| said.contains("copied")));
    assert!(app.cards[1].dirty());

    // Back to where the save came from, and send the same one again: that would be a second save
    // under one name on the far card, which is worth asking about.
    app.switch();
    app.copy();
    match &app.mode {
        Mode::Confirming(confirm) => {
            assert_eq!(confirm.pending, Pending::CopyOver);
            assert!(confirm.question.contains("already on the other card"));
        }
        Mode::Browsing => panic!("a duplicate name should be asked about"),
    }
}

/// Leaving with work in hand asks; leaving with none does not.
#[test]
fn quitting_asks_only_when_something_would_be_lost() {
    let mut app = App::new(vec![card()], picker());
    app.ask_quit();
    assert!(app.done, "a card with nothing to write just closes");

    let mut app = App::new(vec![card()], picker());
    app.cards[0].remove(0).expect("removes");
    app.ask_quit();
    assert!(!app.done, "an unwritten change holds the door");
    match &app.mode {
        Mode::Confirming(confirm) => {
            assert_eq!(confirm.pending, Pending::Quit);
            assert!(confirm.warning.as_ref().is_some_and(|w| w.contains("lost")));
        }
        Mode::Browsing => panic!("it should have asked"),
    }
    app.confirm(Pending::Quit);
    assert!(app.done);
}

/// What a write is asked about says that a file is being replaced.
#[test]
fn the_write_question_says_the_file_is_being_written_over() {
    let mut app = App::new(vec![card()], picker());
    app.ask_write();
    assert!(app.status.as_deref() == Some("nothing to write"), "an unedited card has nothing to do");

    app.cards[0].remove(0).expect("removes");
    app.ask_write();
    match &app.mode {
        Mode::Confirming(confirm) => {
            assert!(confirm.question.starts_with("Write over "), "{}", confirm.question);
            assert!(confirm.warning.as_ref().is_some_and(|w| w.contains("replaced")));
        }
        Mode::Browsing => panic!("it should have asked"),
    }
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

/// A console icon is drawn at a size a person can see, by a whole number of pixels.
#[test]
fn an_icon_is_blown_up_to_something_visible() {
    use ratatui_image::picker::ProtocolType;

    let mut picker = Picker::from_fontsize((8, 16));
    // A graphics protocol, because with half blocks a cell *is* a pixel or two and there is no
    // scaling to observe — the terminal's own cells are the limit.
    picker.set_protocol_type(ProtocolType::Iterm2);
    let mut app = App::new(vec![open(PS1)], picker);

    // Tall enough for the picture to have its room: the block map above it takes four rows.
    let drawn = screen(&mut app, 70, 26);
    // The protocol carries the size it was handed. A 16x16 icon scaled by eight is 128, and an
    // unscaled one would say 16 — which is what this caught before the scaling went in.
    assert!(drawn.contains("width=128px"), "the icon is blown up:\n{drawn}");
    assert!(drawn.contains("height=128px"), "and keeps its proportions");
}

/// An animated icon advances on its own; a still one gives the loop nothing to wait for.
#[test]
fn an_animated_icon_advances_and_a_still_one_does_not() {
    use std::thread::sleep;
    use std::time::Duration;

    let mut app = App::new(vec![open(PS1)], picker());

    // Gran Turismo keeps one frame. Nothing can change the screen but a key, so the event loop is
    // told to block rather than spin: that is what `None` means here.
    assert_eq!(app.next_frame_in(), None, "a still icon asks for no wakeup");
    app.tick();

    // Crash Bandicoot 2 keeps three.
    app.step(1);
    let wait = app.next_frame_in().expect("an animated icon asks to be woken");
    assert!(wait <= Duration::from_millis(250), "and within the frame's hold, not later");

    // A tick before the hold is up changes nothing; one after it moves to the next frame.
    let first = screen(&mut app, 60, 16);
    app.tick();
    assert_eq!(screen(&mut app, 60, 16), first, "the frame holds for its time");

    sleep(wait + Duration::from_millis(20));
    app.tick();
    // Frames 0 and 1 of this icon are the same picture, so the screen is expected to match; what
    // is being checked is that the wait restarted, which it only does on an advance.
    let again = app.next_frame_in().expect("still animating");
    assert!(again > wait.saturating_sub(Duration::from_millis(20)), "the hold started over");

    // And moving the selection puts the animation back to its first frame.
    app.step(-1);
    assert_eq!(app.next_frame_in(), None, "back on the still icon");
}

/// The card being acted on is obvious without reading the border characters.
#[test]
fn the_focused_card_is_marked() {
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    let one = Card::open(ps1.join("Gran Turismo/DuckStation/shared_card_1.mcd")).expect("opens");
    let two = Card::open(ps1.join("Castlevania Symphony of the Night/DuckStation/shared_card_2.mcd"))
        .expect("opens");
    let mut app = App::new(vec![one, two], picker());

    let drawn = screen(&mut app, 86, 20);
    assert_eq!(drawn.matches('▶').count(), 1, "one card is marked, and only one:\n{drawn}");
    let first = drawn.lines().position(|line| line.contains('▶')).expect("a marker");

    app.switch();
    let drawn = screen(&mut app, 86, 20);
    assert_eq!(drawn.matches('▶').count(), 1);
    let second = drawn.lines().position(|line| line.contains('▶')).expect("a marker");
    assert_ne!(first, second, "and it moves with the focus");
}

/// The row under the map's own title, so the card titles' separators are not counted.
fn map_row(drawn: &str) -> String {
    let at = drawn.lines().position(|line| line.contains("blocks used")).expect("a map");
    drawn.lines().nth(at + 1).expect("a row under it").to_owned()
}

/// The blocks a card holds, one cell each, so a save's size is visible rather than read.
#[test]
fn a_cards_blocks_are_drawn_one_cell_each() {
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    let mut app = App::new(
        vec![Card::open(ps1.join("Gran Turismo/DuckStation/shared_card_1.mcd")).expect("opens")],
        picker(),
    );
    let drawn = screen(&mut app, 86, 20);

    // Sixteen blocks: five for Gran Turismo, one for Crash, ten free.
    assert!(drawn.contains("6 of 16 blocks used"), "{drawn}");
    let row = map_row(&drawn);
    assert_eq!(row.matches('█').count() + row.matches('▓').count(), 6, "one cell per used block");
    assert_eq!(row.matches('·').count(), 10, "and one per free block:\n{drawn}");

    // The selected save's blocks are the filled ones, so selecting moves which are solid.
    assert_eq!(row.matches('█').count(), 5, "Gran Turismo is selected and takes five");
    app.step(1);
    let row = map_row(&screen(&mut app, 86, 20));
    assert_eq!(row.matches('█').count(), 1, "Crash takes one: {row}");
}

/// A report is something to read, not something to dismiss.
#[test]
fn a_status_message_does_not_swallow_the_next_key() {
    let mut app = App::new(vec![card()], picker());
    app.copy();
    assert!(app.status.is_some(), "it said something");
    // The loop clears the message and acts on the key in the same breath; this is that pair.
    app.status = None;
    app.step(1);
    assert_eq!(app.status, None, "and the key did its own job");
}

/// The dialog is answered by pressing a button, and Yes is the one under the cursor.
#[test]
fn a_question_opens_on_yes_and_is_answered_by_the_button_under_the_cursor() {
    use ratatui::style::Modifier;

    // Where the cursor is, read off the cells rather than off the state, so this checks what a
    // person can actually see.
    fn on_yes(app: &mut App) -> bool {
        let mut terminal = Terminal::new(TestBackend::new(86, 20)).expect("a test terminal");
        terminal.draw(|frame| app.draw(frame)).expect("draws");
        let buffer = terminal.backend().buffer().clone();
        let (mut yes_at, mut marked) = (None, None);
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol() == "Y" {
                    yes_at = Some(x);
                }
                if cell.modifier.contains(Modifier::BOLD) && cell.symbol() == "Y" {
                    marked = Some(x);
                }
            }
        }
        yes_at.is_some() && yes_at == marked
    }

    let mut app = App::new(vec![card()], picker());
    app.ask_delete();
    assert!(on_yes(&mut app), "a question opens with Yes under the cursor");

    // Moving puts it on No, and answering there does nothing but put the question away.
    app.toggle();
    assert!(!on_yes(&mut app));
    app.answer();
    assert!(matches!(app.mode, Mode::Browsing), "answered");
    assert!(!app.cards[0].dirty(), "and No did not delete anything");

    // Answering on Yes does the thing.
    app.ask_delete();
    let before = app.cards[0].entries().len();
    app.answer();
    assert!(matches!(app.mode, Mode::Browsing));
    assert_eq!(app.cards[0].entries().len(), before - 1, "Yes deleted it");
    assert!(app.cards[0].dirty());
}

/// The letters move the cursor rather than answering outright.
#[test]
fn y_and_n_point_at_a_button_without_committing() {
    let mut app = App::new(vec![card()], picker());
    let before = app.cards[0].entries().len();

    app.ask_delete();
    app.point_at(true);
    assert!(matches!(app.mode, Mode::Confirming(_)), "y points, it does not press");
    assert_eq!(app.cards[0].entries().len(), before, "and nothing has happened yet");

    app.point_at(false);
    app.answer();
    assert_eq!(app.cards[0].entries().len(), before, "answering on No changed nothing");

    // Escape puts the question away the same way No does.
    app.ask_delete();
    app.dismiss();
    assert!(matches!(app.mode, Mode::Browsing));
    assert_eq!(app.cards[0].entries().len(), before);
}

/// Two cards, opened on copies so a test that writes never touches what is vendored.
fn two_cards(tag: &str) -> App {
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    let mut cards = Vec::new();
    for (name, from) in [
        ("a", "Gran Turismo/DuckStation/shared_card_1.mcd"),
        ("b", "Castlevania Symphony of the Night/DuckStation/shared_card_2.mcd"),
    ] {
        let to = std::env::temp_dir().join(format!("1cards-{tag}-{name}.mcd"));
        std::fs::copy(ps1.join(from), &to).expect("copies the fixture");
        cards.push(Card::open(&to).expect("opens"));
    }
    App::new(cards, picker())
}

/// A copy lands on the other card, so that is the card the keys then act on.
///
/// Leaving the focus behind is how `w` ends up writing the card that did not change and reporting
/// that there was nothing to write — which is what this is here to stop happening again.
#[test]
fn a_copy_takes_the_focus_with_it_so_the_next_write_is_the_right_card() {
    let mut app = two_cards("focus");
    assert_eq!(app.focus, 0);

    app.on_key(KeyCode::Char('c'));
    assert_eq!(app.focus, 1, "the focus follows the save to where it landed");
    assert!(app.cards[1].dirty(), "and that is the card with something to write");

    app.on_key(KeyCode::Char('w'));
    assert!(matches!(app.mode, Mode::Confirming(_)), "so w asks about writing it");
    app.on_key(KeyCode::Enter);
    assert_eq!(app.status.as_deref(), Some("written"));
    assert!(!app.cards[1].dirty());
}

/// Asking to write a card with nothing to write says where the changes actually are.
#[test]
fn writing_the_wrong_card_says_which_one_has_the_changes() {
    let mut app = two_cards("elsewhere");
    app.on_key(KeyCode::Char('c'));
    // Back to the card that did not change.
    app.on_key(KeyCode::Tab);
    app.on_key(KeyCode::Char('w'));

    let said = app.status.as_deref().expect("it says something");
    assert!(said.contains("nothing to write here"), "{said}");
    assert!(said.contains(".mcd"), "and names the card that does have changes: {said}");
    assert!(matches!(app.mode, Mode::Browsing), "without asking about anything");
}

/// Quitting with work in hand opens on No, unlike every other question.
#[test]
fn the_quit_question_opens_on_no() {
    let mut app = two_cards("quit");
    app.on_key(KeyCode::Char('c'));

    app.on_key(KeyCode::Char('q'));
    match &app.mode {
        Mode::Confirming(confirm) => {
            assert_eq!(confirm.pending, Pending::Quit);
            assert!(!confirm.yes, "the safe answer is the one under the cursor here");
        }
        Mode::Browsing => panic!("it should have asked"),
    }
    // So Enter without looking keeps the work rather than dropping it.
    app.on_key(KeyCode::Enter);
    assert!(!app.done, "answering without moving does not quit");
    assert!(app.cards[1].dirty(), "and the work is still there");

    // Escape is the No button too.
    app.on_key(KeyCode::Char('q'));
    app.on_key(KeyCode::Esc);
    assert!(!app.done);
    assert!(matches!(app.mode, Mode::Browsing), "and the question is put away");
}

/// Every other question opens on Yes, since the thing asked about is the thing just asked for.
#[test]
fn the_other_questions_open_on_yes() {
    let mut app = two_cards("yes");
    for key in ['d', 'w'] {
        if key == 'w' {
            app.cards[app.focus].remove(0).expect("something to write");
        }
        app.on_key(KeyCode::Char(key));
        match &app.mode {
            Mode::Confirming(confirm) => assert!(confirm.yes, "{key} opens on Yes"),
            Mode::Browsing => panic!("{key} should have asked"),
        }
        app.on_key(KeyCode::Esc);
    }
}

/// The question box is as tall as what it holds, and the buttons are always in it.
///
/// A box built to fit a short question puts its buttons past its own bottom edge as soon as a path
/// long enough to wrap turns up — which is every real path, and which left no way to answer.
#[test]
fn a_question_that_wraps_still_shows_its_buttons() {
    // A path long enough to wrap at any width worth drawing.
    let deep = std::env::temp_dir().join("Library/Application Support/DuckStation/memcards");
    std::fs::create_dir_all(&deep).expect("a deep directory");
    let target = deep.join("shared_card_1.mcd");
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    std::fs::copy(ps1.join("Gran Turismo/DuckStation/shared_card_1.mcd"), &target)
        .expect("copies the fixture");

    let mut app = App::new(vec![Card::open(&target).expect("opens")], picker());
    app.cards[0].remove(0).expect("something to write");
    app.on_key(KeyCode::Char('w'));

    for (width, height) in [(46, 20), (80, 20), (120, 40)] {
        let drawn = screen(&mut app, width, height);
        assert!(drawn.contains("Write over"), "the question is there at {width}x{height}");
        assert!(drawn.contains("Yes"), "and so is Yes at {width}x{height}:\n{drawn}");
        assert!(drawn.contains("No"), "and No at {width}x{height}");
        // The box closes, so nothing it holds fell off the bottom.
        assert!(drawn.contains('╚'), "the box closes at {width}x{height}:\n{drawn}");

        // The buttons sit inside the box rather than past its last line.
        let bottom = drawn.lines().position(|line| line.contains('╚')).expect("a bottom");
        let buttons = drawn.lines().position(|line| line.contains("Yes")).expect("buttons");
        assert!(buttons < bottom, "buttons are inside the box at {width}x{height}");
    }
}

/// The block map takes as many rows as its blocks need, rather than one line and a shrug.
#[test]
fn the_block_map_wraps_onto_as_many_rows_as_it_needs() {
    let ps1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1");
    let mut app = App::new(
        vec![Card::open(ps1.join("Gran Turismo/DuckStation/shared_card_1.mcd")).expect("opens")],
        picker(),
    );

    // Wide enough for all sixteen blocks side by side.
    let drawn = screen(&mut app, 100, 24);
    let cells = |row: &str| row.matches('█').count() + row.matches('▓').count() + row.matches('·').count();
    assert_eq!(cells(&map_row(&drawn)), 16, "one line holds them all when there is room");

    // Narrow enough that they cannot be, where they wrap instead of being dropped. The panel's
    // own title is cut at this width, so the map is found by its first cell rather than by text.
    let drawn = screen(&mut app, 34, 24);
    let at = drawn.lines().position(|line| line.contains('█')).expect("the selected save's blocks");
    let rows: Vec<&str> = drawn.lines().skip(at).take_while(|line| cells(line) > 0).collect();
    assert!(rows.len() >= 2, "the blocks run over more than one row:\n{drawn}");
    assert_eq!(
        rows.iter().map(|row| cells(row)).sum::<usize>(),
        16,
        "and every one of them is drawn:\n{drawn}"
    );
}

/// A card too big to draw one cell per block still shows which save is selected.
///
/// A PS2 card has eight thousand blocks. Drawn as a bar it says how full the card is and nothing
/// about what is on it, so moving the selection changed nothing on screen.
#[test]
fn a_large_card_scales_its_map_rather_than_giving_up_on_it() {
    let mut app = App::new(vec![card()], picker());

    let first = screen(&mut app, 84, 24);
    assert!(first.contains("a cell"), "the map says what a cell stands for:\n{first}");
    let selected = |drawn: &str| {
        drawn.lines().find(|line| line.contains('█')).map(ToOwned::to_owned).expect("a selection")
    };
    let before = selected(&first);

    app.step(1);
    let after = selected(&screen(&mut app, 84, 24));
    assert_ne!(before, after, "and moving the selection moves what is filled in");

    // Every cell is one of the three: occupied, selected, or free.
    let row = before.trim_matches(|c| c != '█' && c != '▓' && c != '·');
    assert!(row.chars().all(|c| matches!(c, '█' | '▓' | '·')), "{row}");
}
