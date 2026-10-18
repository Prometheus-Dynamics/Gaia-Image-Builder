//! Render tests of the dashboard on a test backend, at the sizes the TUI
//! must work at, plus the keys that drive it.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use super::*;
use crate::commands::live_status::{LiveVariant, RunError, fixture_status};
use crate::tui::metrics::Metrics;
use crate::tui::run_view::tests::fixture_view;
use crate::tui::run_view::{RunPhase, RunSource, RunView};

/// The view's clock: a minute into the fixture run.
const NOW_MS: u64 = 1_060_000;

fn no_tail(_: &str) -> Vec<String> {
    Vec::new()
}

fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The dashboard drawn at `width` x `height`, as text.
fn draw(view: &RunView, ui: &mut DashboardState, width: u16, height: u16) -> String {
    let metrics = Metrics::new("/nonexistent");
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| render(frame, frame.area(), view, ui, &metrics, &no_tail))
        .expect("draw");
    buffer_text(terminal.backend().buffer())
}

fn live_view() -> RunView {
    let mut view = fixture_view(NOW_MS);
    view.variant = Some(LiveVariant {
        id: "rel2026".into(),
        index: 2,
        count: 4,
    });
    view
}

#[test]
fn live_run_fits_the_small_terminal() {
    let view = live_view();
    let text = draw(&view, &mut DashboardState::default(), 80, 24);
    assert!(text.contains("RUNNING"), "{text}");
    assert!(text.contains("image:buildroot"), "{text}");
    assert!(text.contains("Overall"), "{text}");
    assert!(text.contains("Output"), "{text}");
}

#[test]
fn the_footer_names_the_keys() {
    let hint = footer_hint(false);
    assert!(hint.contains("[p] pause/resume"), "{hint}");
    assert!(hint.contains("[c] cancel"), "{hint}");
    assert!(footer_hint(true).contains("[q] quit"));
}

#[test]
fn live_run_uses_the_wide_layout_with_charts_beside_the_table() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let text = draw(&view, &mut ui, 120, 32);
    assert!(text.contains("Operations 4/4"), "{text}");
    assert!(text.contains("1 Progress"), "{text}");
    assert!(text.contains("2 Resources"), "{text}");
    assert!(text.contains("30/60"), "{text}");
    assert!(text.contains("variant rel2026 2/4"), "{text}");
    assert!(text.contains("make ETA"), "{text}");
    assert!(text.contains("Why"), "{text}");
}

#[test]
fn large_terminal_shows_the_whole_picture() {
    let view = live_view();
    let mut ui = DashboardState {
        tab: ChartTab::Steps,
        selected: Some("image:buildroot".into()),
        ..DashboardState::default()
    };
    let text = draw(&view, &mut ui, 160, 45);
    assert!(text.contains("steps of image:buildroot"), "{text}");
    assert!(text.contains("buildroot make"), "{text}");
    assert!(text.contains("compiling linux"), "{text}");
    assert!(text.contains("config_changed"), "{text}");
}

#[test]
fn packages_tab_lists_active_and_slowest() {
    let mut view = live_view();
    let build = view.ops[2].build.as_mut().expect("build");
    build.active_since = vec![crate::commands::live_status::PackageStart {
        name: "linux".into(),
        started_ms: NOW_MS - 45_000,
    }];
    build.finished = vec![crate::commands::live_status::PackageTiming {
        name: "mesa3d".into(),
        ms: 90_000,
    }];
    let mut ui = DashboardState {
        tab: ChartTab::Packages,
        ..DashboardState::default()
    };
    let text = draw(&view, &mut ui, 120, 32);
    assert!(text.contains("slowest finished"), "{text}");
    assert!(text.contains("mesa3d"), "{text}");
    assert!(text.contains("45s"), "{text}");
}

#[test]
fn resources_tab_draws_the_machine_rows() {
    let view = live_view();
    let mut metrics = Metrics::new("/nonexistent");
    metrics.cpu.push(40.0);
    metrics.memory.push(50.0);
    metrics.memory_used_kib = 8 * 1024 * 1024;
    metrics.memory_total_kib = 16 * 1024 * 1024;
    metrics.load.push(1.25);
    let mut ui = DashboardState {
        tab: ChartTab::Resources,
        ..DashboardState::default()
    };
    let mut terminal = ratatui::Terminal::new(TestBackend::new(120, 32)).expect("terminal");
    terminal
        .draw(|frame| render(frame, frame.area(), &view, &mut ui, &metrics, &no_tail))
        .expect("draw");
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("CPU  40%"), "{text}");
    assert!(text.contains("Memory  8.0 GiB of 16.0 GiB (50%)"), "{text}");
    assert!(text.contains("Load (1 min)  1.25"), "{text}");
}

#[test]
fn the_progress_chart_plots_this_run_against_the_last() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let text = draw(&view, &mut ui, 160, 45);
    assert!(text.contains("this run"), "{text}");
    assert!(text.contains("last run"), "{text}");
    assert!(text.contains("now"), "{text}");
}

#[test]
fn a_completed_run_opens_on_the_summary() {
    let status = fixture_status();
    let mut view = RunView::ended(
        &status,
        "completed",
        NOW_MS,
        RunSource::Monitor,
        &[],
        NOW_MS,
    );
    view.ops = live_view().ops;
    view.outcome.out_dir = Some("/out/demo".into());
    view.outcome.report_files = 5;
    view.outcome.sizes = vec![gaia_report::OutputSizeRecord {
        kind: "disk".into(),
        name: "image.img".into(),
        path: "/out/demo/image.img".into(),
        bytes: 3 * 1024 * 1024 * 1024,
        change: gaia_report::SizeChange::New,
    }];
    let mut ui = DashboardState::default();
    let text = draw(&view, &mut ui, 160, 45);
    assert_eq!(ui.tab, ChartTab::Summary);
    assert!(
        text.contains("COMPLETED") || text.contains("DONE"),
        "{text}"
    );
    assert!(text.contains("Build completed in"), "{text}");
    assert!(text.contains("slowest operations"), "{text}");
    assert!(text.contains("image.img"), "{text}");
    assert!(text.contains("3.0 GiB  new"), "{text}");
    assert!(text.contains("reports: 5 file(s)"), "{text}");
    assert!(text.contains("Finished"), "{text}");
}

#[test]
fn a_failed_run_shows_its_errors() {
    let status = fixture_status();
    let errors = vec![RunError {
        operation_id: "image:buildroot".into(),
        code: "backend_command_failed".into(),
        message: "make: *** [linux] Error 1".into(),
    }];
    let mut view = RunView::ended(
        &status,
        "failed",
        NOW_MS,
        RunSource::Attached,
        &errors,
        NOW_MS,
    );
    view.ops = live_view().ops;
    assert_eq!(view.phase, RunPhase::Failed);
    let mut ui = DashboardState::default();
    let text = draw(&view, &mut ui, 120, 32);
    assert!(text.contains("Build failed after"), "{text}");
    assert!(text.contains("make: *** [linux] Error 1"), "{text}");
}

#[test]
fn the_operation_popup_explains_the_operation() {
    let view = live_view();
    let mut ui = DashboardState {
        selected: Some("image:buildroot".into()),
        popup: true,
        ..DashboardState::default()
    };
    let text = draw(&view, &mut ui, 120, 32);
    assert!(text.contains("runs because config_changed"), "{text}");
    assert!(text.contains("expected: 2m00s"), "{text}");
    assert!(text.contains("[Esc] close"), "{text}");
    assert!(text.contains("steps"), "{text}");
}

#[test]
fn a_run_with_no_operations_yet_says_so() {
    let mut view = live_view();
    view.ops.clear();
    view.phase = RunPhase::Starting;
    let text = draw(&view, &mut DashboardState::default(), 120, 32);
    assert!(text.contains("no operations planned yet"), "{text}");
    assert!(text.contains("STARTING"), "{text}");
}

#[test]
fn tiny_terminals_get_a_notice_not_a_panic() {
    let view = live_view();
    for (width, height) in [(20, 10), (39, 20), (80, 7)] {
        let text = draw(&view, &mut DashboardState::default(), width, height);
        assert!(
            text.contains("terminal too small"),
            "{width}x{height}: {text}"
        );
    }
    // Too small to say anything: it must still not panic.
    let _ = draw(&view, &mut DashboardState::default(), 1, 1);
    // The smallest size that lays out, with the popup open.
    let mut ui = DashboardState {
        selected: Some("image:buildroot".into()),
        popup: true,
        ..DashboardState::default()
    };
    let _ = draw(&view, &mut ui, 40, 8);
    let _ = draw(&view, &mut ui, 20, 10);
}

#[test]
fn every_size_between_the_smallest_and_a_large_terminal_draws() {
    let view = live_view();
    for (width, height) in [
        (40, 8),
        (80, 24),
        (100, 30),
        (120, 32),
        (160, 45),
        (200, 60),
    ] {
        for tab in [
            ChartTab::Progress,
            ChartTab::Resources,
            ChartTab::Steps,
            ChartTab::Packages,
        ] {
            let mut ui = DashboardState {
                tab,
                charts_narrow: true,
                log_height: 12,
                ..DashboardState::default()
            };
            let _ = draw(&view, &mut ui, width, height);
        }
    }
}

#[test]
fn filter_sort_and_selection_keys_move_the_table() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    ui.sync(&view);
    assert_eq!(
        ui.selected.as_deref(),
        Some("image:buildroot"),
        "follows the running op"
    );

    ui.handle_key(KeyCode::Char('j'), none, Some(&view));
    assert!(!ui.follow, "moving the cursor stops following");
    assert_eq!(ui.selected.as_deref(), Some("image:assemble"));
    ui.handle_key(KeyCode::Down, none, Some(&view));
    assert_eq!(
        ui.selected.as_deref(),
        Some("image:assemble"),
        "clamped at the end"
    );

    let action = ui.handle_key(KeyCode::Char('f'), none, Some(&view));
    assert_eq!(action, DashAction::Notice("filter: running".into()));
    ui.sync(&view);
    assert_eq!(
        ui.selected.as_deref(),
        Some("image:buildroot"),
        "only running is listed"
    );

    ui.handle_key(KeyCode::Char('s'), none, Some(&view));
    assert_eq!(ui.sort, crate::tui::run_view::OpSort::Duration);
}

#[test]
fn the_tab_keys_pick_charts_and_wrap() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    ui.handle_key(KeyCode::Char('4'), none, Some(&view));
    assert_eq!(ui.tab, ChartTab::Packages);
    ui.handle_key(KeyCode::Right, none, Some(&view));
    assert_eq!(ui.tab, ChartTab::Summary, "the summary is the fifth tab");
    ui.handle_key(KeyCode::Right, none, Some(&view));
    assert_eq!(ui.tab, ChartTab::Progress, "wraps to the first tab");
    // Five keys exist whether or not the run is over.
    ui.handle_key(KeyCode::Char('5'), none, Some(&view));
    assert_eq!(ui.tab, ChartTab::Summary);
}

#[test]
fn pause_and_cancel_need_a_second_key_to_cancel() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    assert_eq!(
        ui.handle_key(KeyCode::Char('p'), none, Some(&view)),
        DashAction::Pause
    );
    assert_eq!(
        ui.handle_key(KeyCode::Char('c'), none, Some(&view)),
        DashAction::CancelArmed
    );
    assert_eq!(
        ui.handle_key(KeyCode::Char('x'), none, Some(&view)),
        DashAction::Notice("cancel not confirmed; the build keeps running".into())
    );
    ui.handle_key(KeyCode::Char('c'), none, Some(&view));
    assert_eq!(
        ui.handle_key(KeyCode::Char('y'), none, Some(&view)),
        DashAction::Cancel
    );

    let mut paused = live_view();
    paused.phase = RunPhase::Paused;
    assert_eq!(
        ui.handle_key(KeyCode::Char('p'), none, Some(&paused)),
        DashAction::Resume
    );
    let over = RunView::ended(
        &fixture_status(),
        "completed",
        NOW_MS,
        RunSource::Monitor,
        &[],
        NOW_MS,
    );
    assert!(matches!(
        ui.handle_key(KeyCode::Char('c'), none, Some(&over)),
        DashAction::Notice(_)
    ));
}

#[test]
fn output_keys_follow_scroll_and_size() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    ui.handle_key(KeyCode::PageUp, none, Some(&view));
    assert!(!ui.log_follow);
    assert_eq!(ui.log_scroll, 5);
    ui.handle_key(KeyCode::End, none, Some(&view));
    assert!(ui.log_follow && ui.log_scroll == 0);
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    ui.handle_key(KeyCode::Char('-'), none, Some(&view));
    assert_eq!(ui.log_height, 3, "never below three rows");
    ui.handle_key(KeyCode::Char('/'), none, Some(&view));
    for ch in "linux".chars() {
        ui.handle_key(KeyCode::Char(ch), none, Some(&view));
    }
    ui.handle_key(KeyCode::Enter, none, Some(&view));
    assert_eq!(ui.search.as_deref(), Some("linux"));
    assert!(!ui.search_typing);
    let text = draw(&view, &mut ui, 120, 32);
    assert!(text.contains("search \"linux\": 1 line(s)"), "{text}");
}

#[test]
fn escape_closes_the_popup_before_leaving() {
    let view = live_view();
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    ui.sync(&view);
    ui.handle_key(KeyCode::Enter, none, Some(&view));
    assert!(ui.popup);
    assert_eq!(
        ui.handle_key(KeyCode::Esc, none, Some(&view)),
        DashAction::None
    );
    assert!(!ui.popup);
    assert_eq!(
        ui.handle_key(KeyCode::Esc, none, Some(&view)),
        DashAction::Back
    );
}

#[test]
fn a_running_build_without_a_view_only_leaves() {
    let mut ui = DashboardState::default();
    let none = KeyModifiers::empty();
    assert_eq!(
        ui.handle_key(KeyCode::Char('b'), none, None),
        DashAction::Picker
    );
    assert_eq!(
        ui.handle_key(KeyCode::Char('p'), none, None),
        DashAction::None
    );
}

#[test]
fn an_attached_view_from_a_snapshot_renders_the_same_way() {
    let status = fixture_status();
    let view = RunView::live(&status, RunSource::Attached, true, 1_100_000);
    let text = draw(&view, &mut DashboardState::default(), 120, 32);
    assert!(text.contains("PAUSED"), "{text}");
    assert!(text.contains("artifact:cli"), "{text}");
    assert!(text.contains("cargo build finished"), "{text}");
}
