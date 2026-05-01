mod cli;
mod commands;
#[cfg(feature = "tui")]
pub mod tui;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_default_providers::ProviderCatalogs;
use gaia_image_providers::ImageProviderCatalog;
use gaia_report::{mask_pairs, mask_value};
use gaia_source_providers::SourceProviderCatalog;
use gaia_spec::ResolvedBuildSpec;
use sha2::{Digest, Sha256};
use std::any::Any;
use std::fs;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Duration;

pub use cli::{AppArgs, AppCommand, CleanArgs};
pub use commands::{CommandOutcome, CommandResult};

#[derive(Default)]
pub struct AppContext {
    pub source_catalog: SourceProviderCatalog,
    pub artifact_catalog: ArtifactProviderCatalog,
    pub image_catalog: ImageProviderCatalog,
}

impl AppContext {
    pub fn with_defaults() -> Self {
        let (source_catalog, artifact_catalog, image_catalog) =
            ProviderCatalogs::with_defaults().into_parts();

        Self {
            source_catalog,
            artifact_catalog,
            image_catalog,
        }
    }
}

pub fn run() -> i32 {
    let args = AppArgs::from_env();
    let outcome = run_with_args(args);
    print_outcome(&outcome);
    outcome.exit_code()
}

pub fn run_with_args(args: AppArgs) -> CommandOutcome {
    let context = AppContext::with_defaults();
    match panic::catch_unwind(AssertUnwindSafe(|| commands::dispatch(&context, args))) {
        Ok(outcome) => outcome,
        Err(payload) => CommandOutcome::Failed {
            message: format!("command failed: {}", panic_message(payload.as_ref())),
        },
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    "unknown panic".into()
}

impl CommandOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Help { .. } | Self::Version { .. } => 0,
            Self::TuiExited { exit_code, .. } => *exit_code,
            Self::Failed { .. } => 1,
            Self::Validated { validation, .. } if !validation.errors.is_empty() => 2,
            Self::Planned { diagnostics, .. } if !diagnostics.is_empty() => 3,
            Self::Ran {
                report,
                validation,
                plan_diagnostics,
                ..
            } if report.summary.error_count > 0
                || !validation.errors.is_empty()
                || !plan_diagnostics.is_empty() =>
            {
                4
            }
            _ => 0,
        }
    }
}

fn print_outcome(outcome: &CommandOutcome) {
    match outcome {
        CommandOutcome::Help { text } | CommandOutcome::Version { text } => {
            println!("{text}");
        }
        CommandOutcome::TuiExited { summary, .. } if !summary.is_empty() => {
            println!("{summary}");
        }
        CommandOutcome::TuiExited { .. } => {}
        CommandOutcome::Resolved { spec } => {
            println!(
                "resolved build '{}' with {} source(s), {} artifact(s), {} install(s)",
                spec.identity.display_name,
                spec.sources.len(),
                spec.artifacts.len(),
                spec.install.entries.len()
            );
            print_selection(spec);
            for line in backend_overview_lines(spec) {
                println!("{line}");
            }
        }
        CommandOutcome::Validated { spec, validation } => {
            println!(
                "validation: {} error(s), {} warning(s)",
                validation.errors.len(),
                validation.warnings.len()
            );
            print_selection(spec);
            for line in backend_overview_lines(spec) {
                println!("{line}");
            }
            for diagnostic in &validation.diagnostics {
                let location = diagnostic
                    .location
                    .as_deref()
                    .map(|value| format!(" [{value}]"))
                    .unwrap_or_default();
                println!("{}{}: {}", diagnostic.code, location, diagnostic.message);
            }
        }
        CommandOutcome::Planned {
            spec,
            plan,
            diagnostics,
        } => {
            println!(
                "plan for '{}' has {} operation(s)",
                plan.build_id.as_str(),
                plan.operations.len()
            );
            let required = plan
                .operations
                .iter()
                .filter(|operation| {
                    operation.optionality == gaia_plan::OperationOptionality::Required
                })
                .count();
            let conditional = plan
                .operations
                .iter()
                .filter(|operation| {
                    operation.optionality == gaia_plan::OperationOptionality::Conditional
                })
                .count();
            let best_effort = plan
                .operations
                .iter()
                .filter(|operation| {
                    operation.optionality == gaia_plan::OperationOptionality::BestEffort
                })
                .count();
            println!(
                "plan optionality: required={} conditional={} best-effort={}",
                required, conditional, best_effort
            );
            print_selection(spec);
            for line in backend_overview_lines(spec) {
                println!("{line}");
            }
            for diagnostic in diagnostics {
                println!("plan {}: {}", diagnostic.code, diagnostic.message);
            }
        }
        CommandOutcome::Cleaned { spec, report } => {
            let action = if report.dry_run {
                "would clean"
            } else {
                "cleaned"
            };
            println!(
                "{action} build '{}' paths={} missing={}",
                spec.identity.display_name,
                report.removed.len(),
                report.missing.len()
            );
            for path in &report.removed {
                println!("{action}: {}", path.display());
            }
            for path in &report.missing {
                println!("clean missing: {}", path.display());
            }
        }
        CommandOutcome::Ran {
            report,
            report_outputs,
            post_build_output,
            run_duration,
            validation,
            plan_diagnostics,
            execution_errors,
        } => {
            if let Some(output) = post_build_output
                && !output.trim().is_empty()
            {
                println!("{output}");
                return;
            }
            println!(
                "run summary: build='{}' operations={} completed={} reused={} image_reused={} errors={} warnings={}",
                report.summary.build_name,
                report.summary.operation_count,
                report.summary.completed_operations,
                report.summary.reused_operations,
                report.summary.image_reused,
                report.summary.error_count,
                report.summary.warning_count
            );
            if !report.summary.image_reuse_details.is_empty() {
                println!(
                    "image reuse: {}",
                    report.summary.image_reuse_details.join(", ")
                );
            }
            if report.summary.rolled_back_operations > 0 {
                println!(
                    "rollback: operations={}",
                    report.summary.rolled_back_operations
                );
            } else if report.summary.error_count > 0 && !report.summary.rollback_on_error {
                println!("rollback: disabled-by-policy");
            }
            println!(
                "failure policy: rollback_on_error={} preserve_failed_outputs={} rollback_domains={}",
                report.summary.rollback_on_error,
                report.summary.preserve_failed_outputs,
                rollback_domains_display(&report.summary.rollback_domains),
            );
            println!(
                "provenance: sources={} artifacts={} image={:?}",
                report.provenance.source_providers.len(),
                report.provenance.artifact_providers.len(),
                report.provenance.image_provider
            );
            println!(
                "manifest: operations={} sources={} artifacts={} installs={}",
                report.manifest.operations.len(),
                report.manifest.sources.len(),
                report.manifest.artifacts.len(),
                report.manifest.installs.len()
            );
            println!(
                "checkpoints: total={} built={} reused={}",
                report.summary.checkpoint_count,
                report.summary.checkpoint_built_count,
                report.summary.checkpoint_reused_count,
            );
            for line in runtime_overview_lines(report) {
                println!("{line}");
            }
            if let Some(selected_build_file) = &report.provenance.selected_build_file {
                println!("selection build-file: {selected_build_file}");
            }
            if let Some(selected_preset) = &report.provenance.selected_preset {
                println!("selection preset: {selected_preset}");
            }
            if let Some(branch) = &report.summary.build_branch {
                println!("build branch: {branch}");
            }
            if let Some(target) = &report.summary.build_target {
                println!("build target: {target}");
            }
            if let Some(profile) = &report.summary.build_profile {
                println!("build profile: {profile}");
            }
            if let Some(primary_image_output) = &report.summary.primary_image_output {
                println!("primary image output: {primary_image_output}");
                if let Ok(bytes) = fs::metadata(primary_image_output).map(|metadata| metadata.len())
                {
                    println!("primary image size: {} bytes", bytes);
                }
                if let Ok(sha256) = sha256_file(Path::new(primary_image_output)) {
                    println!("primary image sha256: {sha256}");
                }
            }
            println!("run time: {}", format_elapsed(*run_duration));
            if !report.provenance.selected_inputs.is_empty() {
                println!(
                    "selection inputs: {}",
                    report
                        .provenance
                        .selected_inputs
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !report.provenance.selected_env_files.is_empty() {
                println!(
                    "selection env-files: {}",
                    report.provenance.selected_env_files.join(", ")
                );
            }
            if !report.provenance.selected_env_overrides.is_empty() {
                println!(
                    "selection env-overrides: {}",
                    report
                        .provenance
                        .selected_env_overrides
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !report.provenance.precedence_order.is_empty() {
                println!(
                    "selection precedence: {}",
                    report.provenance.precedence_order.join(" -> ")
                );
            }
            if !report.provenance.explicit_overrides.is_empty() {
                println!(
                    "selection overrides: {}",
                    report
                        .provenance
                        .explicit_overrides
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !report.provenance.metadata_labels.is_empty() {
                println!(
                    "selection metadata-labels: {}",
                    report
                        .provenance
                        .metadata_labels
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            println!("rebuild reasons={}", report.rebuild_reasons.len());
            println!(
                "execution events={}",
                report.provenance.completed_operation_ids.len()
            );
            if !report.summary.failure_classes.is_empty() {
                println!(
                    "execution failure-classes: {}",
                    report
                        .summary
                        .failure_classes
                        .iter()
                        .map(|entry| format!("{:?}={}", entry.class, entry.count))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            for output in &report_outputs.files {
                println!(
                    "output {}: {} ({} bytes)",
                    output.kind.as_str(),
                    output.path.display(),
                    output.bytes
                );
            }
            for reason in report.rebuild_reasons.iter().take(5) {
                println!("rebuild {}: {}", reason.code, reason.message);
            }
            for diagnostic in &validation.diagnostics {
                let location = diagnostic
                    .location
                    .as_deref()
                    .map(|value| format!(" [{value}]"))
                    .unwrap_or_default();
                println!(
                    "validation {}{}: {}",
                    diagnostic.code, location, diagnostic.message
                );
            }
            for diagnostic in plan_diagnostics {
                println!("plan {}: {}", diagnostic.code, diagnostic.message);
            }
            for error in execution_errors {
                println!(
                    "execution-error {} [{}]: {}",
                    error.code,
                    error.operation_id.as_str(),
                    error.message
                );
                for line in error.output_tail.iter().take(5) {
                    println!(
                        "execution-output [{}]: {}",
                        error.operation_id.as_str(),
                        line
                    );
                }
            }
            for failure in report.execution_failures.iter().take(5) {
                println!(
                    "execution-failure {} {:?} [{}]: {}",
                    failure.code, failure.class, failure.operation_id, failure.message
                );
                for line in failure.output_tail.iter().take(5) {
                    println!(
                        "execution-failure-output [{}]: {}",
                        failure.operation_id, line
                    );
                }
            }
        }
        CommandOutcome::Failed { message } => {
            eprintln!("{message}");
        }
    }
}

fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
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

pub fn backend_overview_lines(spec: &ResolvedBuildSpec) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "providers: sources={} artifacts={} image={:?}",
        spec.sources.len(),
        spec.artifacts.len(),
        spec.image.provider_kind()
    ));
    lines.push(format!(
        "runtime plan: installs={} stage-files={} stage-envs={} stage-services={} checkpoints={}",
        spec.install.entries.len(),
        spec.stage.files.len(),
        spec.stage.env_sets.len(),
        spec.stage.services.len(),
        spec.checkpoints.points.len(),
    ));
    lines.push(format!(
        "execution jobs: {}",
        if spec.policy.execution.jobs == 0 {
            "all".to_string()
        } else {
            spec.policy.execution.jobs.to_string()
        }
    ));
    lines.push(format!(
        "execution backend: {}",
        spec.policy
            .execution
            .docker
            .as_ref()
            .map(|docker| format!("docker ({})", docker.image))
            .unwrap_or_else(|| "host".to_string())
    ));
    lines.push(format!(
        "failure policy: rollback_on_error={} preserve_failed_outputs={} rollback_domains={}",
        spec.policy.failure.rollback_on_error,
        spec.policy.failure.preserve_failed_outputs,
        rollback_domains_display(
            &spec
                .policy
                .failure
                .rollback_domains
                .iter()
                .map(|domain| domain.as_str().to_string())
                .collect::<Vec<_>>(),
        )
    ));
    if let Some(install) = spec.install.entries.first() {
        lines.push(format!(
            "runtime install target: {} -> {}",
            install.id.as_str(),
            install.dest
        ));
    }
    if let Some(file) = spec.stage.files.first() {
        lines.push(format!(
            "runtime stage-file target: {} -> {}",
            file.id.as_str(),
            file.dest
        ));
    }
    if let Some(checkpoint) = spec.checkpoints.points.first() {
        let backend = checkpoint
            .backend
            .as_ref()
            .map(|backend| backend.backend.as_str())
            .unwrap_or("default");
        lines.push(format!(
            "runtime checkpoint target: {} via {}",
            checkpoint.id.as_str(),
            backend
        ));
    }
    lines
}

fn rollback_domains_display(domains: &[String]) -> String {
    if domains.is_empty() {
        "none".into()
    } else {
        domains.join(",")
    }
}

pub fn runtime_overview_lines(report: &gaia_report::ReportBundle) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "runtime state: installs={} stage-files={} stage-envs={} stage-services={} checkpoints={}",
        report.provenance.install_backend_states.len(),
        report.provenance.stage_file_backend_states.len(),
        report.provenance.stage_env_set_backend_states.len(),
        report.provenance.stage_service_backend_states.len(),
        report.provenance.checkpoint_backend_states.len(),
    ));
    if let Some(record) = report.provenance.install_backend_states.first()
        && let Some(dest) = record.state.get("dest")
    {
        lines.push(format!("runtime install sample: {} -> {}", record.id, dest));
    }
    if let Some(record) = report.provenance.stage_file_backend_states.first()
        && let Some(dest) = record.state.get("dest")
    {
        lines.push(format!(
            "runtime stage-file sample: {} -> {}",
            record.id, dest
        ));
    }
    if let Some(record) = report.provenance.checkpoint_backend_states.first() {
        let backend = record
            .state
            .get("backend")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| "default".into());
        lines.push(format!(
            "runtime checkpoint sample: {} via {}",
            record.id, backend
        ));
    }
    lines
}

fn print_selection(spec: &gaia_spec::ResolvedBuildSpec) {
    if let Some(selected_build_file) = &spec.selection.selected_build_file {
        println!("selection build-file: {selected_build_file}");
    }
    if let Some(selected_preset) = &spec.selection.selected_preset {
        println!("selection preset: {selected_preset}");
    }
    if let Some(branch) = &spec.metadata.branch {
        println!("build branch: {branch}");
    }
    if let Some(target) = &spec.metadata.target {
        println!("build target: {target}");
    }
    if let Some(profile) = &spec.metadata.profile {
        println!("build profile: {profile}");
    }
    if !spec.selection.selected_inputs.is_empty() {
        let masked = mask_pairs(&spec.selection.selected_inputs, &spec.reporting);
        println!(
            "selection inputs: {}",
            masked
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(requested_build) = &spec.selection.requested_build {
        println!("selection requested-build: {requested_build}");
    }
    if !spec.selection.env_files.is_empty() {
        println!(
            "selection env-files: {}",
            spec.selection.env_files.join(", ")
        );
    }
    if !spec.selection.env_overrides.is_empty() {
        let masked = mask_pairs(&spec.selection.env_overrides, &spec.reporting);
        println!(
            "selection env-overrides: {}",
            masked
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !spec.selection.precedence_order.is_empty() {
        println!(
            "selection precedence: {}",
            spec.selection.precedence_order.join(" -> ")
        );
    }
    if !spec.selection.explicit_overrides.is_empty() {
        println!(
            "selection overrides: {}",
            spec.selection
                .explicit_overrides
                .iter()
                .map(|(key, value)| {
                    let display_value = if let Some(env_key) = key.strip_prefix("env.") {
                        mask_value(env_key, value, &spec.reporting)
                    } else {
                        value.clone()
                    };
                    format!("{key}={display_value}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
