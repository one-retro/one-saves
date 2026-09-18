//! `1cards` — a terminal explorer for memory cards.
//!
//! Open one card to look through it, or two to move saves between them. Reading is safe; the two
//! things that are not — deleting a save and writing a card back — ask first, and a write goes to
//! a temporary file that replaces the original only once the rebuilt card has been read back.

use std::io::stdout;
use std::process::ExitCode;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::Picker;

use one_saves_tui::app::{App, Mode};
use one_saves_tui::model::Card;

const USAGE: &str = "\
1cards <card> [card]

Opens one memory card, or two so saves can be moved between them. Any format
one-saves-convert reads: PS1, PS2, GameCube, Nintendo 64, Neo Geo, Dreamcast.
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

    let mut cards = Vec::new();
    for path in &paths {
        match Card::open(path) {
            Ok(card) => cards.push(card),
            Err(error) => {
                eprintln!("1cards: {path}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

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
        // A question takes yes or no and nothing else, so a stray key cannot delete anything.
        if let Mode::Confirming(confirm) = std::mem::replace(&mut app.mode, Mode::Browsing) {
            match key.code {
                KeyCode::Char('y' | 'Y') => app.confirm(confirm.pending),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => app.status = None,
                // Anything else leaves the question standing rather than answering it.
                _ => app.mode = Mode::Confirming(confirm),
            }
            continue;
        }

        // What was said last stops being news the moment anything else happens, but it never
        // swallows the key that happened: a report is something to read, not something to dismiss.
        app.status = None;
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => app.ask_quit(),
            KeyCode::Up | KeyCode::Char('k') => app.step(-1),
            KeyCode::Down | KeyCode::Char('j') => app.step(1),
            // Both directions do the same thing with two cards, and a person reaching for
            // shift-tab is asking for the other one either way.
            KeyCode::Tab | KeyCode::BackTab => app.switch(),
            KeyCode::Char('c') => app.copy(),
            KeyCode::Char('d') => app.ask_delete(),
            KeyCode::Char('w') => app.ask_write(),
            _ => {}
        }
    }
    Ok(())
}
