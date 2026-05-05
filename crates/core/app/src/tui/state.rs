use super::*;
use crate::cli::EXAMPLE_DEFAULT_BUILD_CONFIG;
const SETUP_REFRESH_DEBOUNCE: Duration = Duration::from_millis(200);

pub(crate) struct TuiState<'a> {
    pub(crate) context: &'a AppContext,
    pub(crate) build: String,
    pub(crate) options: ResolveOptions,
    pub(crate) screen: Screen,
    pub(crate) build_entries: Vec<BuildEntry>,
    pub(crate) build_list: ListState,
    pub(crate) setup_items: Vec<SetupItem>,
    pub(crate) setup_list: ListState,
    pub(crate) operation_list: ListState,
    pub(crate) monitor_view: usize,
    pub(crate) detail_scroll: u16,
    pub(crate) spec: Option<ResolvedBuildSpec>,
    pub(crate) validation: Option<ValidationReport>,
    pub(crate) plan: Option<ExecutionPlan>,
    pub(crate) plan_diagnostics: Vec<gaia_plan::PlanDiagnostic>,
    pub(crate) last_run: Option<RunArtifacts>,
    pub(crate) last_run_duration: Option<Duration>,
    pub(crate) live_events: Vec<ExecutionEvent>,
    pub(crate) run_state: RunState,
    pub(crate) status: String,
    pub(crate) status_since: Instant,
    pub(crate) edit_field: Option<SetupEditField>,
    pub(crate) edit_buffer: String,
    pub(crate) pending_exit_code: Option<(i32, Instant)>,
    pub(crate) pending_refresh_at: Option<Instant>,
    pub(crate) refresh_receiver: Option<Receiver<RefreshThreadMessage>>,
    pub(crate) refresh_revision: u64,
    pub(crate) detail_follow_tail: bool,
}

impl<'a> TuiState<'a> {
    pub(crate) fn new(context: &'a AppContext, build: &str, options: &ResolveOptions) -> Self {
        let mut build_list = ListState::default();
        let build_entries = discover_build_entries(build);
        let discovered_build = build_entries
            .iter()
            .find(|entry| entry.path == build)
            .map(|entry| entry.path.clone());
        let should_open_picker = discovered_build.is_none()
            && build == EXAMPLE_DEFAULT_BUILD_CONFIG
            && !build_entries.is_empty();
        let selected_build = discovered_build
            .or_else(|| {
                should_open_picker
                    .then(|| build_entries.first().map(|entry| entry.path.clone()))
                    .flatten()
            })
            .unwrap_or_else(|| build.to_string());
        if !build_entries.is_empty() {
            let selected = build_entries
                .iter()
                .position(|entry| entry.path == selected_build)
                .unwrap_or(0);
            build_list.select(Some(selected));
        }

        let mut setup_list = ListState::default();
        setup_list.select(Some(0));

        Self {
            context,
            build: selected_build,
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
            operation_list: ListState::default(),
            monitor_view: 0,
            detail_scroll: 0,
            spec: None,
            validation: None,
            plan: None,
            plan_diagnostics: Vec::new(),
            last_run: None,
            last_run_duration: None,
            live_events: Vec::new(),
            run_state: RunState::Idle,
            status: "loading build state".into(),
            status_since: Instant::now(),
            edit_field: None,
            edit_buffer: String::new(),
            pending_exit_code: None,
            pending_refresh_at: None,
            refresh_receiver: None,
            refresh_revision: 0,
            detail_follow_tail: true,
        }
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_since = Instant::now();
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
                self.apply_refresh_artifacts(RefreshArtifacts {
                    options,
                    spec,
                    validation,
                    plan,
                    plan_diagnostics,
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

    pub(crate) fn tick(&mut self) {
        if let RunState::Running { spinner_tick, .. } = &mut self.run_state {
            *spinner_tick = spinner_tick.wrapping_add(1);
        }
        self.poll_refresh_completion();
        self.start_pending_refresh();
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
        self.detail_scroll = 0;
        self.ensure_operation_selection();
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
        items.extend([
            SetupItem::Jobs,
            SetupItem::Overview,
            SetupItem::Selection,
            SetupItem::Validation,
            SetupItem::Plan,
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

    Ok(RefreshArtifacts {
        options,
        spec,
        validation,
        plan,
        plan_diagnostics,
    })
}

fn resolve_spec_for_refresh(
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
