use super::*;
use crate::commands::live_status::{
    LiveRunInfo, RunTracker, live_run, live_variant, plan_operations, run_outcome_label,
    run_selection_label, unix_now_ms,
};
use crate::tui::dashboard::{DashboardState, RunEnd};

impl<'a> TuiState<'a> {
    pub(crate) fn poll_run_completion(&mut self) {
        let RunState::Running {
            receiver,
            started_at,
            ..
        } = &self.run_state
        else {
            return;
        };
        let run_duration = started_at.elapsed();
        let mut finished = None;
        loop {
            match receiver.try_recv() {
                Ok(RunThreadMessage::Event(event)) => {
                    if let Some(tracker) = self.tracker.as_mut() {
                        tracker.record(&event, unix_now_ms());
                    }
                    self.events.push(event);
                }
                Ok(RunThreadMessage::Finished(result)) => {
                    finished = Some(*result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err("run thread disconnected".into()));
                    break;
                }
            }
        }

        let Some(message) = finished else {
            return;
        };

        match message {
            Ok(run) => {
                self.run_end = Some(RunEnd {
                    outcome: run_outcome_label(&run.outcome).to_string(),
                    ended_ms: unix_now_ms(),
                });
                let cancelled = run.outcome.cancelled;
                let error_count = run.outcome.errors.len();
                // The run thread forwards every event before `Finished`, so
                // the live log is complete. It also holds the streamed build
                // output, which the outcome deliberately does not keep.
                if self.events.is_empty() {
                    self.events = EventLog::from_events(&run.outcome.events);
                }
                for timing in &run.outcome.operation_timings {
                    if timing.status == gaia_exec::OperationTimingStatus::Built {
                        self.operation_durations.insert(
                            timing.operation_id.as_str().to_string(),
                            u64::try_from(timing.duration.as_millis()).unwrap_or(u64::MAX),
                        );
                    }
                }
                self.last_run = Some(run);
                self.last_run_duration = Some(run_duration);
                if cancelled {
                    self.set_status("run cancelled");
                } else if error_count > 0 {
                    self.focus_first_failure();
                    self.set_status(format!(
                        "run failed with {error_count} error(s); showing logs for the failed operation"
                    ));
                } else {
                    let report_count = self
                        .last_run
                        .as_ref()
                        .map(|run| run.report_outputs.files.len())
                        .unwrap_or(0);
                    self.set_status(format!(
                        "run completed successfully; {} report file(s) written",
                        report_count
                    ));
                }
            }
            Err(message) => {
                self.run_end = Some(RunEnd {
                    outcome: "failed".into(),
                    ended_ms: unix_now_ms(),
                });
                self.set_status(format!("run failed: {message}"));
            }
        }
        self.run_state = RunState::Idle;
        self.cancelling = false;
        if self.quit == QuitState::CancelThenExit {
            self.pending_exit_code = Some((self.exit_code().max(130), Instant::now()));
        }
        self.quit = QuitState::Idle;
    }

    /// Selects the first failed operation, so the dashboard opens on it.
    fn focus_first_failure(&mut self) {
        let failed = self.events.first_failed().map(str::to_string).or_else(|| {
            self.last_run.as_ref().and_then(|run| {
                run.outcome
                    .errors
                    .first()
                    .map(|error| error.operation_id.as_str().to_string())
            })
        });
        if let Some(id) = failed {
            self.dash.selected = Some(id);
        }
        self.dash.follow = false;
        self.dash.log_follow = true;
    }

    pub(crate) fn start_run(&mut self) {
        if matches!(self.run_state, RunState::Running { .. }) {
            self.set_status("run already in progress");
            return;
        }
        if self.pending_refresh_at.is_some() || self.refresh_receiver.is_some() {
            self.refresh();
        }
        // Two runs of one build dir would fight over its state.
        let running = self
            .spec
            .as_ref()
            .and_then(|spec| live_run(std::path::Path::new(&spec.workspace.build_dir)));
        if let Some(run) = running {
            self.attach_if_live();
            self.set_status(format!(
                "cannot run: gaia run {} is already running this build",
                run.pid
            ));
            return;
        }

        let (Some(_spec), Some(validation), Some(_plan)) = (
            self.spec.as_ref(),
            self.validation.as_ref(),
            self.plan.as_ref(),
        ) else {
            self.set_status("cannot run: build state is not ready");
            return;
        };

        if !validation.errors.is_empty() {
            self.set_status(format!(
                "cannot run: {} validation error(s) must be fixed first",
                validation.errors.len()
            ));
            self.setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Validation)));
            return;
        }
        if !self.plan_diagnostics.is_empty() {
            self.set_status(format!(
                "cannot run: {} plan diagnostic(s) must be fixed first",
                self.plan_diagnostics.len()
            ));
            self.setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Plan)));
            return;
        }

        let build = self.build.clone();
        let options = self.options.clone();
        let cancellation = ExecutionCancellation::new();
        let cancellation_for_thread = cancellation.clone();
        let (tx, rx) = mpsc::channel();
        self.events = EventLog::default();
        self.last_run = None;
        self.last_run_duration = None;
        self.pending_exit_code = None;
        self.quit = QuitState::Idle;
        self.dash = DashboardState::default();
        self.run_end = None;
        self.cancelling = false;
        self.tracker = self
            .spec
            .as_ref()
            .zip(self.plan.as_ref())
            .map(|(spec, plan)| {
                RunTracker::new(LiveRunInfo {
                    build_name: spec.build_name().to_string(),
                    display_name: spec.display_name().to_string(),
                    ops_total: plan.operations.len(),
                    operations: plan_operations(&plan.operations, &self.operation_durations),
                    variant: live_variant(spec),
                    selection: run_selection_label(spec),
                })
            });
        thread::spawn(move || {
            let context = AppContext::with_defaults();
            let result =
                collect_run_artifacts(&context, &build, &options, &cancellation_for_thread, &tx)
                    .map_err(|error| error.to_string());
            let _ = tx.send(RunThreadMessage::Finished(Box::new(result)));
        });
        self.run_state = RunState::Running {
            receiver: rx,
            cancellation,
            started_at: Instant::now(),
            spinner_tick: 0,
        };
        self.screen = Screen::Monitor;
        self.set_status("run started");
    }

    pub(crate) fn cancel_run(&mut self) {
        match &self.run_state {
            RunState::Running { cancellation, .. } => {
                cancellation.cancel();
                self.cancelling = true;
                self.set_status("cancellation requested");
            }
            RunState::Idle => {
                self.set_status("no active run to cancel");
            }
        }
    }
}
pub(crate) fn collect_run_artifacts(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    cancellation: &ExecutionCancellation,
    event_sender: &mpsc::Sender<RunThreadMessage>,
) -> io::Result<RunArtifacts> {
    let (_, spec) =
        super::state::resolve_spec_for_refresh(build, options.clone()).map_err(io::Error::other)?;
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
    let (event_tx, event_rx) = mpsc::channel::<ExecutionEvent>();
    let forwarder = {
        let sender = event_sender.clone();
        thread::spawn(move || {
            while let Ok(event) = event_rx.recv() {
                let _ = sender.send(RunThreadMessage::Event(event));
            }
        })
    };
    let outcome = execute_plan_with_cancellation_and_observer(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &context.source_catalog,
            artifact_catalog: &context.artifact_catalog,
            image_catalog: &context.image_catalog,
        },
        cancellation,
        Some(event_tx),
    );
    // Every sender is dropped once execution returns; wait until all events
    // are forwarded so they arrive before `Finished`.
    let _ = forwarder.join();
    save_reuse_state(&spec, &plan, &outcome, reuse_state.as_ref());
    let report = generate_report(&spec, &validation, &plan, &outcome);
    let report_outputs = write_report_bundle(&spec, &report)?;

    Ok(RunArtifacts {
        spec,
        validation,
        plan,
        plan_diagnostics,
        outcome,
        report,
        report_outputs,
        post_build_output: None,
        run_duration: Duration::default(),
    })
}
