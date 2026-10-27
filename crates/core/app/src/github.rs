//! GitHub Actions output. With `GITHUB_ACTIONS=true` every validation
//! diagnostic and failed operation is also printed as a workflow command
//! (`::error title=...::...`), so it shows up as an annotation on the run.
//! With `GITHUB_STEP_SUMMARY` set, `gaia run` appends a Markdown summary of
//! the run to that file: the outcome, the operation counts, the image sizes,
//! the errors and the finish lines (exports, resume hint).

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use gaia_report::ReportBundle;
use gaia_validate::{DiagnosticSeverity, ValidationReport};

use crate::output::{format_bytes, format_elapsed};
use crate::output_images::size_change_text;
use crate::run_finish::RunFinish;
use crate::{AppCommand, CommandOutcome};

/// Writes the GitHub output of `outcome`, when running in GitHub Actions.
/// `annotations` is false for `--json`, whose stdout is one JSON document.
pub(crate) fn write_github_output(
    command: AppCommand,
    outcome: &CommandOutcome,
    finish: &RunFinish,
    annotations: bool,
) {
    if annotations && std::env::var("GITHUB_ACTIONS").is_ok_and(|value| value == "true") {
        for line in annotation_lines(outcome) {
            println!("{line}");
        }
    }
    if command != AppCommand::Run {
        return;
    }
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|path| !path.is_empty()) else {
        return;
    };
    let summary = step_summary(outcome, finish);
    if summary.is_empty() {
        return;
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(summary.as_bytes()));
    if let Err(error) = written {
        eprintln!(
            "warning: could not write the GitHub step summary to '{}': {error}",
            std::path::Path::new(&path).display()
        );
    }
}

/// The workflow commands for the diagnostics and errors in `outcome`.
pub(crate) fn annotation_lines(outcome: &CommandOutcome) -> Vec<String> {
    let mut lines = Vec::new();
    match outcome {
        CommandOutcome::Validated { validation, .. } => {
            validation_annotations(validation, &mut lines);
        }
        CommandOutcome::Ran {
            validation, report, ..
        } => {
            validation_annotations(validation, &mut lines);
            for error in &report.summary.errors {
                lines.push(format!(
                    "::error title={}::{}",
                    escape_property(&format!("gaia {} ({})", error.code, error.operation_id)),
                    escape_data(&error.message)
                ));
            }
        }
        CommandOutcome::Variants { outcomes, .. } => {
            for item in outcomes {
                lines.extend(annotation_lines(&item.outcome));
            }
        }
        CommandOutcome::Failed { message } => {
            lines.push(format!("::error title=gaia::{}", escape_data(message)));
        }
        _ => {}
    }
    lines
}

fn validation_annotations(validation: &ValidationReport, lines: &mut Vec<String>) {
    for diagnostic in &validation.diagnostics {
        let level = match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
        };
        let message = match &diagnostic.location {
            Some(location) => format!("{location}: {}", diagnostic.message),
            None => diagnostic.message.clone(),
        };
        lines.push(format!(
            "::{level} title={}::{}",
            escape_property(&format!("gaia {}", diagnostic.code)),
            escape_data(&message)
        ));
    }
}

/// The Markdown summary of a `gaia run` outcome; empty for other outcomes.
pub(crate) fn step_summary(outcome: &CommandOutcome, finish: &RunFinish) -> String {
    match outcome {
        CommandOutcome::Ran {
            report,
            run_duration,
            ..
        } => run_summary(report, *run_duration, outcome.exit_code(), &finish.lines),
        CommandOutcome::Variants { outcomes, .. } => outcomes
            .iter()
            .map(|item| step_summary(&item.outcome, &RunFinish::default()))
            .collect(),
        CommandOutcome::Failed { message } => {
            format!(
                "### gaia run failed\n\n```text\n{}\n```\n\n",
                message.trim_end()
            )
        }
        _ => String::new(),
    }
}

fn run_summary(
    report: &ReportBundle,
    duration: Duration,
    exit_code: i32,
    finish: &[String],
) -> String {
    let summary = &report.summary;
    let verdict = if summary.cancelled {
        "cancelled"
    } else if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    let name = match &summary.build_version {
        Some(version) => format!("{} {version}", summary.build_name),
        None => summary.build_name.clone(),
    };
    let mut out = String::new();
    let _ = writeln!(out, "### gaia run `{}`: {verdict}\n", cell(&name));
    let _ = writeln!(out, "| | |\n|---|---|");
    let _ = writeln!(out, "| run time | {} |", format_elapsed(duration));
    let _ = writeln!(
        out,
        "| operations | {} ({} built, {} reused) |",
        summary.operation_count,
        summary
            .completed_operations
            .saturating_sub(summary.reused_operations),
        summary.reused_operations
    );
    if let Some(target) = &summary.build_target {
        let _ = writeln!(out, "| target | {} |", cell(target));
    }
    let _ = writeln!(
        out,
        "| warnings / errors | {} / {} |",
        summary.warning_count, summary.error_count
    );
    if let Some(primary) = &summary.primary_image_output {
        let _ = writeln!(out, "| primary image | `{}` |", cell(primary));
    }
    if !summary.sizes.is_empty() {
        let _ = writeln!(
            out,
            "\n| image output | kind | size | change |\n|---|---|---:|---|"
        );
        for record in &summary.sizes {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} |",
                cell(&record.name),
                record.kind,
                format_bytes(record.bytes),
                cell(&size_change_text(record))
            );
        }
    }
    if !summary.errors.is_empty() {
        let _ = writeln!(out, "\n**Errors**\n");
        for error in &summary.errors {
            let _ = writeln!(
                out,
                "- `{}` {}: {}",
                error.operation_id,
                error.code,
                error.message.lines().next().unwrap_or_default()
            );
        }
    }
    if !finish.is_empty() {
        let _ = writeln!(out, "\n```text\n{}\n```", finish.join("\n"));
    }
    out.push('\n');
    out
}

/// Text inside a Markdown table cell: pipes escaped, one line.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// A workflow command's message: `%`, CR and LF encoded.
fn escape_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A workflow command's property value: also `:` and `,` encoded.
fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_validate::ValidationDiagnostic;

    #[test]
    fn diagnostics_become_annotations_with_escaped_text() {
        let validation = ValidationReport {
            warnings: Vec::new(),
            errors: Vec::new(),
            diagnostics: vec![
                ValidationDiagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "secret_undefined",
                    message: "names secret 'x'\n100% missing".into(),
                    location: Some("artifacts.app".into()),
                },
                ValidationDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: "a,b:c",
                    message: "careful".into(),
                    location: None,
                },
            ],
        };
        let mut lines = Vec::new();
        validation_annotations(&validation, &mut lines);
        assert_eq!(
            lines,
            [
                "::error title=gaia secret_undefined::artifacts.app: names secret 'x'%0A100%25 missing",
                "::warning title=gaia a%2Cb%3Ac::careful",
            ]
        );
    }

    #[test]
    fn a_failed_command_is_one_error_annotation_and_a_summary_block() {
        let outcome = CommandOutcome::Failed {
            message: "config did not resolve".into(),
        };
        assert_eq!(
            annotation_lines(&outcome),
            ["::error title=gaia::config did not resolve"]
        );
        assert_eq!(
            step_summary(&outcome, &RunFinish::default()),
            "### gaia run failed\n\n```text\nconfig did not resolve\n```\n\n"
        );
    }

    #[test]
    fn table_cells_escape_pipes_and_newlines() {
        assert_eq!(cell("a|b\nc"), "a\\|b c");
    }
}
