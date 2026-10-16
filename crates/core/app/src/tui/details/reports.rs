//! The Reports tab: the summary of the last run of the build. Its outcome and
//! times, the operations and steps that took longest as bars, the images it
//! published, its errors, and where its report files are.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{heading, key_value, note, render_page};
use crate::commands::live_status::OpState;
use crate::tui::run_view::{RunPhase, RunView};
use crate::tui::theme;
use crate::tui::widgets::badge::phase_badge;
use crate::tui::widgets::bars::{bar, share};
use crate::tui::widgets::format;
use gaia_spec::ResolvedBuildSpec;

/// Operations listed as the slowest.
const SLOWEST: usize = 8;
/// Image operations whose steps are drawn.
const STEP_OPS: usize = 2;
/// Errors listed.
const ERRORS: usize = 8;

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    view: Option<&RunView>,
    spec: Option<&ResolvedBuildSpec>,
    scroll: &mut u16,
) {
    let Some(spec) = spec else {
        render_page(
            frame,
            area,
            "Reports",
            vec![note("loading the build...")],
            scroll,
        );
        return;
    };
    let report_dir = format!("{}/.gaia/reports", spec.workspace.out_dir);
    let Some(view) = view else {
        let mut lines = vec![
            Line::styled("no run of this build has finished yet", theme::muted()),
            Line::from(""),
            heading("expected report files"),
        ];
        for name in expected_files(&spec.identity.build_name) {
            lines.push(Line::from(format!("  {name}")));
        }
        lines.push(Line::from(""));
        lines.push(key_value("reports dir", report_dir));
        render_page(frame, area, "Reports", lines, scroll);
        return;
    };

    let bar_width = (area.width as usize).saturating_sub(52).clamp(8, 40);
    let mut lines = vec![summary_line(view)];
    lines.push(key_value("started", format::clock_utc(view.started_ms)));
    lines.push(key_value("elapsed", format::duration(view.elapsed_ms())));
    if let Some(variant) = &view.variant {
        lines.push(key_value(
            "variant",
            format!("{} ({} of {})", variant.id, variant.index, variant.count),
        ));
    }
    let counts = &view.counts;
    lines.push(key_value(
        "operations",
        format!(
            "{} built  {} reused  {} failed  {} cancelled  {} skipped",
            counts.done, counts.reused, counts.failed, counts.cancelled, counts.skipped
        ),
    ));
    if let Some(total) = view.expected_total_ms() {
        lines.push(key_value(
            "recorded work",
            format!(
                "~{} summed over operations (parallel work overlaps)",
                format::duration(total)
            ),
        ));
    }

    lines.push(Line::from(""));
    lines.push(heading("Slowest operations"));
    let slowest = view.slowest(SLOWEST);
    let max = slowest.first().map_or(0, |(_, ms)| *ms);
    for (op, ms) in slowest {
        lines.push(Line::from(vec![
            Span::raw(format!("  {:<28} ", format::truncate(&op.id, 28))),
            Span::styled(bar(bar_width, share(ms, max)), theme::running()),
            Span::raw(format!(" {:>7}", format::duration(ms))),
        ]));
    }
    if view.slowest(1).is_empty() {
        lines.push(note("  no operation has finished"));
    }

    let stepped = view
        .ops
        .iter()
        .filter(|op| !op.steps.is_empty())
        .take(STEP_OPS)
        .collect::<Vec<_>>();
    for op in stepped {
        lines.push(Line::from(""));
        lines.push(heading(format!("Steps of {}", op.id)));
        let max = op.steps.iter().map(|step| step.ms).max().unwrap_or(0);
        for step in op.steps.iter().take(12) {
            lines.push(Line::from(vec![
                Span::raw(format!("  {:<26} ", format::truncate(&step.step, 26))),
                Span::styled(bar(bar_width, share(step.ms, max)), theme::running()),
                Span::raw(format!(" {:>7}", format::duration(step.ms))),
            ]));
        }
    }

    if !view.outcome.sizes.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("Images"));
        let max = view
            .outcome
            .sizes
            .iter()
            .map(|size| size.bytes)
            .max()
            .unwrap_or(0);
        for size in &view.outcome.sizes {
            let shown = if size.change == gaia_report::SizeChange::Removed {
                "-".to_string()
            } else {
                format::bytes(size.bytes)
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {:<28} ", format::truncate(&size.name, 28))),
                Span::styled(bar(bar_width, share(size.bytes, max)), theme::ok()),
                Span::raw(format!(
                    " {shown} {}",
                    crate::output_images::size_change_text(size)
                )),
            ]));
        }
    }

    if !view.errors.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("Errors ({})", view.errors.len()),
            theme::failed(),
        ));
        for error in view.errors.iter().take(ERRORS) {
            let message = error.message.lines().next().unwrap_or("");
            lines.push(Line::styled(
                format!("  {}  {}  {message}", error.operation_id, error.code),
                theme::failed(),
            ));
        }
        if view.errors.len() > ERRORS {
            lines.push(note(format!(
                "  ... and {} more; see the Events tab",
                view.errors.len() - ERRORS
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(heading("Output"));
    if let Some(out_dir) = &view.outcome.out_dir {
        lines.push(key_value("output", out_dir.clone()));
    }
    lines.push(key_value(
        "reports",
        format!("{} file(s) in {report_dir}", view.outcome.report_files),
    ));
    render_page(frame, area, "Reports", lines, scroll);
}

/// The headline: the outcome's badge, then the words for it.
fn summary_line(view: &RunView) -> Line<'static> {
    let elapsed = format::duration(view.elapsed_ms());
    let words = match view.phase {
        RunPhase::Completed => format!("Build completed in {elapsed}"),
        RunPhase::Failed => format!("Build failed after {elapsed}"),
        RunPhase::Cancelled => format!("Build cancelled after {elapsed}"),
        RunPhase::Paused => format!("Build paused at {elapsed}"),
        RunPhase::Starting => "Build starting".to_string(),
        RunPhase::Cancelling => format!("Build cancelling at {elapsed}"),
        RunPhase::Running => format!("Build running for {elapsed}"),
        RunPhase::Ended => format!("Run ended after {elapsed}"),
    };
    let failed = view
        .ops
        .iter()
        .filter(|op| op.state == OpState::Failed)
        .count();
    let mut spans = vec![phase_badge(view.phase), Span::raw("  "), Span::raw(words)];
    if failed > 0 {
        spans.push(Span::styled(format!("  {failed} failed"), theme::failed()));
    }
    Line::from(spans)
}

/// The report files a build writes, by name.
fn expected_files(build_name: &str) -> Vec<String> {
    [
        "summary.json",
        "selection.json",
        "provenance.json",
        "manifest.json",
        "rebuild-reasons.json",
    ]
    .iter()
    .map(|kind| format!("{build_name}.{kind}"))
    .collect()
}
