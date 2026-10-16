//! The Plan tab: every operation of the next run as a table, with whether it
//! runs or is reused, why, and what it took last time.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table, TableState};

use crate::tui::theme;
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;
use gaia_plan::{ExecutionPlan, OperationReuse, PlannedOperation};

/// Which operations the table lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlanFilter {
    #[default]
    All,
    Run,
    Reuse,
}

impl PlanFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Run,
            Self::Run => Self::Reuse,
            Self::Reuse => Self::All,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Run => "run",
            Self::Reuse => "reuse",
        }
    }

    fn passes(self, operation: &PlannedOperation) -> bool {
        match self {
            Self::All => true,
            Self::Run => operation.reuse.should_execute(),
            Self::Reuse => !operation.reuse.should_execute(),
        }
    }
}

/// Moves the table's selection by `step` rows, within `total`.
pub(crate) fn move_selection(table: &mut TableState, total: usize, step: isize) {
    if total == 0 {
        table.select(None);
        return;
    }
    let current = table.selected().unwrap_or(0) as isize;
    let next = (current + step).clamp(0, total as isize - 1) as usize;
    table.select(Some(next));
}

/// Why an operation runs or is reused, as `(code, text)`.
pub(crate) fn reason_of(operation: &PlannedOperation) -> (String, String) {
    match &operation.reuse {
        OperationReuse::Execute(reason) => (reason.code.to_string(), reason.message.clone()),
        OperationReuse::Reuse { source } => ("reused".to_string(), format!("reused from {source}")),
    }
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    plan: Option<&ExecutionPlan>,
    durations: &BTreeMap<String, u64>,
    filter: PlanFilter,
    table: &mut TableState,
) {
    let Some(plan) = plan else {
        chrome::empty_state(
            frame,
            area,
            "Plan",
            &["no plan yet", "the plan appears once the build state loads"],
        );
        return;
    };
    let running = plan
        .operations
        .iter()
        .filter(|operation| operation.reuse.should_execute())
        .count();
    let shown = plan
        .operations
        .iter()
        .filter(|operation| filter.passes(operation))
        .count();
    let title = format!(
        " Plan  {} operations: {running} run, {} reused  filter {}  showing {shown} ",
        plan.operations.len(),
        plan.operations.len() - running,
        filter.label()
    );
    let wide = area.width >= 100;
    let mut header = vec!["Decision", "Operation", "Reason", "Last run"];
    let mut widths = vec![
        Constraint::Length(9),
        Constraint::Min(18),
        Constraint::Length(22),
        Constraint::Length(9),
    ];
    if wide {
        header.push("Why");
        widths.push(Constraint::Min(16));
    }
    let rows = plan
        .operations
        .iter()
        .filter(|operation| filter.passes(operation))
        .map(|operation| row(operation, durations, wide))
        .collect::<Vec<_>>();
    let table_widget = Table::new(rows, widths)
        .header(Row::new(
            header
                .into_iter()
                .map(|title| Cell::from(Span::styled(title, theme::muted()))),
        ))
        .block(theme::panel(title))
        .row_highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let total = shown;
    if table.selected().is_some_and(|index| index >= total) {
        table.select(total.checked_sub(1));
    }
    frame.render_stateful_widget(table_widget, area, table);
}

fn row(
    operation: &PlannedOperation,
    durations: &BTreeMap<String, u64>,
    wide: bool,
) -> Row<'static> {
    let (decision, style) = if operation.reuse.should_execute() {
        ("RUN", theme::running())
    } else {
        ("REUSE", theme::reused())
    };
    let (code, text) = reason_of(operation);
    let last = durations
        .get(operation.id.as_str())
        .map(|ms| format::duration(*ms))
        .unwrap_or_else(|| "-".into());
    let mut cells = vec![
        Cell::from(Span::styled(decision, style)),
        Cell::from(operation.id.as_str().to_string()),
        Cell::from(Span::styled(code, theme::muted())),
        Cell::from(Line::from(last).right_aligned()),
    ];
    if wide {
        cells.push(Cell::from(Span::styled(text, theme::muted())));
    }
    Row::new(cells)
}
