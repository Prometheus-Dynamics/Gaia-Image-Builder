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

use crate::commands::{CommandOutcome, RunArtifacts, load_reuse_state, save_reuse_state};
use crate::{AppContext, backend_overview_lines, runtime_overview_lines};

pub fn run_tui_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
) -> CommandOutcome {
    match launch_tui(context, build, options) {
        Ok((exit_code, summary)) => CommandOutcome::TuiExited { summary, exit_code },
        Err(error) => CommandOutcome::Failed {
            message: format!("failed to launch tui: {error}"),
        },
    }
}

fn launch_tui(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
) -> io::Result<(i32, String)> {
    let mut state = TuiState::new(context, build, options);
    state.refresh();

    let mut terminal = setup_terminal()?;
    let exit_code = run_loop(&mut terminal, &mut state)?;
    restore_terminal(&mut terminal)?;
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

        match key.code {
            KeyCode::Char('q') => return Ok(state.exit_code()),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(state.exit_code());
            }
            _ => state.handle_key(key.code, key.modifiers),
        }
        state.tick();
    }
}

mod details;
mod discovery;
mod input;
mod model;
mod render;
mod run;
mod setup;
mod state;
mod status;

pub(crate) use discovery::*;
pub(crate) use model::*;
pub(crate) use render::*;
pub(crate) use state::*;
