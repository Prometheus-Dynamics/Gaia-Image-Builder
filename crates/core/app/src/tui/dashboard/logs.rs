//! The output pane: the recent output of the run, newest last. It follows the
//! newest line unless scrolled, can show one operation, and highlights a
//! search in place.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::DashboardState;
use crate::commands::live_status::LogLine;
use crate::tui::run_view::RunView;
use crate::tui::theme;
use crate::tui::widgets::format;
use crate::tui::widgets::search::{contains_ignore_case, highlight};

/// Width of the operation column when every line shows its operation.
const OP_WIDTH: usize = 18;

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, view: &RunView, ui: &mut DashboardState) {
    let only = if ui.log_only_selected {
        ui.selected.clone()
    } else {
        None
    };
    let lines = view
        .logs
        .iter()
        .filter(|log| only.as_ref().is_none_or(|id| &log.op == id))
        .collect::<Vec<_>>();

    let follow = if ui.log_follow { "follow" } else { "held" };
    let scope = only
        .as_deref()
        .map(|id| format!("op {id}"))
        .unwrap_or_else(|| "all operations".to_string());
    let search = match (&ui.search, ui.search_typing) {
        (Some(text), true) => format!("  search: {text}_"),
        (Some(text), false) if !text.is_empty() => {
            let matches = lines
                .iter()
                .filter(|log| contains_ignore_case(&log.line, text))
                .count();
            format!("  search \"{text}\": {matches} line(s)")
        }
        _ => String::new(),
    };
    let block = theme::panel(format!(
        " Output  {} lines  {scope}  {follow}{search} ",
        lines.len()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    ui.log_scroll = ui.log_scroll.min(max_scroll);
    let start = if ui.log_follow {
        max_scroll
    } else {
        max_scroll.saturating_sub(ui.log_scroll)
    };
    let show_op = only.is_none();
    let rendered = lines
        .iter()
        .skip(start)
        .take(height)
        .map(|log| line_for(log, show_op, ui.search.as_deref()))
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        let text = if view.logs.is_empty() {
            "no output yet"
        } else {
            "no output for this operation"
        };
        frame.render_widget(Paragraph::new(Line::styled(text, theme::muted())), inner);
        return;
    }
    frame.render_widget(Paragraph::new(rendered), inner);
}

fn line_for(log: &LogLine, show_op: bool, search: Option<&str>) -> Line<'static> {
    let mut spans = Vec::new();
    if show_op {
        spans.push(Span::styled(
            format!("{:<OP_WIDTH$} ", format::truncate(&log.op, OP_WIDTH)),
            theme::muted(),
        ));
    }
    match search.filter(|text| !text.is_empty()) {
        Some(pattern) => spans.extend(highlight(&log.line, pattern)),
        None => spans.push(Span::raw(log.line.clone())),
    }
    Line::from(spans)
}
