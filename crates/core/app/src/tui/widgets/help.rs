//! The `?` overlay: the keys of the screen and the global ones, in sections.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::chrome::centred;
use crate::tui::theme;

/// A section of the overlay: a heading and its `(keys, action)` rows.
pub(crate) struct HelpSection<'a> {
    pub(crate) title: &'a str,
    pub(crate) keys: &'a [(&'a str, &'a str)],
}

/// Draws the overlay over `area`, centred. Rows that do not fit are cut.
pub(crate) fn render_help(frame: &mut Frame<'_>, area: Rect, sections: &[HelpSection<'_>]) {
    let mut lines = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::styled(section.title.to_string(), theme::accent()));
        for (keys, action) in section.keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{keys:>18}  "), theme::warn()),
                Span::raw((*action).to_string()),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("any key closes this", theme::muted()));
    let popup = centred(area, 70, 90, (44, 8));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(theme::panel(" Keys ").style(Style::default())),
        popup,
    );
}
