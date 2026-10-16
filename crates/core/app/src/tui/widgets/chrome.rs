//! The frame around the screens: the key-hint footer, empty states, the
//! too-small notice and popup frames.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::theme;

/// The smallest area the dashboard lays out; below it a notice is shown.
pub(crate) const MIN_WIDTH: u16 = 40;
pub(crate) const MIN_HEIGHT: u16 = 8;

/// A key hint such as `[f] filter  [s] sort`, with each bracketed key in the
/// accent style, followed by the notice if there is one.
pub(crate) fn hint_line(hint: &str, notice: Option<&str>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut rest = hint;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|offset| open + offset) else {
            break;
        };
        spans.push(Span::raw(rest[..open].to_string()));
        spans.push(Span::styled(
            rest[open..=close].to_string(),
            theme::accent(),
        ));
        rest = &rest[close + 1..];
    }
    spans.push(Span::raw(rest.to_string()));
    if let Some(notice) = notice {
        spans.push(Span::styled("  |  ", theme::muted()));
        spans.push(Span::styled(notice.to_string(), theme::warn()));
    }
    Line::from(spans)
}

/// The footer: the key hints for the screen, wrapped if they do not fit.
pub(crate) fn footer(frame: &mut Frame<'_>, area: Rect, hint: &str, notice: Option<&str>) {
    let paragraph = Paragraph::new(Text::from(hint_line(hint, notice)))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(theme::border()),
        );
    frame.render_widget(paragraph, area);
}

/// A panel with a centred muted message, for a view with nothing to show.
pub(crate) fn empty_state(frame: &mut Frame<'_>, area: Rect, title: &str, lines: &[&str]) {
    let mut text = vec![Line::from("")];
    text.extend(
        lines
            .iter()
            .map(|line| Line::styled(line.to_string(), theme::muted())),
    );
    frame.render_widget(
        Paragraph::new(Text::from(text))
            .alignment(Alignment::Center)
            .block(theme::panel(format!(" {title} "))),
        area,
    );
}

/// The one-line header of every screen: the gaia mark, the screen's name and
/// what the screen is about (a build, a run), with `subject` after it.
pub(crate) fn screen_header(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    subject: Vec<Span<'static>>,
) {
    let mut spans = vec![
        Span::styled(" Gaia ", theme::selected()),
        Span::styled(format!(" {title}  "), theme::accent()),
    ];
    spans.extend(subject);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The smallest terminal the whole TUI lays out in; below it a notice is shown.
pub(crate) fn fits_screen(area: Rect) -> bool {
    area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT
}

/// The notice for an area too small to lay anything out.
pub(crate) fn too_small(frame: &mut Frame<'_>, area: Rect) {
    let text = format!("terminal too small: need {MIN_WIDTH}x{MIN_HEIGHT}");
    frame.render_widget(
        Paragraph::new(Line::styled(text, theme::warn())).wrap(Wrap { trim: true }),
        area,
    );
}

/// Whether `area` is big enough for the dashboard.
pub(crate) fn fits(area: Rect) -> bool {
    area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT
}

/// A rectangle centred in `area`, `width_pct` by `height_pct` of it (at least
/// the given minimum, and never more than `area`).
pub(crate) fn centred(area: Rect, width_pct: u16, height_pct: u16, min: (u16, u16)) -> Rect {
    let width = (area.width * width_pct / 100).max(min.0).min(area.width);
    let height = (area.height * height_pct / 100).max(min.1).min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Clears `area` and draws a popup frame with `title` in it; returns the
/// inside to draw the popup's content in.
pub(crate) fn popup_frame(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, area);
    let block = theme::panel(format!(" {title} ")).style(Style::default());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// A popup's content split into the body and a one-line hint at the bottom.
pub(crate) fn popup_body(inner: Rect) -> (Rect, Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    (rows[0], rows[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_style_only_the_bracketed_keys() {
        let line = hint_line("[f] filter  [s] sort", Some("saved"));
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "[f] filter  [s] sort  |  saved");
        assert_eq!(line.spans[1].style, theme::accent());
        assert_eq!(line.spans[0].style, Style::default());
    }

    #[test]
    fn centred_fits_inside_its_area() {
        let area = Rect::new(0, 0, 80, 24);
        let popup = centred(area, 80, 80, (40, 12));
        assert!(popup.x + popup.width <= area.width && popup.y + popup.height <= area.height);
        assert_eq!(popup.width, 64);
        let tiny = centred(Rect::new(0, 0, 20, 5), 80, 80, (40, 12));
        assert_eq!((tiny.width, tiny.height), (20, 5));
    }
}
