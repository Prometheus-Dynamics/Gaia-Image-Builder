//! The TUI's visual language: one palette of terminal colours with a meaning
//! each (ok, running, failed, ...), the icons that go with them, and the
//! rounded frame every panel uses. Colours are the 16 basic ones so the
//! terminal's own theme decides how they look.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};

pub(crate) const ICON_PENDING: &str = "○";
pub(crate) const ICON_RUNNING: &str = "↻";
pub(crate) const ICON_OK: &str = "✓";
pub(crate) const ICON_REUSED: &str = "↺";
pub(crate) const ICON_FAILED: &str = "✗";
pub(crate) const ICON_CANCELLED: &str = "⊘";
pub(crate) const ICON_SKIPPED: &str = "–";
pub(crate) const ICON_PAUSED: &str = "⏸";
pub(crate) const ICON_PLAY: &str = "▶";

/// Success.
pub(crate) fn ok() -> Style {
    Style::default().fg(Color::Green)
}

/// Reused from an earlier run.
pub(crate) fn reused() -> Style {
    Style::default().fg(Color::LightBlue)
}

/// Executing right now.
pub(crate) fn running() -> Style {
    Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD)
}

/// Failed.
pub(crate) fn failed() -> Style {
    Style::default()
        .fg(Color::LightRed)
        .add_modifier(Modifier::BOLD)
}

/// Cancelled or slower than expected: needs a look, not an alarm.
pub(crate) fn warn() -> Style {
    Style::default().fg(Color::LightYellow)
}

/// Paused.
pub(crate) fn paused() -> Style {
    Style::default().fg(Color::LightMagenta)
}

/// Not started, skipped, or secondary text.
pub(crate) fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Headings and the keys in hints.
pub(crate) fn accent() -> Style {
    Style::default()
        .fg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
}

/// The row or item under the cursor.
pub(crate) fn selected() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
}

/// A panel's frame.
pub(crate) fn border() -> Style {
    Style::default().fg(Color::Gray)
}

/// A bordered panel with a rounded frame and a title.
pub(crate) fn panel<'a>(title: impl Into<ratatui::text::Line<'a>>) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
}
