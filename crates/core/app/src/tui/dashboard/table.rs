//! The operations table: every planned operation with its state, how long it
//! ran (live while it runs), what it took last time, and how that compares.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table};

use super::DashboardState;
use crate::commands::live_status::LiveOperation;
use crate::tui::run_view::RunView;
use crate::tui::theme;
use crate::tui::widgets::badge::op_badge;
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;

/// From this width of the table the table shows why each operation runs.
const WHY_WIDTH: u16 = 64;

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, view: &RunView, ui: &mut DashboardState) {
    if view.ops.is_empty() {
        chrome::empty_state(
            frame,
            area,
            "Operations",
            &[
                "no operations planned yet",
                "the plan appears once the run starts",
            ],
        );
        return;
    }
    let order = view.display_order(ui.filter, ui.sort);
    let show_why = area.width >= WHY_WIDTH;
    let header = Row::new(
        ["State", "Operation", "Time", "Expected", "Δ", "Why"]
            .into_iter()
            .take(if show_why { 6 } else { 5 })
            .map(|title| Cell::from(Span::styled(title, theme::muted()))),
    );
    let rows = order
        .iter()
        .map(|&index| row(view, &view.ops[index], show_why));
    let mut widths = vec![
        Constraint::Length(8),
        Constraint::Min(16),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(7),
    ];
    if show_why {
        widths.push(Constraint::Min(8));
    }
    let title = format!(
        " Operations {}/{}  filter {}  sort {} ",
        order.len(),
        view.ops.len(),
        ui.filter.label(),
        ui.sort.label()
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(theme::panel(title))
        .row_highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(table, area, &mut ui.table);
}

fn row(view: &RunView, op: &LiveOperation, show_why: bool) -> Row<'static> {
    let (icon, label, style) = op_badge(op);
    let state = Line::from(vec![Span::styled(format!("{icon} {label}"), style)]);
    let time = view
        .op_duration_ms(op)
        .map(format::duration)
        .unwrap_or_else(|| "-".into());
    let expected = op
        .expected_ms
        .map(format::duration)
        .unwrap_or_else(|| "-".into());
    let delta = match view.delta_ms(op) {
        Some(delta) if delta > 0 => Span::styled(format::delta(delta), theme::warn()),
        Some(delta) => Span::styled(format::delta(delta), theme::ok()),
        None => Span::raw(""),
    };
    let mut cells = vec![
        Cell::from(state),
        Cell::from(op.id.clone()),
        Cell::from(Line::from(time).right_aligned()),
        Cell::from(Line::from(expected).right_aligned()),
        Cell::from(Line::from(delta).right_aligned()),
    ];
    if show_why {
        let why = match (&op.reason_code, op.planned_reuse) {
            (Some(code), _) => code.clone(),
            (None, true) => "reused".to_string(),
            (None, false) => String::new(),
        };
        cells.push(Cell::from(Span::styled(why, theme::muted())));
    }
    Row::new(cells)
}
