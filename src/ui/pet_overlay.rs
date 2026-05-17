//! Pet overlay — renders Mochi's sprite + mood label in a small box at the
//! top-right corner of the chat body. Reads `App.pet_character` and
//! `App.pet_mood`; reactive to events that mutate the mood elsewhere.

use crate::app::App;
use crate::pet::{label, sprite_for};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::theme;

const OVERLAY_WIDTH: u16 = 18;
const OVERLAY_HEIGHT: u16 = 5;
const MIN_BODY_WIDTH: u16 = 50;
const MIN_BODY_HEIGHT: u16 = 8;
const RIGHT_MARGIN: u16 = 1;
const TOP_MARGIN: u16 = 0;

pub fn render(frame: &mut Frame, body: Rect, app: &App) {
    if body.width < MIN_BODY_WIDTH || body.height < MIN_BODY_HEIGHT {
        return;
    }

    let rect = Rect::new(
        body.x + body.width.saturating_sub(OVERLAY_WIDTH + RIGHT_MARGIN),
        body.y + TOP_MARGIN,
        OVERLAY_WIDTH,
        OVERLAY_HEIGHT,
    );

    frame.render_widget(Clear, rect);

    let sprite = sprite_for(app.pet_character, app.pet_mood);
    let mut lines: Vec<Line<'_>> = sprite
        .lines()
        .map(|s| Line::from(Span::styled(s.to_owned(), Style::default().fg(theme::RUST_ORANGE))))
        .collect();
    lines.push(Line::from(Span::styled(
        format!(" {} • {}", app.pet_character.name(), label(app.pet_mood)),
        Style::default().fg(theme::DIM).add_modifier(Modifier::DIM),
    )));

    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(theme::DIM));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}
