use super::*;

impl<'a> TuiState<'a> {
    pub(crate) fn selected_detail_view(&self) -> DetailView {
        match self.screen {
            Screen::Picker => DetailView::Overview,
            Screen::Setup => self.selected_setup_item().detail_view(),
            Screen::Monitor => self.selected_monitor_view().detail_view(),
        }
    }

    pub(crate) fn should_tail_detail(&self) -> bool {
        self.screen == Screen::Monitor
            && self.detail_follow_tail
            && matches!(
                self.selected_monitor_view(),
                MonitorView::Events | MonitorView::Logs
            )
    }

    pub(crate) fn setup_panel_title(&self) -> String {
        self.selected_detail_view().title().to_string()
    }

    pub(crate) fn monitor_panel_title(&self) -> String {
        String::new()
    }

    pub(crate) fn monitor_tabs_title(&self) -> Line<'static> {
        let spans = MonitorView::all()
            .iter()
            .enumerate()
            .flat_map(|(index, view)| {
                let mut spans = Vec::new();
                if index > 0 {
                    spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
                }
                let selected = index == self.monitor_view;
                spans.push(Span::styled(
                    format!(" {} ", view.detail_view().title()),
                    if selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ));
                spans
            })
            .collect::<Vec<_>>();
        Line::from(spans)
    }

    pub(crate) fn ensure_operation_selection(&mut self) {
        let total = self.operation_total();
        match total {
            0 => self.operation_list.select(None),
            _ if self.operation_list.selected().is_none() => self.operation_list.select(Some(0)),
            _ => {
                let current = self.operation_list.selected().unwrap_or(0);
                if current >= total {
                    self.operation_list.select(Some(total - 1));
                }
            }
        }
    }

    pub(crate) fn selected_operation(&self) -> Option<&PlannedOperation> {
        let idx = self.operation_list.selected()?;
        self.plan.as_ref()?.operations.get(idx)
    }

    pub(crate) fn operation_total(&self) -> usize {
        self.plan
            .as_ref()
            .map(|plan| plan.operations.len())
            .unwrap_or(0)
    }

    pub(crate) fn operation_items(&self) -> Vec<OperationItem> {
        let Some(plan) = self.plan.as_ref() else {
            return Vec::new();
        };
        plan.operations
            .iter()
            .map(|operation| {
                let (status, color) = self.operation_status(operation.id.as_str());
                OperationItem {
                    label: format!("{} {:?}", operation.id.as_str(), operation.kind),
                    status,
                    color,
                }
            })
            .collect()
    }

    pub(crate) fn operation_status(&self, operation_id: &str) -> (&'static str, Color) {
        if let Some(run) = self.last_run.as_ref() {
            if run
                .outcome
                .rolled_back_ids
                .iter()
                .any(|id| id.as_str() == operation_id)
            {
                return ("ROLL", Color::Magenta);
            }
            if run
                .outcome
                .errors
                .iter()
                .any(|error| error.operation_id.as_str() == operation_id)
            {
                return ("FAIL", Color::Red);
            }
            if run
                .outcome
                .reused_ids
                .iter()
                .any(|id| id.as_str() == operation_id)
            {
                return ("REUSE", Color::LightBlue);
            }
            if run
                .outcome
                .completed_ids
                .iter()
                .any(|id| id.as_str() == operation_id)
            {
                return ("OK", Color::Green);
            }
            if run
                .outcome
                .cancelled_operation_id
                .as_ref()
                .map(|id| id.as_str())
                == Some(operation_id)
            {
                return ("CANCEL", Color::LightYellow);
            }
        }
        if matches!(self.run_state, RunState::Running { .. }) {
            let status = live_operation_status(&self.live_events, operation_id);
            if let Some(status) = status {
                return status;
            }
            return ("WAIT", Color::DarkGray);
        }
        ("PEND", Color::DarkGray)
    }

    pub(crate) fn detail_lines(&self) -> Vec<Line<'static>> {
        match self.selected_detail_view() {
            DetailView::Overview => self.overview_lines(),
            DetailView::Selection => self.selection_lines(),
            DetailView::Validation => self.validation_lines(),
            DetailView::Plan => self.plan_lines(),
            DetailView::Events => self.event_lines(),
            DetailView::Logs => self.log_lines(),
            DetailView::Reports => self.report_lines(),
            DetailView::Spec => self.spec_lines(),
        }
    }

    pub(crate) fn overview_lines(&self) -> Vec<Line<'static>> {
        let Some(spec) = self.spec.as_ref() else {
            return vec![Line::from("loading...")];
        };
        let mut lines = vec![
            Line::from(format!("build: {}", spec.identity.display_name)).bold(),
            Line::from(format!("build file: {}", self.build)),
            Line::from(format!(
                "version: {}",
                spec.identity.version.as_deref().unwrap_or("-")
            )),
            Line::from(format!(
                "branch={} target={} profile={} jobs={}",
                spec.metadata.branch.as_deref().unwrap_or("-"),
                spec.metadata.target.as_deref().unwrap_or("-"),
                spec.metadata.profile.as_deref().unwrap_or("-"),
                if spec.policy.execution.jobs == 0 {
                    "all".to_string()
                } else {
                    spec.policy.execution.jobs.to_string()
                },
            )),
            Line::from(format!(
                "execution runtime: parallel jobs={}",
                if spec.policy.execution.jobs == 0 {
                    "all".to_string()
                } else {
                    spec.policy.execution.jobs.to_string()
                }
            )),
            Line::from(""),
        ];
        lines.extend(backend_overview_lines(spec).into_iter().map(Line::from));
        if let Some(run) = self.last_run.as_ref() {
            lines.push(Line::from(""));
            lines.push(Line::from("last run:").bold());
            lines.push(Line::from(format!(
                "operations={} completed={} reused={} rolled_back={} errors={}",
                run.report.summary.operation_count,
                run.report.summary.completed_operations,
                run.report.summary.reused_operations,
                run.report.summary.rolled_back_operations,
                run.report.summary.error_count,
            )));
            lines.extend(
                runtime_overview_lines(&run.report)
                    .into_iter()
                    .map(Line::from),
            );
        }
        lines
    }

    pub(crate) fn selection_lines(&self) -> Vec<Line<'static>> {
        let Some(spec) = self.spec.as_ref() else {
            return vec![Line::from("selection not loaded")];
        };
        let mut lines = vec![
            Line::from("selection").bold(),
            Line::from(format!(
                "build-file: {}",
                spec.selection.selected_build_file.as_deref().unwrap_or("-")
            )),
            Line::from(format!(
                "requested-build: {}",
                spec.selection.requested_build.as_deref().unwrap_or("-")
            )),
            Line::from(format!(
                "preset: {}",
                spec.selection.selected_preset.as_deref().unwrap_or("-")
            )),
        ];
        if !spec.selection.selected_inputs.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("inputs:").bold());
            lines.extend(
                spec.selection
                    .selected_inputs
                    .iter()
                    .map(|(key, value)| Line::from(format!("{key}={value}"))),
            );
        }
        if !spec.selection.env_files.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("env files:").bold());
            lines.extend(
                spec.selection
                    .env_files
                    .iter()
                    .map(|value| Line::from(value.clone())),
            );
        }
        if !spec.selection.precedence_order.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("precedence:").bold());
            lines.push(Line::from(spec.selection.precedence_order.join(" -> ")));
        }
        lines
    }

    pub(crate) fn validation_lines(&self) -> Vec<Line<'static>> {
        let Some(validation) = self.validation.as_ref() else {
            return vec![Line::from("validation not loaded")];
        };
        let mut lines = vec![
            Line::from(format!(
                "validation summary: errors={} warnings={} diagnostics={}",
                validation.errors.len(),
                validation.warnings.len(),
                validation.diagnostics.len(),
            ))
            .bold(),
            Line::from(""),
        ];
        if validation.diagnostics.is_empty() {
            lines.push(Line::from("no validation diagnostics"));
            return lines;
        }
        lines.extend(validation.diagnostics.iter().map(|diagnostic| {
            let location = diagnostic
                .location
                .as_deref()
                .map(|value| format!(" [{value}]"))
                .unwrap_or_default();
            Line::from(format!(
                "{}{}: {}",
                diagnostic.code, location, diagnostic.message
            ))
        }));
        lines
    }

    pub(crate) fn plan_lines(&self) -> Vec<Line<'static>> {
        let Some(plan) = self.plan.as_ref() else {
            return vec![Line::from("plan not loaded")];
        };
        let Some(operation) = self.selected_operation() else {
            return vec![Line::from(format!("plan operations: {}", plan.operations.len())).bold()];
        };
        let mut lines = vec![
            Line::from(format!("selected operation: {}", operation.id.as_str())).bold(),
            Line::from(format!("kind: {:?}", operation.kind)),
            Line::from(format!("optionality: {:?}", operation.optionality)),
            Line::from(format!("parallelism: {:?}", operation.parallelism.mode)),
            Line::from(format!(
                "parallel domain: {:?}",
                operation.parallelism.domain
            )),
            Line::from("executor mode: serial runtime"),
            Line::from(format!("dependencies: {}", operation.depends_on.len())),
            Line::from(format!("fingerprint: {}", operation.fingerprint)),
            Line::from(format!("reuse: {:?}", operation.reuse)),
            Line::from(""),
        ];
        if !operation.depends_on.is_empty() {
            lines.push(Line::from("depends on:").bold());
            lines.extend(
                operation
                    .depends_on
                    .iter()
                    .map(|dependency| Line::from(dependency.as_str().to_string())),
            );
            lines.push(Line::from(""));
        }
        if !self.plan_diagnostics.is_empty() {
            lines.push(Line::from("plan diagnostics:").bold());
            lines.extend(self.plan_diagnostics.iter().map(|diagnostic| {
                Line::from(format!("{}: {}", diagnostic.code, diagnostic.message))
            }));
        }
        lines
    }

    pub(crate) fn event_lines(&self) -> Vec<Line<'static>> {
        if self.last_run.is_none() && self.live_events.is_empty() {
            return match &self.run_state {
                RunState::Running { .. } => vec![
                    Line::from("events").bold(),
                    Line::from("run in progress"),
                    Line::from("waiting for first execution event or provider log"),
                ],
                RunState::Idle => vec![
                    Line::from("events").bold(),
                    Line::from("no run executed yet"),
                    Line::from("press 's' to start the current build"),
                ],
            };
        }
        let mut lines = vec![Line::from("live event stream").bold()];
        lines.push(Line::from(""));
        let events = self
            .last_run
            .as_ref()
            .map(|run| &run.outcome.events)
            .unwrap_or(&self.live_events);
        lines.extend(
            events
                .iter()
                .filter(|event| !matches!(event, ExecutionEvent::Log { .. }))
                .map(render_event_line),
        );
        if lines.len() == 2 {
            lines.push(Line::from("no events yet"));
        }
        lines
    }

    pub(crate) fn log_lines(&self) -> Vec<Line<'static>> {
        let selected_operation = self
            .selected_operation()
            .map(|operation| operation.id.as_str().to_string());
        let title = selected_operation
            .as_deref()
            .map(|id| format!("task logs: {id}"))
            .unwrap_or_else(|| "task logs".to_string());
        let mut lines = vec![Line::from(title).bold(), Line::from("")];
        let events = self
            .last_run
            .as_ref()
            .map(|run| &run.outcome.events)
            .unwrap_or(&self.live_events);
        let mut found = false;
        for event in events {
            if let ExecutionEvent::Log {
                operation_id,
                message,
            } = event
            {
                if let Some(selected_operation) = selected_operation.as_deref()
                    && operation_id.as_str() != selected_operation
                {
                    continue;
                }
                found = true;
                lines.push(Line::from(message.clone()));
            }
        }
        if !found {
            match &self.run_state {
                RunState::Running { .. } => {
                    lines.push(Line::from("no task logs yet"));
                    lines.push(Line::from(
                        "provider subprocess output and executor logs appear here when emitted",
                    ));
                }
                RunState::Idle => {
                    lines.push(Line::from("no task logs available yet"));
                    lines.push(Line::from(
                        "start a build with 's' and select an operation to inspect its logs",
                    ));
                }
            }
        }
        lines
    }

    pub(crate) fn report_lines(&self) -> Vec<Line<'static>> {
        let Some(spec) = self.spec.as_ref() else {
            return vec![Line::from("reports not available: spec not loaded")];
        };
        let report_dir = PathBuf::from(&spec.workspace.out_dir)
            .join(".gaia")
            .join("reports");
        let expected_files = [
            (
                ReportFileKind::Summary,
                format!("{}.summary.json", spec.identity.build_name),
            ),
            (
                ReportFileKind::Selection,
                format!("{}.selection.json", spec.identity.build_name),
            ),
            (
                ReportFileKind::Provenance,
                format!("{}.provenance.json", spec.identity.build_name),
            ),
            (
                ReportFileKind::Manifest,
                format!("{}.manifest.json", spec.identity.build_name),
            ),
            (
                ReportFileKind::RebuildReasons,
                format!("{}.rebuild-reasons.json", spec.identity.build_name),
            ),
        ];
        let Some(run) = self.last_run.as_ref() else {
            return match &self.run_state {
                RunState::Running { .. } => vec![
                    Line::from("reports").bold(),
                    Line::from("report bundle is written after the run completes"),
                    Line::from(format!("target dir: {}", report_dir.display())),
                    Line::from(""),
                    Line::from("expected files:").bold(),
                    Line::from(expected_files[0].1.clone()),
                    Line::from(expected_files[1].1.clone()),
                    Line::from(expected_files[2].1.clone()),
                    Line::from(expected_files[3].1.clone()),
                    Line::from(expected_files[4].1.clone()),
                ],
                RunState::Idle => vec![
                    Line::from("reports").bold(),
                    Line::from("no report available until a run completes"),
                    Line::from(format!("target dir: {}", report_dir.display())),
                    Line::from(""),
                    Line::from("expected files:").bold(),
                    Line::from(expected_files[0].1.clone()),
                    Line::from(expected_files[1].1.clone()),
                    Line::from(expected_files[2].1.clone()),
                    Line::from(expected_files[3].1.clone()),
                    Line::from(expected_files[4].1.clone()),
                ],
            };
        };
        let report = &run.report;
        let mut lines = vec![
            Line::from("summary").bold(),
            Line::from(format!("target dir: {}", report_dir.display())),
            Line::from(format!(
                "operations={} completed={} reused={} errors={} rolled_back={}",
                report.summary.operation_count,
                report.summary.completed_operations,
                report.summary.reused_operations,
                report.summary.error_count,
                report.summary.rolled_back_operations,
            )),
            Line::from(format!(
                "checkpoints: total={} built={} reused={}",
                report.summary.checkpoint_count,
                report.summary.checkpoint_built_count,
                report.summary.checkpoint_reused_count,
            )),
            Line::from(""),
            Line::from("report files").bold(),
        ];
        lines.extend(run.report_outputs.files.iter().map(|file| {
            Line::from(format!(
                "{}: {} ({} bytes)",
                file.kind.as_str(),
                file.path.display(),
                file.bytes
            ))
        }));
        if !report.execution_failures.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("execution failures").bold());
            for failure in &report.execution_failures {
                lines.push(Line::from(format!(
                    "{} {} {:?}: {}",
                    failure.operation_id, failure.code, failure.class, failure.message
                )));
                lines.extend(
                    failure
                        .output_tail
                        .iter()
                        .take(5)
                        .map(|line| Line::from(format!("  output: {line}"))),
                );
            }
        }
        lines
    }

    pub(crate) fn spec_lines(&self) -> Vec<Line<'static>> {
        let Some(spec) = self.spec.as_ref() else {
            return vec![Line::from("spec not loaded")];
        };
        vec![
            Line::from("typed spec snapshot").bold(),
            Line::from(format!("identity.id={}", spec.identity.id.as_str())),
            Line::from(format!("identity.build_name={}", spec.identity.build_name)),
            Line::from(format!(
                "identity.display_name={}",
                spec.identity.display_name
            )),
            Line::from(format!("workspace.root_dir={}", spec.workspace.root_dir)),
            Line::from(format!("workspace.build_dir={}", spec.workspace.build_dir)),
            Line::from(format!("workspace.out_dir={}", spec.workspace.out_dir)),
            Line::from(format!("sources={}", spec.sources.len())),
            Line::from(format!("artifacts={}", spec.artifacts.len())),
            Line::from(format!("install.entries={}", spec.install.entries.len())),
            Line::from(format!("stage.files={}", spec.stage.files.len())),
            Line::from(format!("stage.env_sets={}", spec.stage.env_sets.len())),
            Line::from(format!("stage.services={}", spec.stage.services.len())),
            Line::from(format!(
                "checkpoints.points={}",
                spec.checkpoints.points.len()
            )),
            Line::from(format!("image.provider={}", image_provider_label(spec))),
        ]
    }
}
