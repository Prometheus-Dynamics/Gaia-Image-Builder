//! Setup-screen Plan panel: what the next run will rebuild or reuse, why,
//! how long each operation took last time, and the estimated run time.

use super::*;
use gaia_plan::{estimate_plan, format_duration_short};

impl<'a> TuiState<'a> {
    pub(crate) fn plan_overview_lines(&self, plan: &ExecutionPlan) -> Vec<Line<'static>> {
        let execute = plan
            .operations
            .iter()
            .filter(|operation| operation.reuse.should_execute())
            .count();
        let mut lines = vec![
            Line::from(format!(
                "{} operation(s): {} will run, {} will be reused",
                plan.operations.len(),
                execute,
                plan.operations.len() - execute
            ))
            .bold(),
        ];
        lines.extend(estimate_lines(plan, &self.operation_durations));
        lines.push(Line::from(""));
        if !self.plan_diagnostics.is_empty() {
            lines.push(Line::from("plan diagnostics:").bold().fg(Color::Red));
            lines.extend(self.plan_diagnostics.iter().map(|diagnostic| {
                Line::from(format!("{}: {}", diagnostic.code, diagnostic.message)).fg(Color::Red)
            }));
            lines.push(Line::from(""));
        }
        for operation in &plan.operations {
            let (badge, color, reason) = match &operation.reuse {
                gaia_plan::OperationReuse::Execute(reason) => {
                    ("RUN  ", Color::LightCyan, reason.message.clone())
                }
                gaia_plan::OperationReuse::Reuse { source } => {
                    ("REUSE", Color::LightBlue, format!("reused from {source}"))
                }
            };
            let mut spans = vec![
                Span::styled(format!("{badge} "), Style::default().fg(color)),
                Span::raw(operation.id.as_str().to_string()),
            ];
            if let Some(ms) = self.operation_durations.get(operation.id.as_str()) {
                spans.push(Span::styled(
                    format!(
                        "  last {}",
                        format_duration_short(Duration::from_millis(*ms))
                    ),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(spans));
            lines.push(Line::from(Span::styled(
                format!("      {reason}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines
    }
}

/// Estimate summary lines, shared with `gaia plan`'s wording.
fn estimate_lines(
    plan: &ExecutionPlan,
    durations: &std::collections::BTreeMap<String, u64>,
) -> Vec<Line<'static>> {
    let estimate = estimate_plan(plan, durations);
    if estimate.executing_operations() == 0 {
        return Vec::new();
    }
    if estimate.timed_operations == 0 {
        return vec![Line::from("estimate: no recorded timings yet").fg(Color::DarkGray)];
    }
    let mut lines = vec![Line::from(format!(
        "estimate: critical path {} ({} of work across {} timed operation(s))",
        format_duration_short(estimate.critical_path_duration),
        format_duration_short(estimate.total_work),
        estimate.timed_operations,
    ))];
    if !estimate.critical_path.is_empty() {
        lines.push(
            Line::from(format!(
                "critical path: {}",
                estimate
                    .critical_path
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ))
            .fg(Color::DarkGray),
        );
    }
    if !estimate.untimed_operations.is_empty() {
        lines.push(
            Line::from(format!(
                "{} operation(s) have no recorded timing",
                estimate.untimed_operations.len()
            ))
            .fg(Color::DarkGray),
        );
    }
    lines
}
