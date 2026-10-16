//! Connects the dashboard to the TUI: the run the monitor shows (built from
//! the run's tracker), and what the dashboard's keys do to the run.

use crossterm::event::{KeyCode, KeyModifiers};

use super::DashAction;
use crate::AppCommand;
use crate::commands::live_status::unix_now_ms;
use crate::tui::run_view::{RunOutcome, RunPhase, RunSource, RunView};
use crate::tui::{QuitState, RunState, Screen, TuiState};

/// How a run this TUI started ended: its outcome word and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunEnd {
    pub(crate) outcome: String,
    pub(crate) ended_ms: u64,
}

impl<'a> TuiState<'a> {
    /// The run the monitor shows: the run this TUI started, live or ended.
    /// `None` before any run.
    pub(crate) fn monitor_run_view(&self) -> Option<RunView> {
        let tracker = self.tracker.as_ref()?;
        let now = unix_now_ms();
        let paused = gaia_process::is_paused();
        let status = tracker.snapshot(paused, now);
        let view = match &self.run_end {
            Some(end) => RunView::ended(
                &status,
                &end.outcome,
                end.ended_ms,
                RunSource::Monitor,
                &[],
                now,
            ),
            None => {
                let mut view = RunView::live(&status, RunSource::Monitor, paused, now);
                if self.cancelling || self.quit == QuitState::CancelThenExit {
                    view.phase = RunPhase::Cancelling;
                }
                view
            }
        };
        Some(view.with_outcome(self.monitor_outcome()))
    }

    /// What the run's report says once the run is over.
    fn monitor_outcome(&self) -> RunOutcome {
        let Some(run) = self.last_run.as_ref() else {
            return RunOutcome::default();
        };
        RunOutcome {
            sizes: run.report.summary.sizes.clone(),
            out_dir: Some(run.spec.workspace.out_dir.clone()),
            report_files: run.report_outputs.files.len(),
        }
    }

    /// A key on the monitor or the attached run.
    pub(crate) fn handle_dashboard_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let action = match self.screen {
            Screen::Monitor => {
                let view = self.monitor_run_view();
                self.dash.handle_key(code, modifiers, view.as_ref())
            }
            Screen::Attach => {
                let view = self.attach.as_ref().and_then(|attach| attach.view.clone());
                self.dash.handle_key(code, modifiers, view.as_ref())
            }
            Screen::Picker | Screen::Setup | Screen::Details => return,
        };
        self.execute_dash_action(action);
    }

    fn execute_dash_action(&mut self, action: DashAction) {
        let attached = self.screen == Screen::Attach;
        match action {
            DashAction::None => {}
            DashAction::Notice(text) => self.set_status(text),
            DashAction::CancelArmed => self.set_status(
                "press c or y again to cancel the build; any other key keeps it running",
            ),
            DashAction::Pause if attached => {
                self.signal_attached(AppCommand::Pause, "pausing the build")
            }
            DashAction::Resume if attached => {
                self.signal_attached(AppCommand::Resume, "resuming the build")
            }
            DashAction::Pause => {
                gaia_process::request_pause();
                self.set_status("pausing the build");
            }
            DashAction::Resume => {
                gaia_process::request_resume();
                self.set_status("resuming the build");
            }
            DashAction::Cancel if attached => {
                self.signal_attached(AppCommand::Cancel, "cancelling the build")
            }
            DashAction::Cancel => self.cancel_run(),
            DashAction::Back => {
                if self.screen == Screen::Monitor {
                    self.screen = Screen::Setup;
                    if matches!(self.run_state, RunState::Running { .. }) {
                        self.set_status("build still running; press m to return to the monitor");
                    }
                }
            }
            DashAction::Picker => self.open_picker(),
        }
    }
}
