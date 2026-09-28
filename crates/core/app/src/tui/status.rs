use super::*;

impl<'a> TuiState<'a> {
    pub(crate) fn run_status_label(&self) -> String {
        match &self.run_state {
            RunState::Idle => {
                if let Some(run) = self.last_run.as_ref() {
                    if run.outcome.cancelled {
                        "cancelled".into()
                    } else if run.outcome.errors.is_empty() {
                        format!("completed: {} reports", run.report_outputs.files.len())
                    } else {
                        format!("failed: {} error(s)", run.outcome.errors.len())
                    }
                } else {
                    "ready".into()
                }
            }
            RunState::Running { .. } if self.quit == QuitState::CancelThenExit => {
                "cancelling".into()
            }
            RunState::Running { .. } => {
                let current = self.events.newest_running().unwrap_or("starting");
                format!("running: {current}")
            }
        }
    }

    pub(crate) fn run_progress_percent(&self) -> u16 {
        let total = self.operation_total();
        if total == 0 {
            return 0;
        }
        let completed = if let Some(run) = self.last_run.as_ref() {
            run.report.summary.completed_operations + run.report.summary.reused_operations
        } else {
            self.events.completed_count()
        }
        .min(total);
        ((completed as f64 / total as f64) * 100.0) as u16
    }

    pub(crate) fn run_elapsed_label(&self) -> String {
        match &self.run_state {
            RunState::Running { started_at, .. } => format_elapsed(started_at.elapsed()),
            RunState::Idle => self
                .last_run_duration
                .map(format_elapsed)
                .unwrap_or_else(|| "00:00:00".into()),
        }
    }

    pub(crate) fn monitor_summary_line(&self) -> String {
        match &self.run_state {
            RunState::Running { .. } => {
                let running = self.events.running();
                let running_label = match running {
                    [] => "starting".to_string(),
                    [only] => only.clone(),
                    [.., newest] => format!("{newest} (+{} more)", running.len() - 1),
                };
                format!(
                    "running: {}  completed={}/{}  follow={}",
                    running_label,
                    self.events.completed_count(),
                    self.operation_total(),
                    if self.follow_running { "on" } else { "off [f]" },
                )
            }
            RunState::Idle => {
                if let Some(run) = self.last_run.as_ref() {
                    format!(
                        "summary: completed={} reused={} rolled_back={} errors={} reports={}  dir={}",
                        run.report.summary.completed_operations,
                        run.report.summary.reused_operations,
                        run.report.summary.rolled_back_operations,
                        run.report.summary.error_count,
                        run.report_outputs.files.len(),
                        PathBuf::from(&run.spec.workspace.out_dir)
                            .join(".gaia")
                            .join("reports")
                            .display()
                    )
                } else {
                    "no run executed yet".into()
                }
            }
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.last_run
            .as_ref()
            .map(|run| {
                if run.report.summary.error_count > 0
                    || !run.validation.errors.is_empty()
                    || !run.plan_diagnostics.is_empty()
                {
                    4
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }

    pub(crate) fn tui_exit_summary(&self) -> String {
        let Some(run) = self.last_run.as_ref() else {
            return String::new();
        };

        let status = if run.outcome.cancelled {
            "cancelled".to_string()
        } else if run.outcome.errors.is_empty() {
            "completed".to_string()
        } else {
            format!("failed ({})", run.outcome.errors.len())
        };
        let mut lines = vec![format!(
            "tui build result: {}  build='{}'",
            status, run.report.summary.build_name
        )];
        lines.push(format!(
            "tui build stats: completed={} reused={} rolled_back={} errors={}",
            run.report.summary.completed_operations,
            run.report.summary.reused_operations,
            run.report.summary.rolled_back_operations,
            run.report.summary.error_count,
        ));
        if let Some(duration) = self.last_run_duration {
            lines.push(format!("tui build time: {}", format_elapsed(duration)));
        }
        lines.push(format!("tui output dir: {}", run.spec.workspace.out_dir));
        lines.push(format!(
            "tui reports dir: {}",
            PathBuf::from(&run.spec.workspace.out_dir)
                .join(".gaia")
                .join("reports")
                .display()
        ));
        lines.push(format!(
            "tui reports written: {}",
            run.report_outputs.files.len()
        ));

        lines.join("\n")
    }
}
