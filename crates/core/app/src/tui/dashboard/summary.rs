//! The completion summary: how the run ended, how long it took, the slowest
//! operations, the images it published and its errors. It is the first chart
//! tab once the run is over.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::run_view::{RunPhase, RunView};
use crate::tui::theme;
use crate::tui::widgets::bars::{bar, share};
use crate::tui::widgets::format;

/// Operations listed as the slowest.
const SLOWEST: usize = 5;
/// Errors listed.
const ERRORS: usize = 5;

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, view: &RunView) {
    let elapsed = format::duration(view.elapsed_ms());
    let (headline, style) = match view.phase {
        RunPhase::Completed => (format!("Build completed in {elapsed}"), theme::ok()),
        RunPhase::Failed => (format!("Build failed after {elapsed}"), theme::failed()),
        RunPhase::Cancelled => (format!("Build cancelled after {elapsed}"), theme::warn()),
        _ => (format!("Run ended after {elapsed}"), theme::muted()),
    };
    let counts = &view.counts;
    let mut lines = vec![
        Line::styled(headline, style),
        Line::from(format!(
            "operations: {} built  {} reused  {} failed  {} cancelled  {} skipped",
            counts.done, counts.reused, counts.failed, counts.cancelled, counts.skipped
        )),
    ];
    if let Some(total) = view.expected_total_ms() {
        lines.push(Line::styled(
            format!(
                "last run of these operations: {} in total (summed over operations; parallel work overlaps)",
                format::duration(total)
            ),
            theme::muted(),
        ));
    }
    if let Some(variant) = &view.variant {
        lines.push(Line::from(format!(
            "variant {} ({} of {})",
            variant.id, variant.index, variant.count
        )));
    }

    let slowest = view.slowest(SLOWEST);
    if !slowest.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled("slowest operations", theme::accent()));
        let max = slowest.first().map_or(0, |(_, ms)| *ms);
        for (op, ms) in slowest {
            lines.push(Line::from(vec![
                Span::raw(format!(
                    "  {:<28} ",
                    crate::tui::widgets::format::truncate(&op.id, 28)
                )),
                Span::styled(bar(16, share(ms, max)), theme::running()),
                Span::raw(format!(" {:>7}", format::duration(ms))),
            ]));
        }
    }

    if !view.outcome.image_sizes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled("images", theme::accent()));
        for image in &view.outcome.image_sizes {
            lines.push(Line::from(format!(
                "  {}  raw {}  content {}",
                image.path,
                format::bytes(image.raw_bytes),
                format::bytes(image.content_bytes)
            )));
        }
    }
    if let Some(out_dir) = &view.outcome.out_dir {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("output: {out_dir}")));
        lines.push(Line::from(format!(
            "reports: {} file(s) in {out_dir}/.gaia/reports",
            view.outcome.report_files
        )));
    }

    if !view.errors.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled("errors", theme::failed()));
        for error in view.errors.iter().take(ERRORS) {
            let message = error.message.lines().next().unwrap_or("").to_string();
            lines.push(Line::styled(
                format!("  {}  {}", error.operation_id, message),
                theme::failed(),
            ));
        }
        if view.errors.len() > ERRORS {
            lines.push(Line::styled(
                format!(
                    "  ... and {} more; see the operation's output",
                    view.errors.len() - ERRORS
                ),
                theme::muted(),
            ));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
