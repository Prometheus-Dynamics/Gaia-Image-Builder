//! `image:check`: reads the built image and its kernel tree, and checks them
//! against each other. It writes nothing but its report, `<out>/<build>.checks.json`,
//! and fails the run on an error. Warnings reach the run summary.

mod boot;
mod elf;
mod fdt;
mod hardware;
mod kernel;
mod kernel_modules;
mod module;
#[cfg(test)]
mod test_dtb;
#[cfg(test)]
mod tests;
mod uboot;

use super::helpers::{failure_with_kind, success_from_messages};
use super::{ExecutionErrorKind, OperationExecutionResult};
use gaia_plan::OperationId;
use gaia_spec::{ResolvedBuildSpec, RollbackDomain};
use kernel_modules::{Finding, Inputs, Severity};
use std::path::{Path, PathBuf};

/// The report's schema version, bumped when its shape changes.
const REPORT_SCHEMA: u32 = 1;
/// How many errors the failure message names before it counts the rest.
const NAMED_ERRORS: usize = 5;

pub(crate) fn execute(
    spec: &ResolvedBuildSpec,
    operation_id: &OperationId,
) -> OperationExecutionResult {
    let report_path = gaia_spec::image_checks_report_path(spec);
    let buildroot = match gaia_spec::buildroot_output_dir(spec) {
        Ok(dir) => dir,
        Err(message) => {
            return failure_with_kind(
                operation_id.clone(),
                "image_check_failed",
                ExecutionErrorKind::ImageCheck,
                message,
            );
        }
    };
    let checks = &spec.image.checks;
    let resolve = |raw: &str| {
        gaia_spec::resolve_workspace_path(&spec.workspace, raw)
            .unwrap_or_else(|_| PathBuf::from(raw))
    };
    let explicit =
        checks.kernel_image.is_some() || checks.modules_dir.is_some() || checks.symvers.is_some();
    let config = std::fs::read_to_string(buildroot.join(".config")).unwrap_or_default();
    let builds_kernel = config
        .lines()
        .any(|line| line.trim() == "BR2_LINUX_KERNEL=y");
    // `BR2_ARCH="aarch64"`: what the U-Boot check holds kernels to.
    let arch = config
        .lines()
        .find_map(|line| line.trim().strip_prefix("BR2_ARCH="))
        .and_then(|value| gaia_spec::target_arch(value.trim_matches('"')));
    if !builds_kernel && !explicit {
        // The hardware list needs the kernel's modules: without a kernel it
        // cannot be met, so it is an error rather than a skip.
        if !spec.image.hardware.is_empty() {
            return failure_with_kind(
                operation_id.clone(),
                "image_check_failed",
                ExecutionErrorKind::ImageCheck,
                format!(
                    "image hardware list cannot be checked: the Buildroot config builds no kernel (BR2_LINUX_KERNEL is not y), and {} hardware entries need its modules",
                    spec.image.hardware.len()
                ),
            );
        }
        let message = "image checks skipped: the Buildroot config builds no kernel (BR2_LINUX_KERNEL is not y)";
        let report = report_json(spec, "skipped", None, &[], &[], &buildroot);
        return match write_report(&report_path, &report) {
            Ok(()) => success_from_messages(
                operation_id.clone(),
                vec![message.to_string()],
                message.to_string(),
                RollbackDomain::Images,
                vec![report_path],
            ),
            Err(error) => report_failure(operation_id, &error),
        };
    }

    let kernel_image = checks
        .kernel_image
        .as_deref()
        .map(resolve)
        .unwrap_or_else(|| buildroot.join("images/Image"));
    let modules_root = buildroot.join("target/lib/modules");
    let modules_dir = checks.modules_dir.as_deref().map(resolve);
    let symvers = match checks.symvers.as_deref() {
        Some(raw) => Some(resolve(raw)),
        None => find_symvers(&buildroot.join("build")),
    };
    let firmware_dir = buildroot.join("target/lib/firmware");
    let images_dir = buildroot.join("images");
    let rootfs = buildroot.join("target");
    // An assembly's trees hold the boot files the board boots (their overlays,
    // config.txt and DTBs). A RAM work dir's trees are gone by now, so each
    // tree is read from the copy of its boot files the assembly left
    // (`boot_tree_snapshot_dir`), and from its disk path only without one.
    let snapshot = gaia_spec::boot_tree_snapshot_dir(spec);
    let labeled_trees: Vec<(PathBuf, String)> = match spec.image.assembly.as_ref() {
        Some(assembly) => match gaia_spec::AssemblyRoots::new(spec, assembly) {
            Ok(roots) => roots
                .trees
                .iter()
                .map(|(id, path)| {
                    let copy = snapshot.join(id.as_str());
                    let read = if copy.is_dir() { copy } else { path.clone() };
                    (read, id.as_str().to_string())
                })
                .collect(),
            Err(message) => {
                return failure_with_kind(
                    operation_id.clone(),
                    "image_check_failed",
                    ExecutionErrorKind::ImageCheck,
                    format!("image checks cannot find the assembly trees: {message}"),
                );
            }
        },
        None => Vec::new(),
    };
    let assembly_trees: Vec<PathBuf> = labeled_trees.iter().map(|(path, _)| path.clone()).collect();
    let excluded_packages: Vec<String> = match &spec.image.definition {
        gaia_spec::ImageDefinition::Buildroot(buildroot_spec) => {
            buildroot_spec.rootfs_exclude_packages.clone()
        }
        _ => Vec::new(),
    };
    let removed = hardware::rootfs_exclusions(&buildroot, &excluded_packages);
    let mut outcome = kernel_modules::check(&Inputs {
        kernel_image: &kernel_image,
        modules_root: &modules_root,
        modules_dir: modules_dir.as_deref(),
        modules_dir_set: checks.modules_dir.is_some(),
        symvers: symvers.as_deref(),
        symvers_set: checks.symvers.is_some(),
        firmware_dir: Some(&firmware_dir),
        module_checks: checks.kernel_modules != Some(false),
        hardware: hardware::Sources {
            build: spec.identity.id.as_str(),
            entries: &spec.image.hardware,
            images_dir: &images_dir,
            rootfs: &rootfs,
            assembly_trees: &assembly_trees,
            removed: &removed,
            pi_model: checks.pi_model.as_deref(),
        },
    });
    let mut boot_trees = vec![
        (images_dir.clone(), "images".to_string()),
        (rootfs.clone(), "target".to_string()),
    ];
    boot_trees.extend(labeled_trees);
    uboot::check(spec.identity.id.as_str(), &boot_trees, arch, &mut outcome);

    let errors: Vec<&Finding> = outcome
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .collect();
    let warnings: Vec<&Finding> = outcome
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Warning)
        .collect();
    let status = if errors.is_empty() {
        "passed"
    } else {
        "failed"
    };
    let report = report_json(
        spec,
        status,
        Some((&outcome, &kernel_image, symvers.as_deref())),
        &errors,
        &warnings,
        &buildroot,
    );
    if let Err(error) = write_report(&report_path, &report) {
        return report_failure(operation_id, &error);
    }

    if !errors.is_empty() {
        let named: Vec<&str> = errors
            .iter()
            .take(NAMED_ERRORS)
            .map(|finding| finding.message.as_str())
            .collect();
        let more = errors.len().saturating_sub(NAMED_ERRORS);
        let mut message = format!(
            "image checks failed with {} error(s): {}",
            errors.len(),
            named.join("; ")
        );
        if more > 0 {
            message.push_str(&format!("; and {more} more"));
        }
        message.push_str(&format!(" (report: {})", report_path.display()));
        return failure_with_kind(
            operation_id.clone(),
            "image_check_failed",
            ExecutionErrorKind::ImageCheck,
            message,
        );
    }
    let release = outcome.release.as_deref().unwrap_or("unknown");
    let mut summary = format!(
        "image checks passed: {} kernel modules and {} imports checked against kernel {release}",
        outcome.modules, outcome.imports
    );
    if !spec.image.hardware.is_empty() {
        summary.push_str(&format!(
            "; {} hardware entries checked",
            spec.image.hardware.len()
        ));
    }
    // The warnings reach the run summary through the image result, which
    // names no output: nothing is published for it.
    let image_result = gaia_image_providers::ImageExecutionResult {
        provider_id: format!("image.{}", spec.image.provider_kind().as_str()),
        collect_dir: None,
        archive_path: None,
        emit_report: false,
        reused: false,
        reuse_details: Vec::new(),
        messages: Vec::new(),
        warnings: warnings
            .iter()
            .map(|finding| finding.message.clone())
            .collect(),
        disk_images: Vec::new(),
        notes: Vec::new(),
        state_details: Vec::new(),
    };
    success_from_messages(
        operation_id.clone(),
        Vec::new(),
        summary,
        RollbackDomain::Images,
        vec![report_path],
    )
    .with_image_result(image_result)
}

fn report_failure(operation_id: &OperationId, error: &str) -> OperationExecutionResult {
    failure_with_kind(
        operation_id.clone(),
        "image_check_report_failed",
        ExecutionErrorKind::ImageCheck,
        error.to_string(),
    )
}

/// `build/linux-*/Module.symvers`, the first one in name order.
fn find_symvers(build_dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(build_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("linux-"))
        })
        .map(|dir| dir.join("Module.symvers"))
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The report. Paths are shown relative to the Buildroot output, so the same
/// build in another place writes the same report.
fn report_json(
    spec: &ResolvedBuildSpec,
    status: &str,
    checked: Option<(&kernel_modules::Outcome, &Path, Option<&Path>)>,
    errors: &[&Finding],
    warnings: &[&Finding],
    buildroot: &Path,
) -> serde_json::Value {
    let shown = |path: &Path| {
        path.strip_prefix(buildroot)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let kernel_modules = match checked {
        Some((outcome, kernel_image, symvers)) => serde_json::json!({
            "release": outcome.release,
            "kernel_image": shown(kernel_image),
            "modules_dir": outcome.modules_dir.as_deref().map(shown),
            "symvers": symvers.map(shown),
            "modules": outcome.modules,
            "imports": outcome.imports,
            "symvers_checked": outcome.symvers_checked,
        }),
        None => serde_json::Value::Null,
    };
    let finding = |finding: &&Finding| {
        serde_json::json!({
            "code": finding.code,
            "module": finding.module,
            "message": finding.message,
        })
    };
    serde_json::json!({
        "schema": REPORT_SCHEMA,
        "build": spec.identity.id.as_str(),
        "status": status,
        "hardware_entries": spec.image.hardware.len(),
        "kernel_modules": kernel_modules,
        "errors": errors.iter().map(finding).collect::<Vec<_>>(),
        "warnings": warnings.iter().map(finding).collect::<Vec<_>>(),
    })
}

fn write_report(path: &Path, report: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|error| format!("cannot encode the image checks report: {error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}
