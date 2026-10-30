use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_exec::{
    ExecutionCancellation, ExecutionEvent, ExecutionProviders,
    execute_plan_with_cancellation_and_observer,
};
use gaia_plan::{ExecutionPlan, plan_build_with_reuse_state};
use gaia_report::{generate_report, write_report_bundle};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::{ValidationReport, validate_spec_with_providers};
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::AppContext;
use crate::commands::{
    CommandOutcome, RunArtifacts, load_operation_durations, load_reuse_state, save_reuse_state,
};

/// How the TUI was invoked from the command line.
pub(crate) struct TuiLaunch<'a> {
    pub build: &'a str,
    /// Whether the user named a build config, as opposed to a default.
    pub build_explicit: bool,
    /// Directory to scan for build entrypoints instead of `configs/builds`.
    pub builds_dir: Option<&'a str>,
}

pub(crate) fn run_tui_command(
    context: &AppContext,
    launch: TuiLaunch<'_>,
    options: &ResolveOptions,
) -> CommandOutcome {
    match launch_tui(context, launch, options) {
        Ok((exit_code, summary)) => CommandOutcome::TuiExited { summary, exit_code },
        Err(error) => CommandOutcome::Failed {
            message: format!("failed to launch tui: {error}"),
        },
    }
}

fn launch_tui(
    context: &AppContext,
    launch: TuiLaunch<'_>,
    options: &ResolveOptions,
) -> io::Result<(i32, String)> {
    let mut state = TuiState::new(context, launch, options);
    // Outside a project the picker opens with no build to load: it shows the
    // running builds only.
    if state.screen != Screen::Picker || Path::new(&state.build).is_file() {
        state.refresh();
    }
    if state.screen != Screen::Picker {
        state.attach_if_live();
    }

    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, &mut state);
    restore_terminal(&mut terminal)?;
    let exit_code = result?;
    Ok((exit_code, state.tui_exit_summary()))
}

fn setup_terminal() -> io::Result<ratatui::Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    ratatui::Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut ratatui::Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn run_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<Stdout>>,
    state: &mut TuiState<'_>,
) -> io::Result<i32> {
    loop {
        state.poll_run_completion();
        if let Some(code) = state.should_exit() {
            return Ok(code);
        }
        // Draw only when the screen changed or an animation is due; the loop
        // still wakes every 100 ms to poll the run, the picker and the attach.
        let now = Instant::now();
        if state.redraw_due(now) {
            terminal.draw(|frame| render(frame, state))?;
            state.redraw_drew(Instant::now());
        }

        if !event::poll(state.poll_timeout(Instant::now()))? {
            state.tick();
            continue;
        }

        let event = event::read()?;
        if matches!(event, Event::Resize(..)) {
            state.request_redraw();
        }
        let Event::Key(key) = event else {
            state.tick();
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        state.request_redraw();

        let is_quit = match key.code {
            KeyCode::Char('c') => key.modifiers.contains(KeyModifiers::CONTROL),
            KeyCode::Char('q') => state.edit_field.is_none(),
            _ => false,
        };
        if is_quit {
            if let Some(code) = state.request_quit() {
                return Ok(code);
            }
        } else {
            state.handle_key(key.code, key.modifiers);
        }
        state.tick();
    }
}

mod attach;
mod dashboard;
mod details;
mod discovery;
mod events;
mod input;
mod metrics;
mod model;
mod picker;
mod redraw;
mod render;
mod run;
mod run_view;
mod setup;
mod setup_view;
mod state;
mod status;
mod theme;
mod widgets;

pub(crate) use attach::*;
pub(crate) use discovery::*;
pub(crate) use events::*;
pub(crate) use model::*;
pub(crate) use render::*;
pub(crate) use state::*;

#[cfg(test)]
mod screen_tests;
