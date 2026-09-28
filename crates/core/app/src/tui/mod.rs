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
use gaia_plan::{ExecutionPlan, PlannedOperation, plan_build_with_reuse_state};
use gaia_report::{ReportFileKind, generate_report, write_report_bundle};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::{ValidationReport, validate_spec_with_providers};
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};

use crate::commands::{
    CommandOutcome, RunArtifacts, load_operation_durations, load_reuse_state, save_reuse_state,
};
use crate::{AppContext, backend_overview_lines, runtime_overview_lines};

/// How the TUI was invoked from the command line.
pub struct TuiLaunch<'a> {
    pub build: &'a str,
    /// Whether the user named a build config, as opposed to a default.
    pub build_explicit: bool,
    /// Directory to scan for build entrypoints instead of `configs/builds`.
    pub builds_dir: Option<&'a str>,
}

pub fn run_tui_command(
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
    state.refresh();

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
        terminal.draw(|frame| render(frame, state))?;

        if !event::poll(Duration::from_millis(100))? {
            state.tick();
            continue;
        }

        let Event::Key(key) = event::read()? else {
            state.tick();
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

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

mod details;
mod discovery;
mod events;
mod input;
mod model;
mod plan_view;
mod render;
mod run;
mod setup;
mod state;
mod status;

pub(crate) use discovery::*;
pub(crate) use events::*;
pub(crate) use model::*;
pub(crate) use render::*;
pub(crate) use state::*;
