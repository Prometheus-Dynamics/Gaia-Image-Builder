//! The chart area: a tab bar and one tab at a time. Progress plots this run
//! against the last one; resources sparkline the machine; steps and packages
//! show where an image operation spends its time.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, LegendPosition, Paragraph, Sparkline};

use super::{ChartTab, DashboardState, summary};
use crate::commands::live_status::LiveOperation;
use crate::tui::metrics::Metrics;
use crate::tui::run_view::RunView;
use crate::tui::theme;
use crate::tui::widgets::bars::{bar, share};
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;

/// Rows a resource takes: its label and its sparkline.
const RESOURCE_ROWS: u16 = 2;
/// Steps and packages listed at most.
const LIST_LIMIT: usize = 12;

pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &RunView,
    ui: &mut DashboardState,
    metrics: &Metrics,
) {
    let tabs = ui.tabs();
    let mut spans = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        let label = format!(" {} {} ", index + 1, tab.title());
        let style = if *tab == ui.tab {
            theme::selected()
        } else {
            theme::muted()
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let block = theme::panel(Line::from(spans));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match ui.tab {
        ChartTab::Summary if view.is_over() => summary::render(frame, inner, view),
        ChartTab::Summary => chrome::empty_state(
            frame,
            inner,
            "Summary",
            &[
                "the summary appears when the run ends",
                "it lists the slowest operations, images and errors",
            ],
        ),
        ChartTab::Progress => progress_chart(frame, inner, view),
        ChartTab::Resources => resources(frame, inner, metrics),
        ChartTab::Steps => steps(frame, inner, view, ui.selected_op(view)),
        ChartTab::Packages => packages(frame, inner, view),
    }
}

/// Cumulative progress over elapsed time: this run, and the last run replayed
/// from its recorded durations.
fn progress_chart(frame: &mut Frame<'_>, area: Rect, view: &RunView) {
    let this = view.curve_this_run();
    let last = view.curve_last_run();
    if this.len() < 2 && last.is_empty() {
        chrome::empty_state(
            frame,
            area,
            "Progress",
            &[
                "nothing to chart yet",
                "the curve appears as operations finish",
            ],
        );
        return;
    }
    let last_total = last.last().map_or(0.0, |point| point.0);
    let x_max = (view.elapsed_ms() as f64 / 1_000.0)
        .max(last_total)
        .max(1.0);
    // The two curves differ in marker and weight, so they tell apart without
    // colour: this run is a solid braille line, the last run a dotted one.
    let mut datasets = vec![
        Dataset::default()
            .name(" this run ")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(theme::running())
            .data(&this),
    ];
    if !last.is_empty() {
        datasets.push(
            Dataset::default()
                .name(" last run ")
                .marker(symbols::Marker::Dot)
                .graph_type(GraphType::Line)
                .style(theme::muted())
                .data(&last),
        );
    }
    // The present: where this run stands now, on its own curve.
    let now_point = this.last().copied().into_iter().collect::<Vec<_>>();
    if !now_point.is_empty() {
        datasets.push(
            Dataset::default()
                .name(" now ")
                .marker(symbols::Marker::Block)
                .graph_type(GraphType::Scatter)
                .style(theme::warn())
                .data(&now_point),
        );
    }
    let chart = Chart::new(datasets)
        .legend_position(Some(LegendPosition::TopLeft))
        .x_axis(
            Axis::default()
                .title(Span::styled("elapsed", theme::muted()))
                .style(theme::muted())
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format::duration((x_max * 500.0) as u64)),
                    Span::raw(format::duration((x_max * 1_000.0) as u64)),
                ]),
        )
        .y_axis(
            Axis::default()
                .title(Span::styled("done", theme::muted()))
                .style(theme::muted())
                .bounds([0.0, 100.0])
                .labels(vec![Span::raw("0%"), Span::raw("50%"), Span::raw("100%")]),
        );
    frame.render_widget(chart, area);
}

/// CPU, memory, tmpfs, load and disk, each with its current value and a
/// sparkline of its recent history.
fn resources(frame: &mut Frame<'_>, area: Rect, metrics: &Metrics) {
    let cpu = metrics.cpu.last().unwrap_or(0.0);
    let memory_pct = metrics.memory.last().unwrap_or(0.0);
    let memory_used = format::bytes(metrics.memory_used_kib * 1_024);
    let memory_total = format::bytes(metrics.memory_total_kib * 1_024);
    let shm = metrics.shm.last().unwrap_or(0.0);
    let load = metrics.load.last().unwrap_or(0.0);
    let read = metrics.disk_read.last().unwrap_or(0.0);
    let write = metrics.disk_write.last().unwrap_or(0.0);
    let items = [
        (
            format!("CPU  {}", format::percent(cpu)),
            metrics.cpu.sparkline(area.width as usize, 100.0, 100.0),
            theme::running(),
        ),
        (
            format!(
                "Memory  {} of {} ({})",
                memory_used,
                memory_total,
                format::percent(memory_pct)
            ),
            metrics.memory.sparkline(area.width as usize, 100.0, 100.0),
            theme::ok(),
        ),
        (
            format!(
                "tmpfs (/dev/shm)  {}",
                format::bytes((shm * 1_024.0 * 1_024.0) as u64)
            ),
            metrics.shm.sparkline(area.width as usize, 100.0, 0.0),
            theme::reused(),
        ),
        (
            format!("Load (1 min)  {load:.2}"),
            metrics.load.sparkline(area.width as usize, 100.0, 0.0),
            theme::warn(),
        ),
        (
            format!(
                "Disk  read {}  write {}",
                format::rate(read),
                format::rate(write)
            ),
            metrics
                .disk_write
                .sparkline(area.width as usize, 100.0, 0.0),
            theme::paused(),
        ),
    ];
    let visible = ((area.height / RESOURCE_ROWS) as usize).min(items.len());
    if visible == 0 {
        return;
    }
    let rows = Layout::vertical(vec![Constraint::Length(RESOURCE_ROWS); visible]).split(area);
    for ((label, data, style), row) in items.into_iter().zip(rows.iter()) {
        let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(*row);
        frame.render_widget(Paragraph::new(Line::styled(label, style)), parts[0]);
        frame.render_widget(
            Sparkline::default().data(data).style(style).max(100),
            parts[1],
        );
    }
}

/// The step times of the selected image operation, or of the one that has
/// reported steps most recently.
fn steps(frame: &mut Frame<'_>, area: Rect, view: &RunView, selected: Option<&LiveOperation>) {
    let op = selected
        .filter(|op| !op.steps.is_empty())
        .or_else(|| view.current_op().filter(|op| !op.steps.is_empty()))
        .or_else(|| view.ops.iter().rev().find(|op| !op.steps.is_empty()));
    let Some(op) = op else {
        chrome::empty_state(
            frame,
            area,
            "Steps",
            &[
                "no step times yet",
                "an image operation reports its steps as it goes",
            ],
        );
        return;
    };
    let name_width = (area.width as usize / 3).clamp(8, 28);
    let bar_width = (area.width as usize).saturating_sub(name_width + 12).max(4);
    let max = op.steps.iter().map(|step| step.ms).max().unwrap_or(0);
    let mut lines = vec![Line::styled(format!("steps of {}", op.id), theme::accent())];
    for step in op.steps.iter().rev().take(LIST_LIMIT).rev() {
        lines.push(Line::from(vec![
            Span::raw(format!(
                "{:<name_width$} ",
                format::truncate(&step.step, name_width)
            )),
            Span::styled(bar(bar_width, share(step.ms, max)), theme::running()),
            Span::raw(format!(" {:>7}", format::duration(step.ms))),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The Buildroot packages of the make: what is building, and the slowest that
/// finished.
fn packages(frame: &mut Frame<'_>, area: Rect, view: &RunView) {
    let Some(op) = view.package_op() else {
        chrome::empty_state(
            frame,
            area,
            "Packages",
            &[
                "no Buildroot packages reported",
                "a make reports its packages as it builds them",
            ],
        );
        return;
    };
    let Some(build) = op.build.as_ref() else {
        return;
    };
    let percent = share(build.done as u64, build.total as u64);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{}  ", op.id), theme::accent()),
            Span::raw(format!("packages {}/{}  ", build.done, build.total)),
            Span::raw(bar(16, percent)),
            Span::raw(format!(" {}", format::percent(percent * 100.0))),
            Span::styled(
                build
                    .eta_secs
                    .map(|eta| format!("  ETA {}", format::eta(eta * 1_000)))
                    .unwrap_or_default(),
                theme::muted(),
            ),
        ]),
        Line::from(""),
    ];
    let now = view.now_ms;
    lines.push(Line::styled(
        format!(
            "active now ({})",
            build.active_since.len().max(build.active.len())
        ),
        theme::running(),
    ));
    let mut active = build.active_since.clone();
    active.sort_by_key(|package| package.started_ms);
    if active.is_empty() {
        lines.extend(
            build
                .active
                .iter()
                .take(LIST_LIMIT)
                .map(|name| Line::from(format!("  {name}"))),
        );
    }
    for package in active.iter().take(LIST_LIMIT) {
        let elapsed = now.saturating_sub(package.started_ms);
        lines.push(Line::from(format!(
            "  {:<24} {}",
            format::truncate(&package.name, 24),
            format::duration(elapsed)
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        format!("slowest finished ({} kept)", build.finished.len()),
        theme::accent(),
    ));
    let mut finished = build.finished.clone();
    finished.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
    let max = finished.first().map_or(0, |package| package.ms);
    let bar_width = (area.width as usize).saturating_sub(42).max(4);
    for package in finished.iter().take(LIST_LIMIT) {
        lines.push(Line::from(vec![
            Span::raw(format!("  {:<24} ", format::truncate(&package.name, 24))),
            Span::styled(bar(bar_width, share(package.ms, max)), theme::ok()),
            Span::raw(format!(" {:>7}", format::duration(package.ms))),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}
