use super::*;

const DEFAULT_BUILD_CONFIG: &str = "examples/default-workspace/configs/default.toml";

pub(crate) struct TuiState<'a> {
    pub(crate) context: &'a AppContext,
    pub(crate) build: String,
    pub(crate) options: ResolveOptions,
    pub(crate) screen: Screen,
    pub(crate) build_entries: Vec<BuildEntry>,
    pub(crate) build_list: ListState,
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
            && build == DEFAULT_BUILD_CONFIG
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
        let mut spec = match try_resolve_config_with_options(&self.build, &self.options) {
            Ok(spec) => spec,
            Err(error) => {
                self.set_status(error.to_string());
                self.spec = None;
                self.validation = None;
                self.plan = None;
                self.plan_diagnostics.clear();
                return;
            }
        };
        let has_branch_override = self
            .options
            .explicit_overrides
            .iter()
            .any(|(key, _)| key == "build.branch");
        if spec.metadata.branch.is_none()
            && !has_branch_override
            && let Some(git_branch) = self.current_git_branch()
        {
            self.set_or_clear_override("build.branch", &git_branch);
            spec = match try_resolve_config_with_options(&self.build, &self.options) {
                Ok(spec) => spec,
                Err(error) => {
                    self.set_status(error.to_string());
                    self.spec = None;
                    self.validation = None;
                    self.plan = None;
                    self.plan_diagnostics.clear();
                    return;
                }
            };
        }
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

        self.spec = Some(spec);
        self.validation = Some(validation);
        self.plan = Some(plan);
        self.plan_diagnostics = plan_diagnostics;
        self.detail_scroll = 0;
        self.ensure_operation_selection();
        self.set_status("refreshed resolve/validate/plan state");
    }

    pub(crate) fn tick(&mut self) {
        if let RunState::Running { spinner_tick, .. } = &mut self.run_state {
            *spinner_tick = spinner_tick.wrapping_add(1);
        }
    }

    pub(crate) fn should_exit(&self) -> Option<i32> {
        self.pending_exit_code
            .as_ref()
            .and_then(|(code, deadline)| (Instant::now() >= *deadline).then_some(*code))
    }
}
