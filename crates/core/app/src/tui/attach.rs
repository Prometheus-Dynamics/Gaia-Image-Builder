//! Attaching to a `gaia run` that another process started: when a build has a
//! live run, the TUI shows its run dashboard (built from the run's live status
//! file, read each second) instead of offering to start one. The picker marks
//! the builds that are running. Pause, resume and cancel go to the run's pid;
//! quitting the monitor never stops the build.

use super::*;
use crate::AppCommand;
use crate::commands::live_status::{
    LiveRun, RunFiles, live_run, live_run_at, read_last, read_last_at, unix_now_ms,
};
use crate::commands::run_registry::{RegisteredRun, list_runs};
use crate::tui::dashboard;
use crate::tui::run_view::{RunSource, RunView};
use crate::tui::widgets::chrome;

/// How often the attached monitor re-reads the run's files.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often the picker re-checks which builds run.
const PICKER_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct AttachState {
    /// The files of the run, by absolute path.
    pub(crate) files: RunFiles,
    /// The live run, `None` once it has ended.
    pub(crate) run: Option<LiveRun>,
    /// The run as the dashboard shows it; `None` when there is no run to show.
    pub(crate) view: Option<RunView>,
    pub(crate) polled_at: Instant,
}

impl AttachState {
    pub(crate) fn load(files: RunFiles) -> Self {
        let mut attach = Self {
            files,
            run: None,
            view: None,
            polled_at: Instant::now(),
        };
        attach.poll();
        attach
    }

    fn poll(&mut self) {
        let now = unix_now_ms();
        self.run = live_run_at(&self.files);
        let ended = match self.run {
            Some(_) => None,
            None => read_last_at(&self.files.last),
        };
        self.view = match (&self.run, ended) {
            (Some(run), _) => Some(match &run.status {
                Some(status) => RunView::live(status, RunSource::Attached, run.paused(), now),
                None => RunView::starting(Some(run.pid), RunSource::Attached, now),
            }),
            (None, Some(last)) => Some(RunView::from_last(&last, RunSource::Attached, now)),
            (None, None) => None,
        };
        self.polled_at = Instant::now();
    }
}

impl<'a> TuiState<'a> {
    /// Opens the monitor when the build has a live run. Returns whether it did.
    pub(crate) fn attach_if_live(&mut self) -> bool {
        let Some(spec) = self.spec.as_ref() else {
            return false;
        };
        let build_dir = PathBuf::from(&spec.workspace.build_dir);
        if live_run(&build_dir).is_none() {
            return false;
        }
        self.attach_to(RunFiles::in_build_dir(&build_dir));
        true
    }

    /// Opens the monitor of a registered run, live or ended, from its
    /// registry entry: no build config is resolved.
    pub(crate) fn attach_registered(&mut self, run: &RegisteredRun) {
        self.attach_to(run.files());
    }

    fn attach_to(&mut self, files: RunFiles) {
        self.attach = Some(AttachState::load(files));
        self.dash = Default::default();
        self.screen = Screen::Attach;
        self.set_status("attached to a gaia run; q leaves the monitor, the build keeps running");
    }

    pub(crate) fn open_picker(&mut self) {
        self.screen = Screen::Picker;
        self.attach = None;
        self.picker_polled_at = None;
    }

    /// Re-reads the attached run, or the running builds of the picker, at
    /// their intervals.
    pub(crate) fn poll_live(&mut self) {
        match self.screen {
            Screen::Attach => {
                if let Some(attach) = self.attach.as_mut()
                    && attach.polled_at.elapsed() >= POLL_INTERVAL
                {
                    attach.poll();
                }
            }
            Screen::Picker => {
                if self
                    .picker_polled_at
                    .is_none_or(|at| at.elapsed() >= PICKER_POLL_INTERVAL)
                {
                    self.refresh_picker_marks();
                }
            }
            Screen::Setup | Screen::Monitor | Screen::Details => {}
        }
    }

    /// Re-reads the registered runs, and for each build config its live run
    /// (if any) and its last recorded run.
    fn refresh_picker_marks(&mut self) {
        self.runs = list_runs();
        self.ensure_picker_selection();
        let options = self.options.clone();
        let mut marks = std::collections::BTreeMap::new();
        let mut last = std::collections::BTreeMap::new();
        for entry in &self.build_entries {
            let meta = self
                .build_meta
                .entry(entry.path.clone())
                .or_insert_with(|| {
                    try_resolve_config_with_options(&entry.path, &options)
                        .ok()
                        .map(|spec| BuildMeta {
                            build_dir: PathBuf::from(&spec.workspace.build_dir),
                            target: spec.metadata.target.clone(),
                            profile: spec.metadata.profile.clone(),
                        })
                })
                .clone();
            let Some(meta) = meta else {
                continue;
            };
            if let Some(run) = live_run(&meta.build_dir) {
                marks.insert(entry.path.clone(), run);
            }
            if let Some(ended) = read_last(&meta.build_dir) {
                last.insert(entry.path.clone(), ended);
            }
        }
        self.live_marks = marks;
        self.build_last = last;
        self.picker_polled_at = Some(Instant::now());
    }

    fn attached_pid(&self) -> Option<u32> {
        self.attach
            .as_ref()
            .and_then(|attach| attach.run.as_ref())
            .map(|run| run.pid)
    }

    /// Sends a command to the attached run's process.
    pub(crate) fn signal_attached(&mut self, command: AppCommand, action: &str) {
        let Some(pid) = self.attached_pid() else {
            self.set_status("no live run to signal; the build has ended");
            return;
        };
        #[cfg(unix)]
        let result = crate::commands::control::signal_run(pid, command);
        #[cfg(not(unix))]
        let result: Result<(), String> = {
            let _ = command;
            Err("pause, resume and cancel need a Unix system".into())
        };
        match result {
            Ok(()) => self.set_status(format!("{action} (gaia run {pid})")),
            Err(error) => self.set_status(error),
        }
        if let Some(attach) = self.attach.as_mut() {
            attach.poll();
        }
    }
}

pub(crate) fn render_attach(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let TuiState {
        attach,
        dash,
        metrics,
        ..
    } = state;
    let Some(view) = attach.as_ref().and_then(|attach| attach.view.as_ref()) else {
        chrome::empty_state(
            frame,
            area,
            "Attached",
            &[
                "no run is live and no final snapshot was found",
                "press b to pick a build",
            ],
        );
        return;
    };
    let tail = |id: &str| view.op_log_lines(id);
    dashboard::render(frame, area, view, dash, metrics, &tail);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::live_status::LastRun;
    use crate::commands::live_status::fixture_status;
    use ratatui::backend::TestBackend;

    fn live_fixture(stopped: bool) -> LiveRun {
        LiveRun {
            pid: 4242,
            status: Some(fixture_status()),
            stopped,
        }
    }

    fn attach_fixture(run: LiveRun) -> AttachState {
        let view = match &run.status {
            Some(status) => RunView::live(status, RunSource::Attached, run.paused(), 1_100_000),
            None => RunView::starting(Some(run.pid), RunSource::Attached, 1_100_000),
        };
        AttachState {
            files: RunFiles::in_build_dir(&PathBuf::new()),
            run: Some(run),
            view: Some(view),
            polled_at: Instant::now(),
        }
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn attached_state(run: LiveRun) -> TuiState<'static> {
        let context: &'static AppContext = Box::leak(Box::new(AppContext::with_defaults()));
        let mut state = TuiState::new(
            context,
            TuiLaunch {
                build: "missing-build.toml",
                build_explicit: true,
                builds_dir: None,
            },
            &ResolveOptions::default(),
        );
        state.screen = Screen::Attach;
        state.attach = Some(attach_fixture(run));
        state
    }

    #[test]
    fn attached_monitor_renders_the_run_dashboard() {
        let mut state = attached_state(live_fixture(true));
        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(120, 32)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &mut state))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("PAUSED"), "{text}");
        assert!(text.contains("Demo build"), "{text}");
        assert!(text.contains("ops 3/10"), "{text}");
        assert!(text.contains("image:buildroot"), "{text}");
        assert!(text.contains("artifact:cli"), "{text}");
        assert!(text.contains("cargo build finished"), "{text}");
        assert!(text.contains("[p] pause/resume"), "{text}");
    }

    #[test]
    fn attached_monitor_says_when_no_run_is_left() {
        let mut state = attached_state(live_fixture(false));
        state.attach = Some(AttachState {
            files: RunFiles::in_build_dir(&PathBuf::new()),
            run: None,
            view: None,
            polled_at: Instant::now(),
        });
        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(100, 20)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &mut state))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("no run is live"), "{text}");
    }

    #[test]
    fn an_ended_run_shows_its_outcome_as_a_view() {
        let mut status = fixture_status();
        status.running.clear();
        let last = LastRun {
            outcome: "failed".into(),
            ended_at: 1_200,
            status,
            errors: vec![crate::commands::live_status::RunError {
                operation_id: "artifact:kernel".into(),
                code: "backend_command_failed".into(),
                message: "make failed".into(),
            }],
        };
        let view = RunView::from_last(&last, RunSource::Attached, 1_300_000);
        assert_eq!(view.phase, crate::tui::run_view::RunPhase::Failed);
        assert_eq!(view.errors.len(), 1);
    }

    #[test]
    fn cancel_needs_a_second_confirming_key() {
        let mut state = attached_state(live_fixture(false));

        state.handle_dashboard_key(KeyCode::Char('c'), KeyModifiers::empty());
        assert!(state.dash.cancel_armed);
        // Any other key disarms it without signalling the run.
        state.handle_dashboard_key(KeyCode::Char('x'), KeyModifiers::empty());
        assert!(!state.dash.cancel_armed);
        assert_eq!(
            state.footer_notice(),
            Some("cancel not confirmed; the build keeps running")
        );
    }
}
