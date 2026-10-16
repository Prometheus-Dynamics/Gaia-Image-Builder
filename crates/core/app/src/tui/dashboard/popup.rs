//! The operation popup: why one operation runs, how it went against its
//! expectation, its packages and steps, its error, and the tail of its output.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::commands::live_status::LiveOperation;
use crate::tui::run_view::RunView;
use crate::tui::theme;
use crate::tui::widgets::badge::op_badge;
use crate::tui::widgets::bars::{bar, share};
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;

/// Output lines shown at the end of the popup.
const TAIL_LINES: usize = 12;

pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &RunView,
    op: &LiveOperation,
    tail: &dyn Fn(&str) -> Vec<String>,
) {
    let popup = chrome::centred(area, 84, 88, (50, 14));
    let inner = chrome::popup_frame(frame, popup, &op.id);
    let (body, hint) = chrome::popup_body(inner);
    let lines = content(view, op, tail);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "[Esc] close  [↑↓] other operation",
            theme::muted(),
        )),
        hint,
    );
}

fn content(
    view: &RunView,
    op: &LiveOperation,
    tail: &dyn Fn(&str) -> Vec<String>,
) -> Vec<Line<'static>> {
    let (icon, label, style) = op_badge(op);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{icon} {label}"), style),
        Span::raw(format!("  domain {}", op.domain)),
    ])];

    let why = match (&op.reason_code, op.planned_reuse) {
        (_, true) => "the plan reuses it: its output is already current".to_string(),
        (Some(code), _) => format!("runs because {code}"),
        (None, _) => "runs".to_string(),
    };
    lines.push(Line::from(format!("why: {why}")));
    if let Some(reason) = &op.reason {
        lines.push(Line::styled(format!("  {reason}"), theme::muted()));
    }

    let took = view
        .op_duration_ms(op)
        .map(|ms| match view.delta_ms(op) {
            Some(delta) => format!("{} ({})", format::duration(ms), format::delta(delta)),
            None => format::duration(ms),
        })
        .unwrap_or_else(|| "not started".to_string());
    let expected = op
        .expected_ms
        .map(format::duration)
        .unwrap_or_else(|| "unknown".to_string());
    lines.push(Line::from(format!("took: {took}   expected: {expected}")));
    if let Some(started) = op.started_ms {
        let offset = started.saturating_sub(view.started_ms);
        lines.push(Line::from(format!(
            "started at +{}",
            format::duration(offset)
        )));
    }

    if let Some(build) = &op.build {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("packages {}/{}", build.done, build.total),
            theme::accent(),
        ));
        if !build.active.is_empty() {
            lines.push(Line::from(format!("  active: {}", build.active.join(", "))));
        }
    }

    if !op.steps.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled("steps", theme::accent()));
        let max = op.steps.iter().map(|step| step.ms).max().unwrap_or(0);
        for step in &op.steps {
            lines.push(Line::from(vec![
                Span::raw(format!("  {:<26} ", format::truncate(&step.step, 26))),
                Span::styled(bar(18, share(step.ms, max)), theme::running()),
                Span::raw(format!(" {:>7}", format::duration(step.ms))),
            ]));
        }
    }

    let errors = view
        .errors
        .iter()
        .filter(|error| error.operation_id == op.id)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled("error", theme::failed()));
        for error in errors {
            if !error.code.is_empty() {
                lines.push(Line::styled(format!("  {}", error.code), theme::failed()));
            }
            for message in error.message.lines().take(8) {
                lines.push(Line::styled(format!("  {message}"), theme::failed()));
            }
        }
    }

    lines.push(Line::from(""));
    let mut output = tail(&op.id);
    if output.is_empty() {
        output = view.op_log_lines(&op.id);
    }
    if output.is_empty() {
        lines.push(Line::styled(
            "no output kept for this operation",
            theme::muted(),
        ));
    } else {
        lines.push(Line::styled(
            format!("output (last {}):", TAIL_LINES.min(output.len())),
            theme::accent(),
        ));
        let skip = output.len().saturating_sub(TAIL_LINES);
        lines.extend(
            output
                .into_iter()
                .skip(skip)
                .map(|line| Line::from(format!("  {line}"))),
        );
    }
    lines
}
