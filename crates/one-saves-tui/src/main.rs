//! `1cards` — a terminal explorer for memory cards.
//!
//! Open one card to look through it, or two to move saves between them. A card that is not there
//! yet is started empty, to copy saves onto. Reading is safe; the things that are not — deleting
//! a save, formatting a card, writing one back — ask first, and a write goes to a temporary file
//! that replaces the original only once the rebuilt card has been read back.

use std::io::stdout;
use std::process::ExitCode;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::Picker;

use one_saves_tui::app::App;
use one_saves_tui::model::Card;

const USAGE: &str = "\
1cards <card> [card]

Opens one memory card, or two so saves can be moved between them. Any format
one-saves-convert reads: PS1, PS2, GameCube, Nintendo 64, Neo Geo, Dreamcast.

A card may be inside a .zip, and so may loose PlayStation saves — a card is
built to hold those. Either way the archive is read-only, marked [ro]: copy
the saves onto a card of your own and write that.

A card that does not exist yet is started empty, marked [new], and written
when you press w. Its name says what kind: .mcr, .ps2, .raw, .mpk or .neo.
Opened beside a card, it is that card's kind and size unless its name says
otherwise, so `1cards card.ps2 copy.ps2` makes an empty card to copy onto.
";

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() || paths.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if paths.len() > 2 {
        eprintln!("1cards: two cards at most");
        return ExitCode::FAILURE;
    }

    // The cards that are there first, so one that is not can be started like one that is.
    let mut opened = Vec::new();
    for path in &paths {
        if !std::path::Path::new(path).exists() {
            opened.push(None);
            continue;
        }
        match Card::open(path) {
            Ok(card) => opened.push(Some(card)),
            Err(error) => {
                eprintln!("1cards: {path}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    for at in 0..opened.len() {
        if opened[at].is_some() {
            continue;
        }
        match Card::create(&paths[at], opened.iter().flatten().next()) {
            Ok(card) => opened[at] = Some(card),
            Err(error) => {
                eprintln!("1cards: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    let cards = opened.into_iter().flatten().collect();

    match run(cards) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("1cards: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cards: Vec<Card>) -> std::io::Result<()> {
    // Asking the terminal what it can draw has to happen before the alternate screen, since it is
    // a question and an answer on the same stdio this is about to take over.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = loop_over(&mut terminal, App::new(cards, picker));

    // Put the terminal back whatever happened, so an error does not leave a broken shell.
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn loop_over<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, mut app: App) -> std::io::Result<()> {
    while !app.done {
        terminal.draw(|frame| app.draw(frame))?;

        // An animated icon has to redraw without anything being pressed, so the wait is however
        // long the showing frame has left. A still icon has nothing to wait for, and blocking on
        // the key is what keeps an idle card from spinning the CPU.
        if let Some(left) = app.next_frame_in()
            && !event::poll(left)?
        {
            app.tick();
            continue;
        }

        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        app.on_key(key.code);
    }
    Ok(())
}
