use super::*;
use crate::commands::live_status::{LastRun, LiveRun, RunTracker, SnapshotCache};
use crate::commands::run_registry::{ListedRun, list_runs};
use crate::tui::dashboard::{DashboardState, RunEnd};
use crate::tui::details::DetailsState;
use crate::tui::metrics::Metrics;
use crate::tui::redraw::Redraw;
use ratatui::widgets::ListState;
const SETUP_REFRESH_DEBOUNCE: Duration = Duration::from_millis(200);

pub(crate) struct TuiState<'a> {
    pub(crate) context: &'a AppContext,
    pub(crate) build: String,
    pub(crate) builds_dir: Option<PathBuf>,
    pub(crate) options: ResolveOptions,
    pub(crate) screen: Screen,
    pub(crate) build_entries: Vec<BuildEntry>,
    pub(crate) build_list: ListState,
    pub(crate) setup_items: Vec<SetupItem>,
    pub(crate) setup_list: ListState,
    pub(crate) spec: Option<ResolvedBuildSpec>,
    pub(crate) validation: Option<ValidationReport>,
    pub(crate) plan: Option<ExecutionPlan>,
    pub(crate) plan_diagnostics: Vec<gaia_plan::PlanDiagnostic>,
    /// Last recorded wall-clock duration (ms) per operation id.
    pub(crate) operation_durations: std::collections::BTreeMap<String, u64>,
    pub(crate) last_run: Option<RunArtifacts>,
    pub(crate) last_run_duration: Option<Duration>,
    pub(crate) events: EventLog,
    /// The state of the run this TUI started, for the monitor.
    pub(crate) tracker: Option<RunTracker>,
    /// Set once the run this TUI started is over.
    pub(crate) run_end: Option<RunEnd>,
    /// Cancellation of the run was asked for.
    pub(crate) cancelling: bool,
    /// The run dashboard's own state: selection, filters, charts, output.
    pub(crate) dash: DashboardState,
    /// Machine metrics for the dashboard, sampled while the TUI runs.
    pub(crate) metrics: Metrics,
    pub(crate) run_state: RunState,
    pub(crate) status: String,
    pub(crate) status_since: Instant,
    pub(crate) edit_field: Option<SetupEditField>,
    pub(crate) edit_buffer: String,
    pub(crate) pending_exit_code: Option<(i32, Instant)>,
    pub(crate) quit: QuitState,
    pub(crate) show_help: bool,
    pub(crate) pending_refresh_at: Option<Instant>,
    pub(crate) refresh_receiver: Option<Receiver<RefreshThreadMessage>>,
    pub(crate) refresh_revision: u64,
    /// The monitor of a run another process started, when attached.
    pub(crate) attach: Option<AttachState>,
    /// Builds of the picker with a live run, by build path.
    pub(crate) live_marks: std::collections::BTreeMap<String, LiveRun>,
    /// What each build config says about itself, resolved once per scan.
    pub(crate) build_meta: std::collections::BTreeMap<String, Option<BuildMeta>>,
    /// The last recorded run of each build config, by build path.
    pub(crate) build_last: std::collections::BTreeMap<String, LastRun>,
    pub(crate) picker_polled_at: Option<Instant>,
    /// Every gaia run registered on this system, live and recently ended.
    pub(crate) runs: Vec<ListedRun>,
    pub(crate) run_list: ListState,
    pub(crate) picker_focus: PickerFocus,
    /// The details screen: its tab and the state of each tab.
    pub(crate) details: DetailsState,
    /// When a frame is due; see [`Redraw`].
    pub(crate) redraw: Redraw,
    /// The picker's reads of the build dirs' status files, re-read only when
    /// a file changed.
    pub(crate) snapshots: SnapshotCache,
}

impl<'a> TuiState<'a> {
    pub(crate) fn new(
        context: &'a AppContext,
        launch: TuiLaunch<'_>,
        options: &ResolveOptions,
    ) -> Self {
        let builds_dir = launch.builds_dir.map(PathBuf::from);
        let build = launch.build;
        let mut build_list = ListState::default();
        let build_entries = discover_build_entries(build, builds_dir.as_deref());
        let discovered_build = build_entries
            .iter()
            .find(|entry| entry.path == build)
            .map(|entry| entry.path.clone());
        // Without an explicit build, let the user choose unless there is only
        // one candidate.
        let selected_build = if launch.build_explicit {
            build.to_string()
        } else {
            discovered_build
                .or_else(|| build_entries.first().map(|entry| entry.path.clone()))
                .unwrap_or_else(|| build.to_string())
        };
        // The start screen is the picker unless the build was named, or this
        // directory has exactly one build and no other build is running.
        let runs = list_runs();
        let other_live_run = runs
            .iter()
            .any(|run| run.is_live() && !run.run.is_config(&selected_build));
        let should_open_picker =
            !launch.build_explicit && (build_entries.len() != 1 || other_live_run);
        if !build_entries.is_empty() {
            let selected = build_entries
                .iter()
                .position(|entry| entry.path == selected_build)
                .unwrap_or(0);
            build_list.select(Some(selected));
        }
        let mut run_list = ListState::default();
        if !runs.is_empty() {
            run_list.select(Some(0));
        }
        let picker_focus = if runs.iter().any(ListedRun::is_live)
            || build_entries.is_empty() && !runs.is_empty()
        {
            PickerFocus::Runs
        } else {
            PickerFocus::Builds
        };

        let mut setup_list = ListState::default();
        setup_list.select(Some(0));

        Self {
            context,
            build: selected_build,
            builds_dir,
            options: options.clone(),
            screen: if should_open_picker {
                Screen::Picker
            } else {
                Screen::Setup
            },
            build_entries,
            build_list,
            setup_items: SetupItem::defaults(),
            setup_list,
            spec: None,
            validation: None,
            plan: None,
            plan_diagnostics: Vec::new(),
            operation_durations: Default::default(),
            last_run: None,
            last_run_duration: None,
            events: EventLog::default(),
            tracker: None,
            run_end: None,
            cancelling: false,
            dash: DashboardState::default(),
            metrics: Metrics::system(),
            run_state: RunState::Idle,
            status: "loading build state".into(),
            status_since: Instant::now(),
            edit_field: None,
            edit_buffer: String::new(),
            pending_exit_code: None,
            quit: QuitState::Idle,
            show_help: false,
            pending_refresh_at: None,
            refresh_receiver: None,
            refresh_revision: 0,
            attach: None,
            live_marks: Default::default(),
            build_meta: Default::default(),
            build_last: Default::default(),
            picker_polled_at: None,
            runs,
            run_list,
            picker_focus,
            details: DetailsState::default(),
            redraw: Redraw::default(),
            snapshots: SnapshotCache::default(),
        }
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_since = Instant::now();
        self.redraw.mark();
    }

    pub(crate) fn footer_notice(&self) -> Option<&str> {
        if self.status.is_empty() || self.status_since.elapsed() > Duration::from_secs(3) {
            None
        } else {
            Some(self.status.as_str())
        }
    }

    pub(crate) fn refresh(&mut self) {
        self.pending_refresh_at = None;
        self.refresh_receiver = None;
        self.refresh_revision = self.refresh_revision.wrapping_add(1);
        match resolve_spec_for_refresh(&self.build, self.options.clone()) {
            Ok((options, spec)) => {
                let validation = validate_spec_with_providers(
                    &spec,
                    &self.context.source_catalog,
                    &self.context.artifact_catalog,
                    &self.context.image_catalog,
                );
                let reuse_state = load_reuse_state(&spec);
                let plan = plan_build_with_reuse_state(
                    &spec,
                    &self.context.source_catalog,
                    &self.context.artifact_catalog,
                    &self.context.image_catalog,
                    reuse_state.as_ref(),
                );
                let plan_diagnostics = plan.validate();
                let operation_durations = load_operation_durations(&spec);
                self.apply_refresh_artifacts(RefreshArtifacts {
                    options,
                    spec,
                    validation,
                    plan,
                    plan_diagnostics,
                    operation_durations,
                });
            }
            Err(error) => {
                self.clear_refresh_state(error);
            }
        }
    }

    pub(crate) fn request_refresh(&mut self, status: impl Into<String>) {
        self.refresh_revision = self.refresh_revision.wrapping_add(1);
        self.pending_refresh_at = Some(Instant::now() + SETUP_REFRESH_DEBOUNCE);
        self.set_status(status);
    }

    /// Whether something on screen animates: a run in this TUI, an attached
    /// live run, or a status notice that is about to expire.
    pub(crate) fn is_animating(&self) -> bool {
        matches!(self.run_state, RunState::Running { .. })
            || (self.screen == Screen::Attach
                && self
                    .attach
                    .as_ref()
                    .is_some_and(|attach| attach.run.is_some()))
            || self.footer_notice().is_some()
    }

    /// Whether a frame is due now. See [`Redraw`].
    pub(crate) fn redraw_due(&self, now: Instant) -> bool {
        self.redraw.due(now, self.is_animating())
    }

    /// Records that a frame was drawn at `now`.
    pub(crate) fn redraw_drew(&mut self, now: Instant) {
        let animating = self.is_animating();
        self.redraw.drew(now, animating);
    }

    /// Asks for a frame: an input event, a resize, or a change of state.
    pub(crate) fn request_redraw(&mut self) {
        self.redraw.mark();
    }

    /// How long the event loop may wait for input: the next animation frame,
    /// at most the 100 ms poll that keeps the background work moving.
    pub(crate) fn poll_timeout(&self, now: Instant) -> Duration {
        let base = Duration::from_millis(100);
        match self.redraw.wait(now, self.is_animating()) {
            Some(wait) => wait.min(base),
            None => base,
        }
    }

    pub(crate) fn tick(&mut self) {
        if let RunState::Running { spinner_tick, .. } = &mut self.run_state {
            *spinner_tick = spinner_tick.wrapping_add(1);
        }
        if self.metrics.sample_if_due(Instant::now())
            && matches!(self.screen, Screen::Monitor | Screen::Attach)
        {
            self.redraw.mark();
        }
        self.poll_refresh_completion();
        self.start_pending_refresh();
        self.poll_live();
    }

    /// Handles q / Ctrl+C. Returns an exit code when the TUI should close now.
    ///
    /// A running build is never abandoned silently: the first press asks for
    /// confirmation, the second cancels the build and exits once it has
    /// stopped, and a third forces the exit.
    pub(crate) fn request_quit(&mut self) -> Option<i32> {
        if !matches!(self.run_state, RunState::Running { .. }) {
            return Some(self.exit_code());
        }
        match self.quit {
            QuitState::Idle => {
                self.quit = QuitState::Armed;
                self.set_status("a build is running: press q again to cancel it and quit");
                None
            }
            QuitState::Armed => {
                self.quit = QuitState::CancelThenExit;
                self.cancel_run();
                self.set_status(
                    "cancelling build; will exit once it stops (press q again to force)",
                );
                None
            }
            QuitState::CancelThenExit => Some(130),
        }
    }

    pub(crate) fn should_exit(&self) -> Option<i32> {
        self.pending_exit_code
            .as_ref()
            .and_then(|(code, deadline)| (Instant::now() >= *deadline).then_some(*code))
    }

    fn start_pending_refresh(&mut self) {
        if matches!(self.run_state, RunState::Running { .. }) || self.refresh_receiver.is_some() {
            return;
        }
        let Some(deadline) = self.pending_refresh_at else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }

        self.pending_refresh_at = None;
        let revision = self.refresh_revision;
        let build = self.build.clone();
        let options = self.options.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = resolve_refresh_artifacts(&build, options);
            let _ = tx.send(RefreshThreadMessage { revision, result });
        });
        self.refresh_receiver = Some(rx);
        self.set_status("refreshing resolve/validate/plan state");
    }

    fn poll_refresh_completion(&mut self) {
        let Some(receiver) = self.refresh_receiver.as_ref() else {
            return;
        };
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.refresh_receiver = None;
                self.clear_refresh_state("refresh worker disconnected");
                return;
            }
        };
        self.refresh_receiver = None;
        if message.revision != self.refresh_revision {
            return;
        }
        match message.result {
            Ok(artifacts) => self.apply_refresh_artifacts(artifacts),
            Err(error) => self.clear_refresh_state(error),
        }
    }

    fn apply_refresh_artifacts(&mut self, artifacts: RefreshArtifacts) {
        self.rebuild_setup_items(&artifacts.spec);
        self.options = artifacts.options;
        self.spec = Some(artifacts.spec);
        self.validation = Some(artifacts.validation);
        self.plan = Some(artifacts.plan);
        self.plan_diagnostics = artifacts.plan_diagnostics;
        self.operation_durations = artifacts.operation_durations;
        self.set_status("refreshed resolve/validate/plan state");
    }

    fn rebuild_setup_items(&mut self, spec: &ResolvedBuildSpec) {
        let selected = self.selected_setup_item();
        let mut items = vec![SetupItem::StartBuild, SetupItem::Branch];
        for input in &spec.inputs.declared {
            match input.name.as_str() {
                "target" => items.push(SetupItem::Target),
                "profile" => items.push(SetupItem::Profile),
                _ => items.push(SetupItem::Input(input.name.clone())),
            }
        }
        if !spec.selection.variants.is_empty() {
            items.push(SetupItem::Variant);
        }
        items.extend([
            SetupItem::Jobs,
            SetupItem::Plan,
            SetupItem::Validation,
            SetupItem::Reports,
            SetupItem::Spec,
            SetupItem::PickBuild,
            SetupItem::Refresh,
        ]);
        items.dedup();
        let fallback_index = self.setup_list.selected().unwrap_or(0).min(items.len() - 1);
        let selected_index = items
            .iter()
            .position(|candidate| *candidate == selected)
            .unwrap_or(fallback_index);
        self.setup_items = items;
        self.setup_list.select(Some(selected_index));
    }

    fn clear_refresh_state(&mut self, error: impl Into<String>) {
        self.set_status(error.into());
        self.spec = None;
        self.validation = None;
        self.plan = None;
        self.plan_diagnostics.clear();
    }
}

fn resolve_refresh_artifacts(
    build: &str,
    options: ResolveOptions,
) -> Result<RefreshArtifacts, String> {
    let (options, spec) = resolve_spec_for_refresh(build, options)?;

    let context = AppContext::with_defaults();
    let validation = validate_spec_with_providers(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    let reuse_state = load_reuse_state(&spec);
    let plan = plan_build_with_reuse_state(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        reuse_state.as_ref(),
    );
    let plan_diagnostics = plan.validate();
    let operation_durations = load_operation_durations(&spec);

    Ok(RefreshArtifacts {
        options,
        spec,
        validation,
        plan,
        plan_diagnostics,
        operation_durations,
    })
}

/// What a build config says about itself, for the picker: where its build
/// dir is, and its target and profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildMeta {
    pub(crate) build_dir: PathBuf,
    pub(crate) target: Option<String>,
    pub(crate) profile: Option<String>,
}

/// Resolves the build the setup shows and runs: the build config with the
/// current overrides, the git branch when the build has none, and for a build
/// with `[[variants]]` the selected variant (the first one when none is).
pub(crate) fn resolve_spec_for_refresh(
    build: &str,
    mut options: ResolveOptions,
) -> Result<(ResolveOptions, ResolvedBuildSpec), String> {
    let mut spec =
        try_resolve_config_with_options(build, &options).map_err(|error| error.to_string())?;
    let has_branch_override = options
        .explicit_overrides
        .iter()
        .any(|(key, _)| key == "build.branch");
    if spec.metadata.branch.is_none()
        && !has_branch_override
        && let Some(git_branch) = current_git_branch()
    {
        options
            .explicit_overrides
            .push(("build.branch".to_string(), git_branch));
        spec =
            try_resolve_config_with_options(build, &options).map_err(|error| error.to_string())?;
    }
    if spec.selection.variants.is_empty() {
        return Ok((options, spec));
    }
    let declared = spec
        .selection
        .variants
        .iter()
        .map(|variant| variant.id.clone())
        .collect::<Vec<_>>();
    let chosen = options
        .selected_variants
        .first()
        .filter(|id| declared.contains(id))
        .cloned()
        .unwrap_or_else(|| declared[0].clone());
    options.selected_variants = vec![chosen.clone()];
    let specs =
        gaia_config::resolve_variant_specs(build, &options).map_err(|error| error.to_string())?;
    let spec = specs
        .into_iter()
        .find(|spec| spec.selection.variant.as_deref() == Some(chosen.as_str()))
        .ok_or_else(|| format!("variant '{chosen}' did not resolve"))?;
    Ok((options, spec))
}

pub(crate) fn current_git_branch() -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("branch")
        .arg("--show-current")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}
