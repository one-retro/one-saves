//! The interface: what is on screen, and what a key does to it.
//!
//! Everything that changes a card lives in [`crate::model`]. What is here is selection, layout and
//! the questions worth asking twice, so the rules can be tested without a terminal and the drawing
//! can be read without the rules in the way.

use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::model::Card;

/// What the interface is waiting for.
pub enum Mode {
    /// Browsing, which is every key doing what it says.
    Browsing,
    /// Waiting on an answer to something that cannot be undone.
    Confirming(Confirm),
}

/// A question, and what it is holding back.
pub struct Confirm {
    /// What is being asked.
    pub question: String,
    /// What the person answering should know before they do.
    pub warning: Option<String>,
    /// What happens on yes.
    pub pending: Pending,
    /// Which button is under the cursor. Yes leads: the question is only asked because a key was
    /// pressed asking for the thing, so the answer it opens on is the one that was just asked for.
    pub yes: bool,
}

impl Confirm {
    /// A question, waiting on its answer with Yes under the cursor.
    fn new(question: String, warning: Option<String>, pending: Pending) -> Self {
        Self { question, warning, pending, yes: true }
    }

    /// The same, opening on No.
    ///
    /// For the one question whose default answer is not its safe one: everywhere else the thing
    /// being asked about is the thing just asked for, and losing unwritten work is not that.
    fn cautious(question: String, warning: Option<String>, pending: Pending) -> Self {
        Self { yes: false, ..Self::new(question, warning, pending) }
    }
}

/// An action held back until it is confirmed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pending {
    /// Delete the selected save from the focused card.
    Delete,
    /// Write the focused card back over the file it came from.
    Write,
    /// Copy the selected save onto the other card, over one already there.
    CopyOver,
    /// Leave, abandoning whatever has not been written.
    Quit,
}

/// The whole of the interface's state.
pub struct App {
    /// The open cards, at most two, so a save has somewhere to go.
    pub cards: Vec<Card>,
    /// Which card the keys act on.
    pub focus: usize,
    /// Which save is selected, one per card so switching does not lose the place.
    selection: Vec<ListState>,
    /// What is being waited for.
    pub mode: Mode,
    /// What just happened, shown until something else happens. Never swallows a key.
    pub status: Option<String>,
    /// Whether to leave.
    pub done: bool,
    /// How the terminal draws pictures, decided once at startup.
    picker: Picker,
    /// The decoded icon for what is showing, keyed by card, save and frame.
    icon: Option<(usize, usize, usize, StatefulProtocol)>,
    /// Which frame of the selected save's icon is showing.
    frame: usize,
    /// When it started showing, which is what the next one waits on.
    shown_at: Instant,
}

impl App {
    /// Opens the interface over the given cards.
    #[must_use]
    pub fn new(cards: Vec<Card>, picker: Picker) -> Self {
        let selection = cards
            .iter()
            .map(|card| {
                let mut state = ListState::default();
                if !card.entries().is_empty() {
                    state.select(Some(0));
                }
                state
            })
            .collect();
        Self {
            cards,
            focus: 0,
            selection,
            mode: Mode::Browsing,
            status: None,
            done: false,
            picker,
            icon: None,
            frame: 0,
            shown_at: Instant::now(),
        }
    }

    /// The card the keys act on.
    fn card(&self) -> &Card {
        &self.cards[self.focus]
    }

    /// Which save is selected on the focused card.
    fn selected(&self) -> Option<usize> {
        self.selection[self.focus].selected()
    }

    /// Whether anything is waiting to be written.
    #[must_use]
    pub fn unwritten(&self) -> bool {
        self.cards.iter().any(Card::dirty)
    }

    /// Moves the selection, clamped to what is there.
    pub fn step(&mut self, delta: isize) {
        let count = self.card().entries().len();
        if count == 0 {
            self.selection[self.focus].select(None);
            return;
        }
        let state = &mut self.selection[self.focus];
        let at = state.selected().unwrap_or(0).saturating_add_signed(delta);
        state.select(Some(at.min(count - 1)));
        self.rewind();
    }

    /// Moves focus to the other card, where there is one.
    pub fn switch(&mut self) {
        if self.cards.len() > 1 {
            self.focus = (self.focus + 1) % self.cards.len();
            self.rewind();
        }
    }

    /// Starts the selected save's icon again from its first frame.
    fn rewind(&mut self) {
        self.icon = None;
        self.frame = 0;
        self.shown_at = Instant::now();
    }

    /// How long the showing frame has left, or `None` where nothing is animating.
    #[must_use]
    pub fn next_frame_in(&self) -> Option<Duration> {
        let frames = self.showing()?;
        if frames.len() < 2 {
            return None;
        }
        Some(self.hold(&frames).saturating_sub(self.shown_at.elapsed()))
    }

    /// Advances the icon if the showing frame has had its time.
    pub fn tick(&mut self) {
        let Some(frames) = self.showing() else { return };
        if frames.len() < 2 {
            return;
        }
        if self.shown_at.elapsed() >= self.hold(&frames) {
            self.frame = (self.frame + 1) % frames.len();
            self.shown_at = Instant::now();
        }
    }

    /// How long the showing frame asks for, or the default where the format states none.
    fn hold(&self, frames: &[crate::model::IconFrame]) -> Duration {
        let at = self.frame % frames.len();
        Duration::from_millis(frames[at].hold_ms.unwrap_or(DEFAULT_HOLD_MS))
    }

    /// The frames of the selected save's icon.
    fn showing(&self) -> Option<Vec<crate::model::IconFrame>> {
        let index = self.selection[self.focus].selected()?;
        let mut entries = self.cards[self.focus].entries();
        (index < entries.len()).then(|| std::mem::take(&mut entries[index].icon))
    }

    /// Asks before deleting, because nothing here is undoable once written.
    pub fn ask_delete(&mut self) {
        let Some(index) = self.selected() else { return };
        let entries = self.card().entries();
        let Some(entry) = entries.get(index) else { return };
        self.mode = Mode::Confirming(Confirm::new(
            format!("Delete {}?", entry.title),
            Some(format!("{} blocks are freed. Nothing is written until you press w.", entry.blocks)),
            Pending::Delete,
        ));
    }

    /// Asks before writing, since writing is what makes an edit real.
    pub fn ask_write(&mut self) {
        if !self.card().dirty() {
            let elsewhere: Vec<String> = self
                .cards
                .iter()
                .enumerate()
                .filter(|(at, card)| *at != self.focus && card.dirty())
                .map(|(_, card)| card.path.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .collect();
            self.status = Some(if elsewhere.is_empty() {
                "nothing to write".into()
            } else {
                format!("nothing to write here — {} has the changes", elsewhere.join(", "))
            });
            return;
        }
        self.mode = Mode::Confirming(Confirm::new(
            format!("Write over {}?", self.card().path.display()),
            Some("The file is replaced by what is listed here.".into()),
            Pending::Write,
        ));
    }

    /// Asks before leaving with work in hand.
    pub fn ask_quit(&mut self) {
        if !self.unwritten() {
            self.done = true;
            return;
        }
        let cards: Vec<String> = self
            .cards
            .iter()
            .filter(|card| card.dirty())
            .map(|card| card.path.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        self.mode = Mode::Confirming(Confirm::cautious(
            "Quit without writing?".into(),
            Some(format!("{} has changes that will be lost.", cards.join(", "))),
            Pending::Quit,
        ));
    }

    /// Copies the selected save onto the other card.
    ///
    /// Asks only where it would land on top of a save already there. Adding one is not a question:
    /// it takes space and nothing else, and a copy that will not fit is refused with the numbers.
    pub fn copy(&mut self) {
        if self.cards.len() < 2 {
            self.status = Some("open a second card to copy onto".into());
            return;
        }
        let Some(index) = self.selected() else { return };
        let entries = self.card().entries();
        let Some(entry) = entries.get(index) else { return };

        let other = (self.focus + 1) % self.cards.len();
        if self.cards[other].entries().iter().any(|there| there.title == entry.title) {
            let title = entry.title.clone();
            self.mode = Mode::Confirming(Confirm::new(
                format!("{title} is already on the other card. Copy anyway?"),
                Some("You will have two saves under one name.".into()),
                Pending::CopyOver,
            ));
            return;
        }
        self.do_copy();
    }

    /// Carries the copy out, whether or not it was asked about.
    fn do_copy(&mut self) {
        let Some(index) = self.selected() else { return };
        let (from, to) = (self.focus, (self.focus + 1) % self.cards.len());
        let (left, right) = self.cards.split_at_mut(from.max(to));
        let (source, target) =
            if from < to { (&left[from], &mut right[0]) } else { (&right[0], &mut left[to]) };
        match target.copy_from(source, index) {
            Ok(()) => {
                // Follow the save to where it landed. Leaving the focus behind is how `w` ends up
                // writing the card that did not change, and reporting that there was nothing to.
                self.focus = to;
                self.rewind();
                self.step(0);
                self.status = Some("copied; press w to write it".into());
            }
            Err(error) => self.status = Some(error.to_string()),
        }
    }

    /// What a key does, which is all of it, so the loop around this is only a pump.
    ///
    /// Keeping it here rather than in the event loop is what lets a test press keys: the bug that
    /// prompted the move was one no test could reach, because the only thing that knew what Enter
    /// meant was a `match` inside a function that needed a terminal.
    pub fn on_key(&mut self, key: KeyCode) {
        // A question is answered by pressing a button, and nothing else reaches past it.
        if matches!(self.mode, Mode::Confirming(_)) {
            match key {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => self.toggle(),
                // The letters move the cursor rather than answering outright, so the key that
                // commits is the same one however you got to the button.
                KeyCode::Char('y' | 'Y') => self.point_at(true),
                KeyCode::Char('n' | 'N') => self.point_at(false),
                KeyCode::Enter | KeyCode::Char(' ') => self.answer(),
                // Escape is the No button, not a third answer.
                KeyCode::Esc => self.dismiss(),
                _ => {}
            }
            return;
        }

        // What was said last stops being news the moment anything else happens, but it never
        // swallows the key that happened: a report is something to read, not something to dismiss.
        self.status = None;
        match key {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => self.ask_quit(),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            // Both directions do the same thing with two cards, and a person reaching for
            // shift-tab is asking for the other one either way.
            KeyCode::Tab | KeyCode::BackTab => self.switch(),
            KeyCode::Char('c') => self.copy(),
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Char('w') => self.ask_write(),
            _ => {}
        }
    }

    /// Moves between the buttons.
    pub fn toggle(&mut self) {
        if let Mode::Confirming(confirm) = &mut self.mode {
            confirm.yes = !confirm.yes;
        }
    }

    /// Puts the cursor on one button outright, which is what y and n do.
    pub fn point_at(&mut self, yes: bool) {
        if let Mode::Confirming(confirm) = &mut self.mode {
            confirm.yes = yes;
        }
    }

    /// Answers with whichever button the cursor is on.
    pub fn answer(&mut self) {
        let Mode::Confirming(confirm) = &self.mode else { return };
        let (yes, pending) = (confirm.yes, confirm.pending);
        if yes {
            self.confirm(pending);
        } else {
            self.dismiss();
        }
    }

    /// Puts a question away without doing what it asked.
    pub fn dismiss(&mut self) {
        self.mode = Mode::Browsing;
        self.status = None;
    }

    /// Carries out what was confirmed.
    pub fn confirm(&mut self, pending: Pending) {
        self.mode = Mode::Browsing;
        match pending {
            Pending::Delete => {
                let Some(index) = self.selected() else { return };
                self.status = Some(match self.cards[self.focus].remove(index) {
                    Ok(()) => {
                        self.rewind();
                        self.step(0);
                        "deleted; press w to write".into()
                    }
                    Err(error) => error.to_string(),
                });
            }
            Pending::Write => {
                self.status = Some(match self.cards[self.focus].save() {
                    Ok(()) => "written".into(),
                    Err(error) => error.to_string(),
                });
            }
            Pending::CopyOver => self.do_copy(),
            Pending::Quit => self.done = true,
        }
    }

    /// Draws everything.
    pub fn draw(&mut self, frame: &mut Frame) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(frame.area());
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(rows[0]);

        self.draw_cards(frame, columns[0]);
        self.draw_side(frame, columns[1]);
        self.draw_status(frame, rows[1]);

        if let Mode::Confirming(_) = &self.mode {
            self.draw_modal(frame, frame.area());
        }
    }

    fn draw_cards(&mut self, frame: &mut Frame, area: Rect) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![
                Constraint::Ratio(1, u32::try_from(self.cards.len()).unwrap_or(1));
                self.cards.len()
            ])
            .split(area);

        for (index, area) in areas.iter().enumerate() {
            let card = &self.cards[index];
            let focused = index == self.focus;
            // The marker leads, because a long title is cut from the right and whether a card has
            // unwritten changes is the last thing that should disappear.
            let title = format!(
                " {}{}{} · {}/{} free ",
                if focused { "▶ " } else { "  " },
                if card.dirty() { "● " } else { "" },
                card.path.file_name().unwrap_or_default().to_string_lossy(),
                card.free_blocks(),
                card.blocks(),
            );
            let name_width = usize::from(area.width).saturating_sub(2 + BLOCK_COLUMN).max(4);
            let items: Vec<ListItem> = card
                .entries()
                .iter()
                .enumerate()
                .map(|(at, entry)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(fit(&entry.title, name_width), Style::default().fg(hue(at))),
                        Span::styled(
                            format!("{:>4} blk", entry.blocks),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                })
                .collect();

            let border = if focused { Color::Cyan } else { Color::DarkGray };
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(if focused { BorderType::Thick } else { BorderType::Plain })
                        .border_style(Style::default().fg(border))
                        .title(Span::styled(
                            title,
                            Style::default().fg(border).add_modifier(if focused {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                        )),
                )
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(list, *area, &mut self.selection[index]);
        }
    }

    fn draw_side(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(0)])
            .split(area);
        self.draw_map(frame, rows[0]);
        self.draw_detail(frame, rows[1]);
    }

    /// The focused card's blocks, one cell each, coloured by what occupies them.
    ///
    /// The order is the directory's rather than the card's physical layout: a save's blocks are
    /// not recorded once it is read, and packing them in listed order is what writing the card
    /// back does anyway. A card with more blocks than there is room for gets a bar instead.
    fn draw_map(&self, frame: &mut Frame, area: Rect) {
        let card = self.card();
        let block = Block::default().borders(Borders::ALL).title(format!(
            " {} of {} blocks used ",
            card.used_blocks(),
            card.blocks()
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let entries = card.entries();
        let total = card.blocks();
        let room = u64::from(inner.width) * u64::from(inner.height);

        if total == 0 || total > room {
            // Too many to draw one each, so the same information as a bar.
            let filled = card
                .used_blocks()
                .checked_mul(u64::from(inner.width))
                .and_then(|scaled| scaled.checked_div(total))
                .and_then(|at| usize::try_from(at).ok())
                .unwrap_or(0);
            let bar: String =
                (0..inner.width as usize).map(|at| if at < filled { '▓' } else { '░' }).collect();
            frame.render_widget(Paragraph::new(bar).style(Style::default().fg(Color::Cyan)), inner);
            return;
        }

        let selected = self.selected();
        let mut cells: Vec<Span> = Vec::new();
        for (at, entry) in entries.iter().enumerate() {
            let style = Style::default().fg(hue(at)).add_modifier(if Some(at) == selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
            for _ in 0..entry.blocks {
                cells.push(Span::styled(if Some(at) == selected { "█" } else { "▓" }, style));
            }
        }
        for _ in 0..card.free_blocks() {
            cells.push(Span::styled("·", Style::default().fg(Color::DarkGray)));
        }

        let width = inner.width as usize;
        let lines: Vec<Line> = cells.chunks(width.max(1)).map(|row| Line::from(row.to_vec())).collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" save ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(index) = self.selected() else { return };
        let entries = self.card().entries();
        let Some(entry) = entries.get(index) else { return };

        let showing = if entry.icon.is_empty() { 0 } else { self.frame % entry.icon.len() };
        if self.icon.as_ref().map(|(card, save, at, _)| (*card, *save, *at))
            != Some((self.focus, index, showing))
        {
            self.icon =
                entry.icon.get(showing).and_then(|icon| image::load_from_memory(&icon.png).ok()).map(
                    |small| (self.focus, index, showing, self.picker.new_resize_protocol(blow_up(&small))),
                );
        }

        let (cell_width, cell_height) = self.picker.font_size();
        let picture = if self.icon.is_some() {
            ICON_PIXELS.div_ceil(cell_height.max(1)).min(inner.height.saturating_sub(4))
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(picture), Constraint::Min(0)])
            .split(inner);
        if let Some((_, _, _, protocol)) = self.icon.as_mut() {
            let area = Rect { width: ICON_PIXELS.div_ceil(cell_width.max(1)).min(inner.width), ..rows[0] };
            frame.render_stateful_widget(StatefulImage::default(), area, protocol);
        }

        let mut lines = vec![Line::styled(
            entry.title.clone(),
            Style::default().fg(hue(index)).add_modifier(Modifier::BOLD),
        )];
        if let Some(detail) = &entry.detail {
            lines.push(Line::styled(detail.clone(), Style::default().fg(Color::Gray)));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(format!("{} blocks", entry.blocks), Style::default().fg(Color::DarkGray)));
        if entry.icon.len() > 1 {
            lines.push(Line::styled(
                format!("{} icon frames", entry.icon.len()),
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), rows[1]);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let keys = "↑↓ select   ⇥ card   c copy   d delete   w write   q quit";
        let (text, style) = match &self.status {
            Some(said) => (said.clone(), Style::default().fg(Color::Cyan)),
            None => (keys.to_owned(), Style::default().fg(Color::DarkGray)),
        };
        frame.render_widget(
            Paragraph::new(text).style(style).block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    /// The question, over the top of everything, because it is the only thing that takes a key.
    fn draw_modal(&self, frame: &mut Frame, area: Rect) {
        let Mode::Confirming(confirm) = &self.mode else { return };

        let width = area.width.saturating_sub(8).clamp(20, 64);
        let height = if confirm.warning.is_some() { 7 } else { 6 };
        let box_area = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height: height.min(area.height),
        };

        let mut lines = vec![Line::from(""), Line::from(confirm.question.clone())];
        if let Some(warning) = &confirm.warning {
            lines.push(Line::styled(warning.clone(), Style::default().fg(Color::Yellow)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            button(" Yes ", confirm.yes),
            Span::raw("   "),
            button(" No ", !confirm.yes),
        ]));

        frame.render_widget(Clear, box_area);
        frame.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
            box_area,
        );
    }
}

/// How large a save's picture is drawn, on its longest side in pixels.
///
/// Blowing it up is a display choice and not a change to the picture. What `x.1sav.icon` carries
/// is what the card holds, at the size the console stored it; this is only how large it is shown.
const ICON_PIXELS: u16 = 128;

/// What the block count column takes, which the name gets what is left of.
const BLOCK_COLUMN: usize = 8;

/// How long a frame shows where the format keeps no timing of its own.
const DEFAULT_HOLD_MS: u64 = 250;

/// One of the modal's buttons, filled in where the cursor is on it.
fn button(label: &str, under_cursor: bool) -> Span<'_> {
    let style = if under_cursor {
        Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Span::styled(label, style)
}

/// The colour a save is drawn in, so its name and its blocks are recognisably the same save.
fn hue(index: usize) -> Color {
    const WHEEL: [Color; 6] =
        [Color::Cyan, Color::Green, Color::Magenta, Color::Yellow, Color::Blue, Color::LightRed];
    WHEEL[index % WHEEL.len()]
}

/// Scales a console icon up to something a person can see, by a whole number of pixels.
///
/// A whole number matters: at 8x every source pixel is an 8x8 square, where at 7.5x some are eight
/// wide and some seven, and a 16x16 picture drawn that way looks subtly wrong in a way that reads
/// as a bad decoder rather than as a scaled image. Nearest-neighbour for the same reason.
fn blow_up(image: &image::DynamicImage) -> image::DynamicImage {
    use image::GenericImageView as _;

    let (width, height) = image.dimensions();
    let longest = width.max(height).max(1);
    let scale = (u32::from(ICON_PIXELS) / longest).max(1);
    if scale == 1 {
        return image.clone();
    }
    image.resize_exact(width * scale, height * scale, image::imageops::FilterType::Nearest)
}

/// Fits a title to a column count, padding it out to exactly that.
///
/// Counted in the cells a terminal gives each character rather than in characters: a PS2 browser
/// line is full width, where every character takes two, and a column measured by counting them
/// would run to twice its width and push what follows off the screen.
fn fit(text: &str, columns: usize) -> String {
    use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

    let (mut out, mut used) = (String::new(), 0);
    if text.width() <= columns {
        out.push_str(text);
        used = text.width();
    } else {
        for character in text.chars() {
            let width = character.width().unwrap_or(0);
            if used + width > columns.saturating_sub(1) {
                break;
            }
            out.push(character);
            used += width;
        }
        out.push('…');
        used += 1;
    }
    out + &" ".repeat(columns.saturating_sub(used))
}
