//! Attaching to a `gaia run` that another process started: when a build has a
//! live run, the TUI shows its monitor (the `gaia status` summary, refreshed
//! each second, and the recent output) instead of offering to start one. The
//! picker marks the builds that are running. Pause, resume and cancel go to
//! the run's pid; quitting the monitor never stops the build.

use super::*;
use crate::AppCommand;
use crate::commands::live_status::{
    LastRun, LiveRun, RunFiles, live_run, live_run_at, read_last_at, unix_now,
};
use crate::commands::run_registry::{ListedRun, RegisteredRun, list_runs};
use crate::commands::status::{last_run_lines, live_badge, live_lines, log_lines, run_summary};

/// How often the attached monitor re-reads the run's files.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often the picker re-checks which builds run.
const PICKER_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct AttachState {
    /// The files of the run, by absolute path.
    pub(crate) files: RunFiles,
    /// The live run, `None` once it has ended.
    pub(crate) run: Option<LiveRun>,
    /// How the run ended, once it is no longer live.
    pub(crate) ended: Option<LastRun>,
    pub(crate) summary: Vec<String>,
    pub(crate) logs: Vec<String>,
    pub(crate) log_scroll: u16,
    pub(crate) follow_tail: bool,
    /// `c` was pressed: the next `c` or `y` cancels the build.
    pub(crate) cancel_armed: bool,
    polled_at: Instant,
}

impl AttachState {
    pub(crate) fn load(files: RunFiles) -> Self {
        let mut attach = Self {
            files,
            run: None,
            ended: None,
            summary: Vec::new(),
            logs: Vec::new(),
            log_scroll: 0,
            follow_tail: true,
            cancel_armed: false,
            polled_at: Instant::now(),
        };
        attach.poll();
        attach
    }

    fn poll(&mut self) {
        let now = unix_now();
        self.run = live_run_at(&self.files);
        self.ended = match self.run {
            Some(_) => None,
            None => read_last_at(&self.files.last),
        };
        self.summary = match (&self.run, &self.ended) {
            (Some(run), _) => live_lines(run, now),
            (None, Some(last)) => {
                let mut lines = vec!["no run is live".to_string()];
                lines.extend(last_run_lines(last, now));
                lines
            }
            (None, None) => vec!["no run is live and no final snapshot was found".to_string()],
        };
        let status = self
            .run
            .as_ref()
            .and_then(|run| run.status.as_ref())
            .or(self.ended.as_ref().map(|last| &last.status));
        self.logs = status.map(log_lines).unwrap_or_default();
        self.polled_at = Instant::now();
    }

    fn scroll_up(&mut self, step: u16) {
        self.follow_tail = false;
        self.log_scroll = self.log_scroll.saturating_sub(step);
    }

    fn scroll_down(&mut self, step: u16) {
        self.log_scroll = self.log_scroll.saturating_add(step);
    }
}

/// A row of the running builds: the run on one line, dimmed once it ended.
pub(crate) fn registered_run_line(run: &ListedRun, now: u64) -> Line<'static> {
    let style = if run.is_live() {
        Style::default().fg(Color::LightGreen)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Line::from(Span::styled(run_summary(run, now), style))
}

/// A picker row: the build's label, then its badge when a run is live.
pub(crate) fn picker_line(label: &str, run: Option<&LiveRun>, now: u64) -> Line<'static> {
    match run {
        None => Line::from(label.to_string()),
        Some(run) => Line::from(vec![
            Span::raw(label.to_string()),
            Span::raw("  "),
            Span::styled(live_badge(run, now), Style::default().fg(Color::LightGreen)),
        ]),
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
            Screen::Setup | Screen::Monitor => {}
        }
    }

    /// Re-reads the registered runs and which build of the picker runs.
    fn refresh_picker_marks(&mut self) {
        self.runs = list_runs();
        self.ensure_picker_selection();
        let options = self.options.clone();
        let mut marks = std::collections::BTreeMap::new();
        for entry in &self.build_entries {
            let build_dir = self
                .build_dirs
                .entry(entry.path.clone())
                .or_insert_with(|| {
                    try_resolve_config_with_options(&entry.path, &options)
                        .ok()
                        .map(|spec| PathBuf::from(spec.workspace.build_dir))
                })
                .clone();
            if let Some(run) = build_dir.and_then(|dir| live_run(&dir)) {
                marks.insert(entry.path.clone(), run);
            }
        }
        self.live_marks = marks;
        self.picker_polled_at = Some(Instant::now());
    }

    fn attached_pid(&self) -> Option<u32> {
        self.attach
            .as_ref()
            .and_then(|attach| attach.run.as_ref())
            .map(|run| run.pid)
    }

    pub(crate) fn handle_attach_key(&mut self, code: KeyCode) {
        let armed = self
            .attach
            .as_ref()
            .is_some_and(|attach| attach.cancel_armed);
        if let Some(attach) = self.attach.as_mut() {
            attach.cancel_armed = false;
        }
        if armed {
            if matches!(code, KeyCode::Char('c' | 'y')) {
                self.signal_attached(AppCommand::Cancel, "cancelling the build");
            } else {
                self.set_status("cancel not confirmed; the build keeps running");
            }
            return;
        }
        match code {
            KeyCode::Char('p') => self.signal_attached(AppCommand::Pause, "pausing the build"),
            KeyCode::Char('r') => self.signal_attached(AppCommand::Resume, "resuming the build"),
            KeyCode::Char('c') => {
                if self.attached_pid().is_none() {
                    self.set_status("no live run to cancel");
                } else {
                    if let Some(attach) = self.attach.as_mut() {
                        attach.cancel_armed = true;
                    }
                    self.set_status(
                        "press c or y again to cancel the build; any other key keeps it running",
                    );
                }
            }
            KeyCode::Char('b') => self.open_picker(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_attached(true, 1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_attached(false, 1),
            KeyCode::PageUp => self.scroll_attached(true, 10),
            KeyCode::PageDown => self.scroll_attached(false, 10),
            KeyCode::Home => {
                if let Some(attach) = self.attach.as_mut() {
                    attach.follow_tail = false;
                    attach.log_scroll = 0;
                }
            }
            KeyCode::End => {
                if let Some(attach) = self.attach.as_mut() {
                    attach.follow_tail = true;
                }
            }
            _ => {}
        }
    }

    fn scroll_attached(&mut self, up: bool, step: u16) {
        if let Some(attach) = self.attach.as_mut() {
            if up {
                attach.scroll_up(step);
            } else {
                attach.scroll_down(step);
            }
        }
    }

    fn signal_attached(&mut self, command: AppCommand, action: &str) {
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
    let Some(attach) = state.attach.as_mut() else {
        return;
    };
    let summary_height = (attach.summary.len() as u16 + 2).clamp(3, (area.height / 2).max(3));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(summary_height), Constraint::Min(4)])
        .split(area);

    let title = match (&attach.run, &attach.ended) {
        (Some(run), _) if run.paused() => "Attached: PAUSED",
        (Some(_), _) => "Attached: live run",
        (None, Some(_)) => "Attached: run ended",
        (None, None) => "Attached: no run",
    };
    let summary = attach
        .summary
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(summary))
            .wrap(Wrap { trim: false })
            .block(Block::default().title(title).borders(Borders::ALL)),
        rows[0],
    );

    let block = Block::default()
        .title(format!(
            "Recent output ({} lines)  PgUp/PgDn scroll  End follows",
            attach.logs.len()
        ))
        .borders(Borders::ALL);
    let inner = block.inner(rows[1]);
    frame.render_widget(block, rows[1]);
    let visible = inner.height as usize;
    let max_scroll = attach.logs.len().saturating_sub(visible);
    let max_scroll = max_scroll.min(u16::MAX as usize) as u16;
    if attach.follow_tail || attach.log_scroll >= max_scroll {
        attach.follow_tail = true;
        attach.log_scroll = max_scroll;
    }
    let logs = attach
        .logs
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(logs)).scroll((attach.log_scroll, 0)),
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::live_status::fixture_status;
    use crate::commands::run_registry::fixture_registered_run;
    use ratatui::backend::TestBackend;

    fn live_fixture(stopped: bool) -> LiveRun {
        LiveRun {
            pid: 4242,
            status: Some(fixture_status()),
            stopped,
        }
    }

    fn attach_fixture(run: LiveRun) -> AttachState {
        let status = run.status.clone().expect("fixture status");
        AttachState {
            files: RunFiles::in_build_dir(&PathBuf::new()),
            summary: live_lines(&run, 1_100),
            logs: log_lines(&status),
            run: Some(run),
            ended: None,
            log_scroll: 0,
            follow_tail: true,
            cancel_armed: false,
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

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn picker_marks_running_builds_with_elapsed_time_and_progress() {
        let running = line_text(&picker_line(
            "cm5.toml",
            Some(&live_fixture(false)),
            1_000 + 12 * 60,
        ));
        assert_eq!(running, "cm5.toml  ● running 12m, 82% packages");
        assert_eq!(line_text(&picker_line("pi.toml", None, 0)), "pi.toml");
    }

    #[test]
    fn attached_monitor_renders_status_and_recent_output() {
        let context = AppContext::with_defaults();
        let mut state = TuiState::new(
            &context,
            TuiLaunch {
                build: "missing-build.toml",
                build_explicit: true,
                builds_dir: None,
            },
            &ResolveOptions::default(),
        );
        state.screen = Screen::Attach;
        state.attach = Some(attach_fixture(live_fixture(true)));

        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(110, 26)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &mut state))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Attached: PAUSED"), "{text}");
        assert!(text.contains("ops: 3/10"), "{text}");
        assert!(text.contains("packages: ["), "{text}");
        assert!(text.contains("image:buildroot"), "{text}");
        assert!(
            text.contains("artifact:cli  cargo build finished"),
            "{text}"
        );
        assert!(text.contains("[p] pause"), "{text}");
    }

    #[test]
    fn cancel_needs_a_second_confirming_key() {
        let context = AppContext::with_defaults();
        let mut state = TuiState::new(
            &context,
            TuiLaunch {
                build: "missing-build.toml",
                build_explicit: true,
                builds_dir: None,
            },
            &ResolveOptions::default(),
        );
        state.screen = Screen::Attach;
        state.attach = Some(attach_fixture(live_fixture(false)));

        state.handle_attach_key(KeyCode::Char('c'));
        assert!(state.attach.as_ref().is_some_and(|a| a.cancel_armed));
        // Any other key disarms it without signalling the run.
        state.handle_attach_key(KeyCode::Char('x'));
        assert!(state.attach.as_ref().is_some_and(|a| !a.cancel_armed));
        assert_eq!(
            state.footer_notice(),
            Some("cancel not confirmed; the build keeps running")
        );
    }

    fn listed_live(name: &str, pid: u32) -> ListedRun {
        let mut status = fixture_status();
        status.pid = pid;
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: Some(LiveRun {
                pid,
                status: Some(status),
                stopped: false,
            }),
            ended: None,
        }
    }

    fn listed_ended(name: &str, pid: u32) -> ListedRun {
        let mut status = fixture_status();
        status.pid = pid;
        status.running.clear();
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: None,
            ended: Some(LastRun {
                outcome: "completed".into(),
                ended_at: 1_500,
                status,
            }),
        }
    }

    /// A picker over an empty directory: no build configs of its own.
    fn picker_without_project(runs: Vec<ListedRun>) -> TuiState<'static> {
        let context: &'static AppContext = Box::leak(Box::new(AppContext::with_defaults()));
        let empty = std::env::temp_dir().join(format!("gaia-tui-empty-{}", std::process::id()));
        fs::create_dir_all(&empty).expect("empty dir");
        let mut state = TuiState::new(
            context,
            TuiLaunch {
                build: "missing-build.toml",
                build_explicit: false,
                builds_dir: empty.to_str(),
            },
            &ResolveOptions::default(),
        );
        state.screen = Screen::Picker;
        state.runs = runs;
        state.ensure_picker_selection();
        state
    }

    #[test]
    fn start_screen_shows_running_builds_without_a_project() {
        let mut state = picker_without_project(vec![
            listed_live("Cm5 image", 4242),
            listed_ended("Pi zero", 99),
        ]);
        assert_eq!(state.picker_focus, PickerFocus::Runs);

        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(110, 20)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &mut state))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Running builds (1 live)"), "{text}");
        assert!(text.contains(">> Cm5 image  pid 4242"), "{text}");
        assert!(
            text.contains("Pi zero  pid 99  ended (completed)"),
            "{text}"
        );
        assert!(
            text.contains("No build configs in this directory."),
            "{text}"
        );
        assert!(text.contains("[Enter] open/attach"), "{text}");
    }

    #[test]
    fn start_screen_without_runs_or_projects_still_opens() {
        let mut state = picker_without_project(Vec::new());
        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(110, 16)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &mut state))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Build Picker"), "{text}");
        assert!(
            text.contains("No build configs in this directory."),
            "{text}"
        );
    }

    #[test]
    fn cursor_moves_through_running_builds_then_build_configs() {
        let mut state =
            picker_without_project(vec![listed_live("One", 4242), listed_live("Two", 4343)]);
        // A build config of this directory, below the running builds.
        state.build_entries = vec![BuildEntry {
            label: "cm5.toml".into(),
            path: "cm5.toml".into(),
        }];
        state.ensure_build_selection();

        state.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(state.run_list.selected(), Some(1));
        state.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(state.picker_focus, PickerFocus::Builds);
        assert_eq!(state.build_list.selected(), Some(0));
        state.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(state.picker_focus, PickerFocus::Runs);
        assert_eq!(state.run_list.selected(), Some(1));
    }

    #[test]
    fn enter_on_a_running_build_attaches_through_its_registry_entry() {
        let mut state = picker_without_project(vec![listed_live("Cm5 image", 4242)]);
        state.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(state.screen, Screen::Attach);
        let attach = state.attach.as_ref().expect("attached");
        assert_eq!(
            attach.files,
            RunFiles::in_build_dir(&PathBuf::from("/builds/demo/build"))
        );
    }
}
