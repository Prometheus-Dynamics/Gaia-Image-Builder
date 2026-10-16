use super::*;

impl<'a> TuiState<'a> {
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
