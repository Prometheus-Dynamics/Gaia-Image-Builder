//! The frame every screen sits in: a header line naming the screen and what it
//! is about, the screen's body, and the footer with its key hints. Below the
//! smallest size a notice replaces the screens.

use super::*;
use crate::tui::dashboard;
use crate::tui::details;
use crate::tui::picker;
use crate::tui::run_view::RunPhase;
use crate::tui::setup_view;
use crate::tui::widgets::badge::phase_badge;
use crate::tui::widgets::chrome;
use crate::tui::widgets::help::{HelpSection, render_help as draw_help};
use ratatui::text::Span;

pub(crate) fn render(frame: &mut Frame<'_>, state: &mut TuiState<'_>) {
    let area = frame.area();
    if !chrome::fits_screen(area) {
        chrome::too_small(frame, area);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    chrome::screen_header(frame, rows[0], screen_title(state), header_subject(state));
    match state.screen {
        Screen::Picker => picker::render(frame, rows[1], state),
        Screen::Setup => setup_view::render(frame, rows[1], state),
        Screen::Details => details::render(frame, rows[1], state),
        Screen::Monitor => render_monitor(frame, rows[1], state),
        Screen::Attach => render_attach(frame, rows[1], state),
    }
    render_footer(frame, rows[2], state);
    if state.show_help {
        render_help(frame, area, state);
    }
}

const HELP_GLOBAL: &[(&str, &str)] = &[
    ("?", "toggle this help"),
    ("q / Ctrl+C", "quit (asks first while a build is running)"),
];
const HELP_PICKER: &[(&str, &str)] = &[
    (
        "j/k Up/Down",
        "move through the builds and the build configs",
    ),
    ("Enter", "attach to a build, or open a build config"),
    ("r", "rescan build files and running builds"),
];
const HELP_SETUP: &[(&str, &str)] = &[
    ("j/k Up/Down", "move through the form"),
    (
        "Left/Right",
        "change the value (target, profile, inputs, variant, jobs)",
    ),
    (
        "Enter",
        "edit the value, or open Plan, Validation, Reports or Spec",
    ),
    (
        "s / r",
        "start the build (when the plan preview says it can start)",
    ),
    ("d", "the details: plan, validation, reports, spec, events"),
    ("m", "back to the monitor"),
    ("b", "pick a different build"),
    ("p", "refresh resolve, validate and plan"),
];
const HELP_DETAILS: &[(&str, &str)] = &[
    ("Left/Right Tab", "switch tab (or 1-5)"),
    ("Up/Down PgUp/PgDn", "scroll; in Plan, move the cursor"),
    ("Home/End", "top or bottom of the tab"),
    ("f", "plan: all, run or reuse only"),
    ("/", "events: search the output (Enter keeps, Esc clears)"),
    ("o", "events: the output of one operation, then all"),
    ("F", "events: follow the newest line, or hold it"),
    ("s / m / b", "start the build / monitor / pick a build"),
    ("Esc", "back to setup"),
];
/// The run dashboard, shared by the monitor and the attached run.
const HELP_DASHBOARD: &[(&str, &str)] = &[
    (
        "j/k Up/Down",
        "select an operation (stops following the running one)",
    ),
    (
        "Enter",
        "details of the operation: why it runs, its time, output",
    ),
    (
        "f / s",
        "filter (all, running, failed, slow) / sort (plan, duration, delta)",
    ),
    (
        "Left/Right Tab",
        "switch chart: Progress, Resources, Steps, Packages, Summary",
    ),
    ("1-5", "pick a chart by its number"),
    (
        "v",
        "narrow terminals: show the charts instead of the table",
    ),
    (
        "PgUp/PgDn Home/End",
        "scroll the output (End follows the newest line)",
    ),
    ("F", "output: follow the newest line, or hold it in place"),
    ("l", "output: only the selected operation"),
    ("/", "search the output (Enter keeps it, Esc clears it)"),
    ("+ / -", "output pane height"),
    ("p", "pause or resume the build"),
    ("c", "cancel the build (c or y to confirm)"),
];
const HELP_MONITOR: &[(&str, &str)] = &[("Esc", "back to setup (the build keeps running)")];
const HELP_ATTACH: &[(&str, &str)] = &[
    ("b", "pick another build"),
    ("q", "leave the monitor; the build keeps running"),
];

pub(crate) fn render_help(frame: &mut Frame<'_>, area: Rect, state: &TuiState<'_>) {
    let sections = match state.screen {
        Screen::Picker => vec![HelpSection {
            title: "Start screen",
            keys: HELP_PICKER,
        }],
        Screen::Setup => vec![HelpSection {
            title: "Setup",
            keys: HELP_SETUP,
        }],
        Screen::Details => vec![HelpSection {
            title: "Details",
            keys: HELP_DETAILS,
        }],
        Screen::Monitor => vec![
            HelpSection {
                title: "Run",
                keys: HELP_DASHBOARD,
            },
            HelpSection {
                title: "Monitor",
                keys: HELP_MONITOR,
            },
        ],
        Screen::Attach => vec![
            HelpSection {
                title: "Run",
                keys: HELP_DASHBOARD,
            },
            HelpSection {
                title: "Attached build",
                keys: HELP_ATTACH,
            },
        ],
    };
    let mut all = sections;
    all.push(HelpSection {
        title: "Global",
        keys: HELP_GLOBAL,
    });
    draw_help(frame, area, &all);
}

/// The name of the screen, in the header.
fn screen_title(state: &TuiState<'_>) -> &'static str {
    match state.screen {
        Screen::Picker => "Start",
        Screen::Setup => "Setup",
        Screen::Details => "Details",
        Screen::Monitor => "Monitor",
        Screen::Attach => "Attached",
    }
}

/// What the screen is about, after the title: the build or the run, and a
/// badge while a build runs from this TUI.
fn header_subject(state: &TuiState<'_>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let name = match state.screen {
        Screen::Picker => format!(
            "{} build config(s), {} running",
            state.build_entries.len(),
            state.runs.iter().filter(|run| run.is_live()).count()
        ),
        Screen::Attach => state
            .attach
            .as_ref()
            .and_then(|attach| attach.view.as_ref())
            .map(|view| view.display_name.clone())
            .unwrap_or_else(|| "no run".to_string()),
        _ => state
            .spec
            .as_ref()
            .map(|spec| spec.identity.display_name.clone())
            .unwrap_or_else(|| {
                std::path::Path::new(&state.build)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "no build".to_string())
            }),
    };
    spans.push(Span::raw(name));
    if matches!(state.run_state, RunState::Running { .. }) {
        spans.push(Span::raw("  "));
        spans.push(phase_badge(RunPhase::Running));
    }
    spans
}

pub(crate) fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiState<'_>) {
    let hint = if state.edit_field.is_some() {
        "[Type] edit  [Backspace] delete  [Enter] apply  [Esc] cancel".to_string()
    } else {
        match state.screen {
            Screen::Picker => {
                "[Enter] open/attach  [j/k] move  [r] rescan  [?] help  [q] quit".into()
            }
            Screen::Setup => {
                "[Enter] select/edit  [←→] value  [s] start  [d] details  [m] monitor  [?] help  [q] quit"
                    .into()
            }
            Screen::Details => format!(
                "{}  [?] help  [q] quit",
                details::footer_hint(state.details.tab())
            ),
            Screen::Monitor => {
                let over = state.monitor_run_view().is_some_and(|view| view.is_over());
                format!("{}  [Esc] setup", dashboard::footer_hint(over))
            }
            Screen::Attach => {
                let over = state
                    .attach
                    .as_ref()
                    .and_then(|attach| attach.view.as_ref())
                    .is_some_and(|view| view.is_over());
                format!("{}  [b] builds", dashboard::footer_hint(over))
            }
        }
    };
    let notice = state.footer_notice();
    let hint = if state.edit_field.is_none() && matches!(state.screen, Screen::Attach) {
        hint.replace("[q] quit", "[q] leave monitor")
    } else {
        hint
    };
    chrome::footer(frame, area, &hint, notice);
}

/// The monitor of the run this TUI started: the run dashboard.
pub(crate) fn render_monitor(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let Some(view) = state.monitor_run_view() else {
        chrome::empty_state(
            frame,
            area,
            "Monitor",
            &[
                "no build has run in this session",
                "press s in setup to start one",
            ],
        );
        return;
    };
    let events = &state.events;
    let tail = |id: &str| {
        let lines = events
            .log(id)
            .map(|log| {
                let skip = log.lines.len().saturating_sub(POPUP_TAIL);
                log.lines.iter().skip(skip).cloned().collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if lines.is_empty() {
            view.op_log_lines(id)
        } else {
            lines
        }
    };
    dashboard::render(frame, area, &view, &mut state.dash, &state.metrics, &tail);
}

/// Output lines the operation popup can take from the monitor's log.
const POPUP_TAIL: usize = 40;
