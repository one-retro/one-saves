//! The interface: what is on screen, and what a key does to it.
//!
//! Everything that changes a card lives in [`crate::model`]. What is here is selection, layout and
//! the one question worth asking twice, so that the rules can be tested without a terminal and the
//! drawing can be read without the rules in the way.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::model::Card;

/// What the interface is waiting for.
pub enum Mode {
    /// Browsing.
    Browsing,
    /// Asking before something irreversible, with the question to show.
    Confirming(String, Pending),
    /// Saying what just happened, until the next keypress.
    Reporting(String),
}

/// An action held back until it is confirmed.
#[derive(Clone, Copy)]
pub enum Pending {
    /// Delete the selected save from the focused card.
    Delete,
    /// Write the focused card back over the file it came from.
    Save,
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
    /// Whether to leave.
    pub done: bool,
    /// How the terminal draws pictures, decided once at startup.
    picker: Picker,
    /// The decoded icon for what is showing, kept because encoding it is not free. Keyed by the
    /// card, the save and the frame, so any of the three changing redraws it.
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
    ///
    /// This is what the event loop waits on: with one frame there is nothing to wait for and a
    /// key is the only thing that can change the screen, so it blocks instead of spinning.
    #[must_use]
    pub fn next_frame_in(&self) -> Option<Duration> {
        let frames = self.showing()?;
        if frames.len() < 2 {
            return None;
        }
        let hold =
            Duration::from_millis(frames[self.frame % frames.len()].hold_ms.unwrap_or(DEFAULT_HOLD_MS));
        Some(hold.saturating_sub(self.shown_at.elapsed()))
    }

    /// Advances the icon if the showing frame has had its time.
    pub fn tick(&mut self) {
        let Some(frames) = self.showing() else { return };
        if frames.len() < 2 {
            return;
        }
        let hold =
            Duration::from_millis(frames[self.frame % frames.len()].hold_ms.unwrap_or(DEFAULT_HOLD_MS));
        if self.shown_at.elapsed() >= hold {
            self.frame = (self.frame + 1) % frames.len();
            self.shown_at = Instant::now();
        }
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
        self.mode = Mode::Confirming(
            format!("Delete {:?}? It occupies {} blocks.", entry.title, entry.blocks),
            Pending::Delete,
        );
    }

    /// Asks before writing, since writing is what makes an edit real.
    pub fn ask_save(&mut self) {
        if !self.card().dirty() {
            self.mode = Mode::Reporting("nothing to write".into());
            return;
        }
        let name = self.card().path.display().to_string();
        self.mode = Mode::Confirming(format!("Write {name}?"), Pending::Save);
    }

    /// Copies the selected save onto the other card.
    pub fn copy(&mut self) {
        if self.cards.len() < 2 {
            self.mode = Mode::Reporting("open a second card to copy onto".into());
            return;
        }
        let Some(index) = self.selected() else { return };
        let (from, to) = (self.focus, (self.focus + 1) % self.cards.len());
        // Two cards at once, which the borrow checker will not hand out, so the part is taken
        // through a split rather than by holding both.
        let (left, right) = self.cards.split_at_mut(from.max(to));
        let (source, target) =
            if from < to { (&left[from], &mut right[0]) } else { (&right[0], &mut left[to]) };
        self.mode = match target.copy_from(source, index) {
            Ok(()) => Mode::Reporting("copied".into()),
            Err(e) => Mode::Reporting(e.to_string()),
        };
    }

    /// Carries out what was confirmed.
    pub fn confirm(&mut self, pending: Pending) {
        self.mode = match pending {
            Pending::Delete => {
                let Some(index) = self.selected() else { return };
                match self.cards[self.focus].remove(index) {
                    Ok(()) => {
                        self.icon = None;
                        self.step(0);
                        Mode::Reporting("deleted; not written yet".into())
                    }
                    Err(e) => Mode::Reporting(e.to_string()),
                }
            }
            Pending::Save => match self.cards[self.focus].save() {
                Ok(()) => Mode::Reporting("written".into()),
                Err(e) => Mode::Reporting(e.to_string()),
            },
        };
    }

    /// Draws everything.
    pub fn draw(&mut self, frame: &mut Frame) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(frame.area());
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(rows[0]);

        self.draw_cards(frame, columns[0]);
        self.draw_detail(frame, columns[1]);
        self.draw_status(frame, rows[1]);
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
            // The edit marker leads, because a long title is cut from the right and whether a
            // card has unwritten changes is the last thing that should disappear.
            let title = format!(
                " {}{} · {}/{} free ",
                if card.dirty() { "● " } else { "" },
                card.path.file_name().unwrap_or_default().to_string_lossy(),
                card.free_blocks(),
                card.blocks(),
            );
            // The block column is fixed and the name takes what is left, so a narrow pane loses
            // characters from a name rather than losing the number beside it.
            let name_width = usize::from(area.width).saturating_sub(2 + BLOCK_COLUMN).max(4);
            let items: Vec<ListItem> = card
                .entries()
                .iter()
                .map(|entry| {
                    ListItem::new(Line::from(vec![
                        Span::raw(fit(&entry.title, name_width)),
                        Span::styled(
                            format!("{:>4} blk", entry.blocks),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(title).border_style(if focused {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                }))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(list, *area, &mut self.selection[index]);
        }
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" save ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(index) = self.selected() else { return };
        let entries = self.card().entries();
        let Some(entry) = entries.get(index) else { return };

        // The icon arrives already decoded to PNG, so nothing here knows what RGB5A3 is. A save
        // that carries none — every PS2 save, whose icons are parts of their own — gets the room
        // back rather than a gap where a picture would have been.
        let showing = if entry.icon.is_empty() { 0 } else { self.frame % entry.icon.len() };
        if self.icon.as_ref().map(|(card, save, at, _)| (*card, *save, *at))
            != Some((self.focus, index, showing))
        {
            self.icon =
                entry.icon.get(showing).and_then(|icon| image::load_from_memory(&icon.png).ok()).map(
                    |small| (self.focus, index, showing, self.picker.new_resize_protocol(blow_up(&small))),
                );
        }
        // A console icon is 16x16 or 32x32, which at one cell per pixel is a smudge. The area is
        // sized in cells to come out near `ICON_PIXELS` on a side, and `Resize::Fit` scales into
        // it with a nearest-neighbour filter, keeping the proportions and the hard pixel edges.
        let (cell_width, cell_height) = self.picker.font_size();
        let wanted = Rect {
            width: ICON_PIXELS.div_ceil(cell_width.max(1)).min(inner.width),
            height: ICON_PIXELS.div_ceil(cell_height.max(1)).min(inner.height.saturating_sub(4)),
            ..inner
        };
        let picture = if self.icon.is_some() { wanted.height } else { 0 };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(picture), Constraint::Min(0)])
            .split(inner);
        if let Some((_, _, _, protocol)) = self.icon.as_mut() {
            let area = Rect { width: wanted.width, ..rows[0] };
            frame.render_stateful_widget(StatefulImage::default(), area, protocol);
        }

        let mut lines = vec![Line::from(entry.title.clone())];
        if let Some(detail) = &entry.detail {
            lines.push(Line::styled(detail.clone(), Style::default().fg(Color::DarkGray)));
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
        let (text, style) = match &self.mode {
            Mode::Browsing => (
                "↑↓ select   tab card   c copy   d delete   w write   q quit".to_owned(),
                Style::default().fg(Color::DarkGray),
            ),
            Mode::Confirming(question, _) => {
                (format!("{question}  [y/n]"), Style::default().fg(Color::Yellow))
            }
            Mode::Reporting(what) => (what.clone(), Style::default().fg(Color::Cyan)),
        };
        frame.render_widget(
            Paragraph::new(text).style(style).block(Block::default().borders(Borders::ALL)),
            area,
        );
    }
}

/// How long a frame shows where the format keeps no timing of its own.
///
/// A PlayStation leaves the rate to the console rather than writing one per frame, so a reader has
/// to pick something; this is about what the hardware runs at, and what a GameCube asks for in
/// both of the cards under `data/saves`.
const DEFAULT_HOLD_MS: u64 = 250;

/// Scales a console icon up to something a person can see, by a whole number of pixels.
///
/// A whole number matters: at 8x every source pixel is an 8x8 square, where at 7.5x some are eight
/// wide and some seven, and a 16x16 picture drawn that way looks subtly wrong in a way that reads
/// as a bad decoder rather than as a scaled image. Nearest-neighbour for the same reason — these
/// are hard-edged pixels and smoothing them would invent colours the card does not hold.
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

/// How large a save's picture is drawn, on its longest side in pixels.
///
/// Blowing it up is a display choice and not a change to the picture. What `x.1sav.icon` carries
/// is what the card holds, at the size the console stored it; this is only how large it is shown,
/// the way a console shows a 16x16 icon at a size a person can see.
const ICON_PIXELS: u16 = 128;

/// What the block count column takes, which the name gets what is left of.
const BLOCK_COLUMN: usize = 8;

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
        // One cell held back for the ellipsis, so a cut name reads as cut.
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
