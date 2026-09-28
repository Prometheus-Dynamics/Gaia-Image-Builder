use super::*;

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
        let mut received_events = false;
        loop {
            match receiver.try_recv() {
                Ok(RunThreadMessage::Event(event)) => {
                    self.events.push(event);
                    received_events = true;
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
            if received_events && self.follow_running {
                self.select_newest_running_operation();
            }
            return;
        };

        match message {
            Ok(run) => {
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
                    self.monitor_view = index_of_monitor_view(MonitorView::Reports);
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
                self.set_status(format!("run failed: {message}"));
            }
        }
        self.run_state = RunState::Idle;
        self.detail_scroll = 0;
        if self.quit == QuitState::CancelThenExit {
            self.pending_exit_code = Some((self.exit_code().max(130), Instant::now()));
        }
        self.quit = QuitState::Idle;
    }

    fn focus_first_failure(&mut self) {
        let failed = self.events.first_failed().map(str::to_string).or_else(|| {
            self.last_run.as_ref().and_then(|run| {
                run.outcome
                    .errors
                    .first()
                    .map(|error| error.operation_id.as_str().to_string())
            })
        });
        if let Some(index) = failed.and_then(|id| self.index_of_operation(&id)) {
            self.operation_list.select(Some(index));
        }
        self.follow_running = false;
        self.monitor_view = index_of_monitor_view(MonitorView::Logs);
        self.detail_follow_tail = true;
    }

    pub(crate) fn select_newest_running_operation(&mut self) {
        let index = self
            .events
            .newest_running()
            .and_then(|id| self.index_of_operation(id));
        if let Some(index) = index
            && self.operation_list.selected() != Some(index)
        {
            self.operation_list.select(Some(index));
            if self.detail_follow_tail {
                self.detail_scroll = 0;
            }
        }
    }

    pub(crate) fn index_of_operation(&self, operation_id: &str) -> Option<usize> {
        self.plan
            .as_ref()?
            .operations
            .iter()
            .position(|operation| operation.id.as_str() == operation_id)
    }

    pub(crate) fn start_run(&mut self) {
        if matches!(self.run_state, RunState::Running { .. }) {
            self.set_status("run already in progress");
            return;
        }
        if self.pending_refresh_at.is_some() || self.refresh_receiver.is_some() {
            self.refresh();
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
        self.follow_running = true;
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
        self.monitor_view = index_of_monitor_view(MonitorView::Logs);
        self.detail_follow_tail = true;
        self.screen = Screen::Monitor;
        self.set_status("run started");
    }

    pub(crate) fn cancel_run(&mut self) {
        match &self.run_state {
            RunState::Running { cancellation, .. } => {
                cancellation.cancel();
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
    let spec = try_resolve_config_with_options(build, options).map_err(io::Error::other)?;
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
