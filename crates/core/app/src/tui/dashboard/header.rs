//! The header bar (state, name, variant, times) and the progress block
//! (the overall gauge and the packages of a Buildroot make).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::run_view::{RunPhase, RunSource, RunView};
use crate::tui::theme;
use crate::tui::widgets::badge::phase_badge;
use crate::tui::widgets::bars::bar;
use crate::tui::widgets::format;

/// The header bar: one line inside a rounded frame.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, view: &RunView) {
    let mut spans = vec![phase_badge(view.phase), Span::raw("  ")];
    if view.source == RunSource::Attached {
        spans.push(Span::styled("attached ", theme::reused()));
    }
    spans.push(Span::styled(view.display_name.clone(), theme::accent()));
    if let Some(selection) = &view.selection {
        spans.push(Span::styled(format!("  {selection}"), theme::muted()));
    }
    if let Some(variant) = &view.variant {
        spans.push(Span::styled(
            format!(
                "  variant {} {}/{}",
                variant.id, variant.index, variant.count
            ),
            theme::reused(),
        ));
    }
    spans.push(Span::raw("  elapsed "));
    spans.push(Span::raw(format::duration(view.elapsed_ms())).bold());
    if view.is_over() {
        spans.push(Span::styled(
            format!("  {}", outcome_word(view.phase)),
            theme::muted(),
        ));
    } else if let Some(eta) = view.eta_ms() {
        spans.push(Span::raw("  ETA "));
        spans.push(Span::raw(format::eta(eta)).bold());
    }
    if let Some(pid) = view.pid {
        spans.push(Span::styled(format!("  pid {pid}"), theme::muted()));
    }
    if area.width >= 100 {
        spans.push(Span::styled(
            format!("  started {}", format::clock_utc(view.started_ms)),
            theme::muted(),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(theme::panel(" Gaia ")),
        area,
    );
}

fn outcome_word(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Completed => "completed",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
        _ => "ended",
    }
}

/// The progress block: the overall gauge and a second line that is the
/// packages of the running make, the current operation, or the outcome. With
/// `framed`, it has a frame; without, it is two plain lines.
pub(super) fn render_progress(frame: &mut Frame<'_>, area: Rect, view: &RunView, framed: bool) {
    let inner = if framed {
        let block = theme::panel(" Progress ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    } else {
        area
    };
    let width = (inner.width as usize).saturating_sub(14).clamp(4, 60);
    let percent = view.progress_percent();
    let counts = counts_line(view);
    let overall = Line::from(vec![
        Span::styled("Overall  ", theme::accent()),
        Span::raw(bar(width, percent as f64 / 100.0)),
        Span::raw(format!(" {:>3}%  ", percent)),
        Span::styled(counts, theme::muted()),
    ]);
    let detail = detail_line(view, width);
    frame.render_widget(Paragraph::new(vec![overall, detail]), inner);
}

fn counts_line(view: &RunView) -> String {
    let running = view
        .ops
        .iter()
        .filter(|op| op.state == crate::commands::live_status::OpState::Running)
        .count();
    let eta = if view.is_over() {
        String::new()
    } else {
        match view.eta_ms() {
            Some(eta) => format!("  ETA {}", format::eta(eta)),
            None => "  ETA -".to_string(),
        }
    };
    format!(
        "ops {}/{}  running {}  failed {}  reused {}{}",
        view.counts.total().min(view.ops_total),
        view.ops_total,
        running,
        view.counts.failed,
        view.counts.reused,
        eta
    )
}

fn detail_line(view: &RunView, width: usize) -> Line<'static> {
    if let Some(op) = view.package_op()
        && let Some(build) = &op.build
        && view.phase != RunPhase::Completed
        && build.total > 0
        && !view.is_over()
    {
        let percent = build.done as f64 / build.total as f64;
        let mut spans = vec![
            Span::styled("Packages ", theme::accent()),
            Span::raw(bar(width, percent)),
            Span::raw(format!(" {}/{}", build.done, build.total)),
        ];
        if let Some(eta) = build.eta_secs {
            spans.push(Span::raw(format!(
                "  make ETA {}",
                format::eta(eta * 1_000)
            )));
        }
        if !build.active.is_empty() {
            spans.push(Span::styled(
                format!("  active: {}", build.active.join(", ")),
                theme::muted(),
            ));
        }
        return Line::from(spans);
    }
    if view.is_over() {
        return Line::from(vec![
            Span::styled("Finished  ", theme::accent()),
            Span::raw(format!(
                "{} in {}",
                outcome_word(view.phase),
                format::duration(view.elapsed_ms())
            )),
        ]);
    }
    match view.current_op() {
        Some(op) => Line::from(vec![
            Span::styled("Now  ", theme::accent()),
            Span::styled(op.id.clone(), theme::running()),
            Span::styled(
                format!(
                    "  for {}",
                    format::duration(view.op_duration_ms(op).unwrap_or(0))
                ),
                theme::muted(),
            ),
            Span::styled(
                op.last_log
                    .as_ref()
                    .map(|line| format!("  last: {line}"))
                    .unwrap_or_default(),
                theme::muted(),
            ),
        ]),
        None => Line::from(Span::styled(
            if view.phase == RunPhase::Starting {
                "Starting the plan"
            } else {
                "Waiting for the next operation"
            },
            theme::muted(),
        )),
    }
}
