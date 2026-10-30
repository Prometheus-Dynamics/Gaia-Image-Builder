//! `--json` output: one JSON document on stdout per command, whatever the
//! outcome. Every document is an object with `schema_version`, `command`,
//! `exit_code` and `status`; the rest depends on the command (see
//! docs/cli.md, "JSON output"). Fields are only ever added within a schema
//! version; renaming or removing one bumps it.

use gaia_plan::OperationReuse;
use serde_json::{Value, json};

use crate::commands::{CommandOutcome, VariantOutcome};
use crate::{AppCommand, RunFinish};

/// The version of the `--json` documents.
pub const JSON_SCHEMA_VERSION: u32 = 1;

/// The JSON document for `outcome` of `command`, with the lines `gaia run`
/// adds after it (export, resume hint) and the process exit code.
pub fn outcome_json(
    command: AppCommand,
    outcome: &CommandOutcome,
    finish: &RunFinish,
    exit_code: i32,
) -> Value {
    let mut document = json!({
        "schema_version": JSON_SCHEMA_VERSION,
        "command": command_name(command),
        "exit_code": exit_code,
    });
    merge(&mut document, outcome_body(outcome));
    if command == AppCommand::Run {
        document["finish"] = json!(finish.lines);
        document["export_failed"] = json!(finish.export_failed);
    }
    document
}

fn command_name(command: AppCommand) -> &'static str {
    match command {
        AppCommand::Help => "help",
        AppCommand::Version => "version",
        AppCommand::Resolve => "resolve",
        AppCommand::Tui => "tui",
        AppCommand::Validate => "validate",
        AppCommand::Plan => "plan",
        AppCommand::Clean => "clean",
        AppCommand::Cache => "cache",
        AppCommand::ConfigSchema | AppCommand::ConfigKeys => "config",
        AppCommand::Pause => "pause",
        AppCommand::Resume => "resume",
        AppCommand::Cancel => "cancel",
        AppCommand::Status => "status",
        AppCommand::Lock => "lock",
        AppCommand::Run => "run",
        AppCommand::Preview => "preview",
        AppCommand::LayerDelta => "layer delta",
    }
}

/// Copies the fields of `extra` (an object) into `document`.
fn merge(document: &mut Value, extra: Value) {
    if let (Some(document), Value::Object(extra)) = (document.as_object_mut(), extra) {
        document.extend(extra);
    }
}

/// The command-specific part of a document.
fn outcome_body(outcome: &CommandOutcome) -> Value {
    match outcome {
        CommandOutcome::Failed { message } => json!({ "status": "error", "error": message }),
        CommandOutcome::Json { value } => {
            let mut body = json!({ "status": "ok" });
            merge(&mut body, value.clone());
            body
        }
        CommandOutcome::Validated { spec, validation } => json!({
            "status": if validation.errors.is_empty() { "ok" } else { "failed" },
            "build": spec.identity.display_name,
            "error_count": validation.errors.len(),
            "warning_count": validation.warnings.len(),
            "diagnostics": validation_diagnostics(validation),
        }),
        CommandOutcome::Planned {
            plan,
            diagnostics,
            estimate,
            ..
        } => json!({
            "status": if diagnostics.is_empty() { "ok" } else { "diagnostics" },
            "build": plan.build_id.as_str(),
            "operation_count": plan.operations.len(),
            "execute_count": plan.operations.iter().filter(|op| op.reuse.should_execute()).count(),
            "operations": plan.operations.iter().map(|operation| {
                let (reason, reused_from) = match &operation.reuse {
                    OperationReuse::Execute(reason) => (
                        json!({ "code": reason.code, "message": reason.message }),
                        Value::Null,
                    ),
                    OperationReuse::Reuse { source } => (Value::Null, json!(source)),
                };
                json!({
                    "id": operation.id.as_str(),
                    "depends_on": operation.depends_on.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
                    "optionality": operation.optionality.as_str(),
                    "execute": operation.reuse.should_execute(),
                    "reason": reason,
                    "reused_from": reused_from,
                })
            }).collect::<Vec<_>>(),
            "estimate": {
                "executing_operations": estimate.executing_operations(),
                "timed_operations": estimate.timed_operations,
                "total_work_ms": duration_ms(estimate.total_work),
                "critical_path_ms": duration_ms(estimate.critical_path_duration),
                "critical_path": estimate.critical_path.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
                "untimed_operations": estimate.untimed_operations.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            },
            "diagnostics": plan_diagnostics(diagnostics),
        }),
        CommandOutcome::Previewed { report } => {
            let mut body = json!({ "status": if report.fail_on_clean && report.tripped() { "tripped" } else { "ok" } });
            merge(&mut body, crate::commands::preview_json(report));
            body
        }
        CommandOutcome::Ran {
            report,
            report_outputs,
            post_build_output,
            run_duration,
            validation,
            plan_diagnostics: diagnostics,
            execution_errors,
            ..
        } => json!({
            "status": run_status(outcome),
            "build": report.summary.build_name,
            "run_duration_ms": duration_ms(*run_duration),
            "summary": serde_json::to_value(&report.summary).unwrap_or(Value::Null),
            "report_files": report_outputs.files.iter().map(|file| json!({
                "kind": file.kind.as_str(),
                "path": file.path.display().to_string(),
                "bytes": file.bytes,
            })).collect::<Vec<_>>(),
            "validation_diagnostics": validation_diagnostics(validation),
            "plan_diagnostics": plan_diagnostics(diagnostics),
            "execution_errors": execution_errors.iter().map(|error| json!({
                "operation_id": error.operation_id.as_str(),
                "code": error.code,
                "message": error.message,
                "output_tail": error.output_tail,
            })).collect::<Vec<_>>(),
            "post_build_output": post_build_output,
        }),
        CommandOutcome::Variants { header, outcomes } => json!({
            "status": if outcomes.iter().all(|item| item.outcome.exit_code() == 0) { "ok" } else { "failed" },
            "header": header,
            "variants": outcomes.iter().map(variant_json).collect::<Vec<_>>(),
        }),
        CommandOutcome::Text { text } => json!({ "status": "ok", "text": text }),
        CommandOutcome::Help { text } | CommandOutcome::Version { text } => {
            json!({ "status": "ok", "text": text })
        }
        CommandOutcome::TuiExited { summary, .. } => json!({ "status": "ok", "text": summary }),
        // Commands that do not take --json; dispatch refuses the flag.
        CommandOutcome::Resolved { .. }
        | CommandOutcome::Cleaned { .. }
        | CommandOutcome::Locked { .. } => json!({ "status": "ok" }),
    }
}

fn variant_json(item: &VariantOutcome) -> Value {
    let mut variant = json!({ "id": item.id, "exit_code": item.outcome.exit_code() });
    merge(&mut variant, outcome_body(&item.outcome));
    variant
}

/// `succeeded`, `failed` or `cancelled`.
fn run_status(outcome: &CommandOutcome) -> &'static str {
    match outcome.exit_code() {
        0 => "succeeded",
        130 => "cancelled",
        _ => "failed",
    }
}

fn validation_diagnostics(validation: &gaia_validate::ValidationReport) -> Vec<Value> {
    validation
        .diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "severity": match diagnostic.severity {
                    gaia_validate::DiagnosticSeverity::Error => "error",
                    gaia_validate::DiagnosticSeverity::Warning => "warning",
                },
                "code": diagnostic.code,
                "location": diagnostic.location,
                "message": diagnostic.message,
            })
        })
        .collect()
}

fn plan_diagnostics(diagnostics: &[gaia_plan::PlanDiagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(|diagnostic| json!({ "code": diagnostic.code, "message": diagnostic.message }))
        .collect()
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
