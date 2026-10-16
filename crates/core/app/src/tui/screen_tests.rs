//! Render tests of every screen on a test backend, at the sizes the TUI must
//! work at, on the example build config resolved for real. Each test asserts
//! the strings a person reads on the screen; the sizes check that nothing
//! panics, from a 20x10 terminal up to 160x45.

use crossterm::event::{KeyCode, KeyModifiers};
use gaia_config::ResolveOptions;
use gaia_testkit::TempDir;
use gaia_validate::{DiagnosticSeverity, ValidationDiagnostic, ValidationReport};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;

use super::*;
use crate::commands::live_status::{LiveRun, RunFiles};
use crate::commands::run_registry::ListedRun;
use crate::tui::details::PlanFilter;
use crate::tui::run_view::RunView;
use crate::tui::run_view::tests::fixture_view;
use crate::tui::setup_view::start_blocker;

/// The example build config of the repository.
fn example_build() -> String {
    format!(
        "{}/../../../examples/default-workspace/configs/default.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// The example build, resolved into a scratch dir, on `screen`. Its build
/// state (spec, validation, plan) is loaded, as the TUI loads it.
fn example_state(screen: Screen) -> (TempDir, TuiState<'static>) {
    let context: &'static AppContext = Box::leak(Box::new(AppContext::with_defaults()));
    let scratch = TempDir::new("gaia-tui-screens");
    let root = scratch.join("root").display().to_string();
    let out = scratch.join("out").display().to_string();
    let build = scratch.join("build").display().to_string();
    let options = ResolveOptions {
        explicit_overrides: vec![
            ("workspace.root_dir".into(), root),
            ("workspace.out_dir".into(), out),
            ("workspace.build_dir".into(), build),
        ],
        ..ResolveOptions::default()
    };
    let mut state = TuiState::new(
        context,
        TuiLaunch {
            build: &example_build(),
            build_explicit: true,
            builds_dir: None,
        },
        &options,
    );
    state.refresh();
    state.screen = screen;
    (scratch, state)
}

/// The text of a drawn buffer, one line per row.
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

/// The whole TUI drawn at `width` x `height`, as text.
pub(crate) fn draw(state: &mut TuiState<'_>, width: u16, height: u16) -> String {
    let mut terminal =
        ratatui::Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| render(frame, state)).expect("draw");
    buffer_text(terminal.backend().buffer())
}

/// The two sizes the screens are checked at.
const SIZES: [(u16, u16); 2] = [(80, 24), (160, 45)];

#[test]
fn every_screen_draws_at_every_size_without_panicking() {
    for screen in [
        Screen::Picker,
        Screen::Setup,
        Screen::Details,
        Screen::Monitor,
        Screen::Attach,
    ] {
        let (_scratch, mut state) = example_state(screen);
        for (width, height) in [(20, 10), (40, 8), SIZES[0], SIZES[1], (1, 1)] {
            let _ = draw(&mut state, width, height);
        }
    }
}

#[test]
fn every_screen_has_a_header_and_a_footer() {
    for (screen, title) in [
        (Screen::Setup, "Setup"),
        (Screen::Details, "Details"),
        (Screen::Picker, "Start"),
    ] {
        let (_scratch, mut state) = example_state(screen);
        for (width, height) in SIZES {
            let text = draw(&mut state, width, height);
            let first = text.lines().next().unwrap_or("");
            assert!(first.contains("Gaia"), "{first}");
            assert!(first.contains(title), "{first}");
            assert!(text.contains("[?] help"), "{text}");
        }
    }
}

#[test]
fn setup_shows_the_form_and_the_plan_preview() {
    let (_scratch, mut state) = example_state(Screen::Setup);
    for (width, height) in SIZES {
        let text = draw(&mut state, width, height);
        assert!(text.contains("Setup"), "{text}");
        assert!(text.contains("Plan preview"), "{text}");
        assert!(text.contains("Operations"), "{text}");
        assert!(text.contains("Why they run"), "{text}");
        assert!(text.contains("Validation"), "{text}");
        assert!(
            text.contains("START BUILD") || text.contains("CANNOT START"),
            "{text}"
        );
    }
}

#[test]
fn the_start_button_says_why_a_build_cannot_start() {
    let (_scratch, mut state) = example_state(Screen::Setup);
    state.validation = Some(ValidationReport {
        errors: vec!["image.kind is not set".into()],
        diagnostics: vec![ValidationDiagnostic {
            severity: DiagnosticSeverity::Error,
            code: "image_kind",
            message: "image.kind is not set".into(),
            location: Some("image".into()),
        }],
        ..ValidationReport::default()
    });
    assert!(start_blocker(&state).is_some());
    let text = draw(&mut state, 160, 45);
    assert!(text.contains("CANNOT START"), "{text}");
    assert!(text.contains("blocked"), "{text}");
    // The badge of a form item that names the error.
    assert!(text.contains("✗"), "{text}");
}

#[test]
fn details_tabs_show_their_content_at_both_sizes() {
    let (_scratch, mut state) = example_state(Screen::Details);
    let expectations = [
        (DetailTab::Plan, "Decision"),
        (DetailTab::Validation, "errors "),
        (DetailTab::Reports, "no run of this build has finished yet"),
        (DetailTab::Spec, "Build"),
        (DetailTab::Events, "no output yet"),
    ];
    for (tab, expected) in expectations {
        state.open_details(tab);
        for (width, height) in SIZES {
            let text = draw(&mut state, width, height);
            assert!(text.contains("1 Plan"), "{text}");
            assert!(text.contains("5 Events"), "{text}");
            assert!(text.contains(expected), "{tab:?}: {text}");
        }
    }
}

#[test]
fn the_plan_table_filters_between_run_and_reuse() {
    let (_scratch, mut state) = example_state(Screen::Details);
    state.open_details(DetailTab::Plan);
    assert_eq!(state.details.plan_filter, PlanFilter::All);
    state.handle_key(KeyCode::Char('f'), KeyModifiers::empty());
    assert_eq!(state.details.plan_filter, PlanFilter::Run);
    let text = draw(&mut state, 160, 45);
    assert!(text.contains("filter run"), "{text}");
    state.handle_key(KeyCode::Char('f'), KeyModifiers::empty());
    state.handle_key(KeyCode::Char('f'), KeyModifiers::empty());
    assert_eq!(state.details.plan_filter, PlanFilter::All);
}

#[test]
fn details_keys_move_between_tabs_and_back_to_setup() {
    let (_scratch, mut state) = example_state(Screen::Setup);
    state.handle_key(KeyCode::Char('d'), KeyModifiers::empty());
    assert_eq!(state.screen, Screen::Details);
    assert_eq!(state.details.tab(), DetailTab::Plan);
    state.handle_key(KeyCode::Right, KeyModifiers::empty());
    assert_eq!(state.details.tab(), DetailTab::Validation);
    state.handle_key(KeyCode::Char('5'), KeyModifiers::empty());
    assert_eq!(state.details.tab(), DetailTab::Events);
    state.handle_key(KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(state.screen, Screen::Setup);
}

#[test]
fn events_search_highlights_and_counts_matching_lines() {
    let (_scratch, mut state) = example_state(Screen::Details);
    state.attach = Some(attach_with(fixture_view(1_060_000)));
    state.open_details(DetailTab::Events);
    state.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
    for ch in "cargo".chars() {
        state.handle_key(KeyCode::Char(ch), KeyModifiers::empty());
    }
    let text = draw(&mut state, 160, 45);
    assert!(text.contains("search: cargo_"), "{text}");
    assert!(text.contains("cargo build finished"), "{text}");
    state.handle_key(KeyCode::Enter, KeyModifiers::empty());
    let text = draw(&mut state, 160, 45);
    assert!(text.contains("search \"cargo\": 1 line(s)"), "{text}");
}

#[test]
fn reports_summarise_the_last_run_with_bars_and_steps() {
    let (_scratch, mut state) = example_state(Screen::Details);
    state.attach = Some(attach_with(fixture_view(1_060_000)));
    state.open_details(DetailTab::Reports);
    for (width, height) in SIZES {
        let text = draw(&mut state, width, height);
        assert!(text.contains("Slowest operations"), "{text}");
        assert!(text.contains("Steps of image:buildroot"), "{text}");
        assert!(text.contains("Output"), "{text}");
    }
}

/// An attached run that is a fixture view: the details read it as the run.
fn attach_with(view: RunView) -> AttachState {
    AttachState {
        files: RunFiles::in_build_dir(&PathBuf::new()),
        run: None,
        view: Some(view),
        polled_at: std::time::Instant::now(),
    }
}

#[test]
fn the_help_overlay_lists_the_keys_of_the_screen() {
    let (_scratch, mut state) = example_state(Screen::Details);
    state.handle_key(KeyCode::Char('?'), KeyModifiers::empty());
    let text = draw(&mut state, 160, 45);
    assert!(text.contains("Details"), "{text}");
    assert!(text.contains("switch tab"), "{text}");
    assert!(text.contains("any key closes this"), "{text}");
}

#[test]
fn the_footer_of_the_charts_names_the_keys_the_tabs_take() {
    assert!(crate::tui::dashboard::footer_hint(false).contains("[←→/1-5] charts"));
    assert!(crate::tui::dashboard::footer_hint(true).contains("[←→/1-5] charts"));
}

/// Writes the renders the phase 2 review reads: the start screen, setup, the
/// plan and reports tabs, at 80x24 and 160x45, into `GAIA_TUI_RENDER_DIR`.
/// Run with `--ignored` and the variable set.
#[test]
#[ignore = "writes render files for review; run with --ignored"]
fn dump_phase2_renders() {
    use crate::commands::live_status::fixture_status;
    use crate::commands::run_registry::fixture_registered_run;
    let Some(dir) = std::env::var_os("GAIA_TUI_RENDER_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let write = |name: &str, state: &mut TuiState<'_>| {
        for (width, height) in SIZES {
            let text = draw(state, width, height);
            let file = dir.join(format!("phase2_{name}_{width}x{height}.txt"));
            std::fs::write(file, text).expect("render file");
        }
    };

    let (_scratch, mut picker) = example_state(Screen::Picker);
    picker.poll_live();
    let mut status = fixture_status();
    status.pid = 4242;
    picker.runs = vec![ListedRun {
        run: fixture_registered_run(4242, "Cm5 image"),
        live: Some(LiveRun {
            pid: 4242,
            status: Some(status),
            stopped: false,
        }),
        ended: None,
    }];
    picker.ensure_picker_selection();
    write("picker", &mut picker);

    let (_scratch, mut setup) = example_state(Screen::Setup);
    write("setup", &mut setup);

    let (_scratch, mut plan) = example_state(Screen::Details);
    plan.open_details(DetailTab::Plan);
    write("plan", &mut plan);

    let (_scratch, mut reports) = example_state(Screen::Details);
    reports.attach = Some(attach_with(fixture_view(1_060_000)));
    reports.open_details(DetailTab::Reports);
    write("reports", &mut reports);
}
