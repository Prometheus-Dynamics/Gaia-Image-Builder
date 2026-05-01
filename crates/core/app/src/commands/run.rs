use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::plan_build_with_reuse_state;
use gaia_process::ProcessRunErrorKind;
use gaia_report::{generate_report, write_report_bundle};
use gaia_validate::validate_spec_with_providers;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use crate::AppContext;

use super::{CommandOutcome, RunArtifacts, load_reuse_state, save_reuse_state};

const DEFAULT_POST_BUILD_HOOK_TIMEOUT_SECONDS: u64 = 300;

pub fn run_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
) -> CommandOutcome {
    let run = match collect_run_artifacts(context, build, options) {
        Ok(run) => run,
        Err(message) => return CommandOutcome::Failed { message },
    };

    if !run.validation.errors.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "refusing to run build '{}': {} validation error(s)",
                run.spec.identity.display_name,
                run.validation.errors.len()
            ),
        };
    }

    if !run.plan_diagnostics.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "refusing to run build '{}': {} plan diagnostic(s)",
                run.spec.identity.display_name,
                run.plan_diagnostics.len()
            ),
        };
    }

    CommandOutcome::Ran {
        report: run.report,
        report_outputs: run.report_outputs,
        post_build_output: run.post_build_output,
        run_duration: run.run_duration,
        validation: run.validation,
        plan_diagnostics: run.plan_diagnostics,
        execution_errors: run.outcome.errors,
    }
}

fn collect_run_artifacts(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
) -> Result<RunArtifacts, String> {
    let span = tracing::info_span!("run_build", build);
    let _guard = span.enter();
    let started_at = Instant::now();
    let spec =
        try_resolve_config_with_options(build, options).map_err(|error| error.to_string())?;
    tracing::debug!(
        build_id = spec.identity.id.as_str(),
        build_name = spec.identity.build_name.as_str(),
        "resolved run build spec"
    );
    let validation = validate_spec_with_providers(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    tracing::debug!(
        errors = validation.errors.len(),
        warnings = validation.warnings.len(),
        diagnostics = validation.diagnostics.len(),
        "validated run build spec"
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
    tracing::debug!(
        operations = plan.operations.len(),
        diagnostics = plan_diagnostics.len(),
        reuse_state = reuse_state.is_some(),
        "planned run build"
    );
    if !validation.errors.is_empty() {
        return run_artifacts_without_execution(
            started_at,
            spec,
            validation,
            plan,
            plan_diagnostics,
        );
    }
    if !plan_diagnostics.is_empty() {
        return run_artifacts_without_execution(
            started_at,
            spec,
            validation,
            plan,
            plan_diagnostics,
        );
    }
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &context.source_catalog,
            artifact_catalog: &context.artifact_catalog,
            image_catalog: &context.image_catalog,
        },
    );
    tracing::debug!(
        completed = outcome.completed_operations,
        reused = outcome.reused_ids.len(),
        errors = outcome.errors.len(),
        cancelled = outcome.cancelled,
        "executed run build"
    );
    let report = generate_report(&spec, &validation, &plan, &outcome);
    let report_outputs = write_report_outputs(&spec, &report)?;
    let run_duration = started_at.elapsed();
    let post_build_output = run_post_build_hook(&spec, &report, &report_outputs, run_duration)
        .map_err(|error| {
            format!(
                "post-build hook failed for build '{}': {error}",
                spec.identity.display_name
            )
        })?;
    if outcome.errors.is_empty() {
        save_reuse_state(&spec, &plan, &outcome);
    }

    Ok(RunArtifacts {
        spec,
        validation,
        plan,
        plan_diagnostics,
        outcome,
        report,
        report_outputs,
        post_build_output,
        run_duration,
    })
}

fn run_artifacts_without_execution(
    started_at: Instant,
    spec: gaia_spec::ResolvedBuildSpec,
    validation: gaia_validate::ValidationReport,
    plan: gaia_plan::ExecutionPlan,
    plan_diagnostics: Vec<gaia_plan::PlanDiagnostic>,
) -> Result<RunArtifacts, String> {
    let outcome = gaia_exec::ExecutionOutcome::default();
    let report = generate_report(&spec, &validation, &plan, &outcome);
    let report_outputs = write_report_outputs(&spec, &report)?;
    Ok(RunArtifacts {
        spec,
        validation,
        plan,
        plan_diagnostics,
        outcome,
        report,
        report_outputs,
        post_build_output: None,
        run_duration: started_at.elapsed(),
    })
}

fn write_report_outputs(
    spec: &gaia_spec::ResolvedBuildSpec,
    report: &gaia_report::ReportBundle,
) -> Result<gaia_report::ReportOutputBundle, String> {
    write_report_bundle(spec, report).map_err(|error| {
        tracing::warn!(
            build_id = spec.identity.id.as_str(),
            build_name = spec.identity.display_name.as_str(),
            error = %error,
            "failed to write report outputs"
        );
        format!(
            "failed to write report outputs for build '{}': {error}",
            spec.identity.display_name
        )
    })
}

#[derive(Debug, Serialize)]
struct PostBuildPayload {
    build_name: String,
    build_version: Option<String>,
    build_target: Option<String>,
    build_profile: Option<String>,
    status: &'static str,
    run_duration_ms: u128,
    primary_output: Option<PostBuildPrimaryOutput>,
    report_dir: Option<String>,
    report_files: Vec<PostBuildReportFile>,
    summary: PostBuildSummary,
}

#[derive(Debug, Serialize)]
struct PostBuildPrimaryOutput {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct PostBuildReportFile {
    kind: String,
    path: String,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct PostBuildSummary {
    operation_count: usize,
    completed_operations: usize,
    reused_operations: usize,
    rolled_back_operations: usize,
    warning_count: usize,
    error_count: usize,
}

fn run_post_build_hook(
    spec: &gaia_spec::ResolvedBuildSpec,
    report: &gaia_report::ReportBundle,
    report_outputs: &gaia_report::ReportOutputBundle,
    run_duration: std::time::Duration,
) -> Result<Option<String>, String> {
    let Some(hook) = &spec.reporting.post_build else {
        return Ok(None);
    };
    let payload = build_post_build_payload(report, report_outputs, run_duration)
        .map_err(|error| format!("failed to assemble post-build payload: {error}"))?;
    let report_dir = report_outputs
        .files
        .first()
        .and_then(|file| file.path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            PathBuf::from(&spec.workspace.out_dir)
                .join(".gaia")
                .join("reports")
        });
    fs::create_dir_all(&report_dir).map_err(|error| {
        format!(
            "failed to create report dir '{}': {error}",
            report_dir.display()
        )
    })?;
    let payload_path = report_dir.join(format!(
        "{}.post-build-payload.json",
        spec.build_name().replace(['/', '\\', ' '], "-")
    ));
    let payload_bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|error| format!("failed to encode post-build payload: {error}"))?;
    fs::write(&payload_path, payload_bytes).map_err(|error| {
        format!(
            "failed to write post-build payload '{}': {error}",
            payload_path.display()
        )
    })?;

    let script_path = resolve_hook_script_path(spec, &hook.script);
    let mut command = if is_executable_file(&script_path) {
        Command::new(&script_path)
    } else {
        let mut command = Command::new("bash");
        command.arg(&script_path);
        command
    };
    command
        .arg(&payload_path)
        .current_dir(&spec.workspace.root_dir)
        .env("GAIA_POST_BUILD_PAYLOAD", &payload_path)
        .env("GAIA_POST_BUILD_REPORT_DIR", &report_dir);
    let timeout_seconds = if hook.timeout_seconds == 0 {
        DEFAULT_POST_BUILD_HOOK_TIMEOUT_SECONDS
    } else {
        hook.timeout_seconds.max(1)
    };
    let retention = spec.policy.execution.output_retention;
    let output = gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        Duration::from_secs(timeout_seconds),
        &format!("post-build hook '{}'", script_path.display()),
        gaia_process::ProcessOutputRetention {
            stdout_bytes: retention.stdout_bytes,
            stderr_bytes: retention.stderr_bytes,
            stdout_lines: retention.stdout_lines,
            stderr_lines: retention.stderr_lines,
        },
        None,
        None,
    )
    .map_err(|error| match error.kind {
        ProcessRunErrorKind::ToolStart => {
            format!(
                "failed to start hook '{}': {}",
                script_path.display(),
                error.message
            )
        }
        ProcessRunErrorKind::Timeout => {
            format!(
                "hook '{}' timed out after {timeout_seconds}s",
                script_path.display()
            )
        }
        ProcessRunErrorKind::Cancelled => {
            format!("hook '{}' was cancelled", script_path.display())
        }
        ProcessRunErrorKind::RuntimeState => {
            format!(
                "hook '{}' failed at runtime: {}",
                script_path.display(),
                error.message
            )
        }
    })?
    .output;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!(
                "hook '{}' exited with status {}",
                script_path.display(),
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "signal".into())
            )
        } else {
            format!("hook '{}' failed: {}", script_path.display(), stderr)
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(Some(stdout))
}

fn build_post_build_payload(
    report: &gaia_report::ReportBundle,
    report_outputs: &gaia_report::ReportOutputBundle,
    run_duration: std::time::Duration,
) -> io::Result<PostBuildPayload> {
    let primary_output = report
        .summary
        .primary_image_output
        .as_deref()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .map(|path| primary_output_payload(&path))
        .transpose()?;
    let report_dir = report_outputs
        .files
        .first()
        .and_then(|file| file.path.parent())
        .map(|path| path.display().to_string());
    Ok(PostBuildPayload {
        build_name: report.summary.build_name.clone(),
        build_version: report.summary.build_version.clone(),
        build_target: report.summary.build_target.clone(),
        build_profile: report.summary.build_profile.clone(),
        status: if report.summary.error_count == 0 {
            "completed"
        } else {
            "failed"
        },
        run_duration_ms: run_duration.as_millis(),
        primary_output,
        report_dir,
        report_files: report_outputs
            .files
            .iter()
            .map(|file| PostBuildReportFile {
                kind: file.kind.as_str().to_string(),
                path: file.path.display().to_string(),
                bytes: file.bytes,
            })
            .collect(),
        summary: PostBuildSummary {
            operation_count: report.summary.operation_count,
            completed_operations: report.summary.completed_operations,
            reused_operations: report.summary.reused_operations,
            rolled_back_operations: report.summary.rolled_back_operations,
            warning_count: report.summary.warning_count,
            error_count: report.summary.error_count,
        },
    })
}

fn primary_output_payload(path: &Path) -> io::Result<PostBuildPrimaryOutput> {
    Ok(PostBuildPrimaryOutput {
        path: path.display().to_string(),
        bytes: fs::metadata(path)?.len(),
        sha256: sha256_file(path)?,
    })
}

fn sha256_file(path: &Path) -> io::Result<String> {
    use std::io::Read;

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn resolve_hook_script_path(spec: &gaia_spec::ResolvedBuildSpec, script: &str) -> PathBuf {
    let path = PathBuf::from(script);
    if path.is_absolute() {
        path
    } else {
        PathBuf::from(&spec.workspace.root_dir).join(path)
    }
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
