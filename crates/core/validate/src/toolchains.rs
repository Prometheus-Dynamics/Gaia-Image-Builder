//! `[[toolchains]]`, the artifacts that build with one, and the device
//! sysroot of an `[image.assembly.packages.device]`.

use std::collections::HashSet;

use gaia_spec::{
    ArtifactDefinition, ResolvedBuildSpec, ToolchainSpec, parse_glibc_version,
    resolve_workspace_path,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

/// Validates every `[[toolchains]]` entry.
pub(crate) fn validate_toolchains(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut ids = HashSet::new();
    for toolchain in &spec.toolchains {
        let label = format!("toolchain '{}'", toolchain.id);
        let location = Some(format!("toolchain:{}", toolchain.id));
        if toolchain.id.trim().is_empty() {
            diagnostics.push(error(
                "toolchain_id_empty",
                "toolchain id cannot be empty".into(),
                Some("toolchains".into()),
            ));
        } else if !ids.insert(toolchain.id.clone()) {
            diagnostics.push(error(
                "toolchain_duplicate",
                format!("{label} is declared more than once"),
                location.clone(),
            ));
        }

        match (&toolchain.root_source, &toolchain.root_path) {
            (None, None) => diagnostics.push(error(
                "toolchain_root_missing",
                format!("{label} needs a root: set `source` (a source's tree) or `path`"),
                location.clone(),
            )),
            (Some(_), Some(_)) => diagnostics.push(error(
                "toolchain_root_conflict",
                format!("{label} sets both `source` and `path`; choose one"),
                location.clone(),
            )),
            (Some(source), None) => {
                if !spec.sources.iter().any(|candidate| candidate.id == *source) {
                    diagnostics.push(error(
                        "toolchain_source_undefined",
                        format!("{label} names undefined source '{}'", source.as_str()),
                        location.clone(),
                    ));
                }
            }
            (None, Some(path)) => {
                if let Err(message) = resolve_workspace_path(&spec.workspace, path) {
                    diagnostics.push(error(
                        "toolchain_path_invalid",
                        format!("{label} has an invalid path: {message}"),
                        location.clone(),
                    ));
                }
            }
        }

        match (&toolchain.sysroot_path, &toolchain.sysroot_source) {
            (None, None) => diagnostics.push(error(
                "toolchain_sysroot_missing",
                format!("{label} needs a sysroot: set `sysroot` (a path under the root) or `sysroot_source`"),
                location.clone(),
            )),
            (Some(_), Some(_)) => diagnostics.push(error(
                "toolchain_sysroot_conflict",
                format!("{label} sets both `sysroot` and `sysroot_source`; choose one"),
                location.clone(),
            )),
            (Some(path), None) if path.trim().is_empty() => diagnostics.push(error(
                "toolchain_sysroot_missing",
                format!("{label} has an empty sysroot path"),
                location.clone(),
            )),
            (None, Some(source)) => {
                if !spec.sources.iter().any(|candidate| candidate.id == *source) {
                    diagnostics.push(error(
                        "toolchain_source_undefined",
                        format!("{label} names undefined sysroot source '{}'", source.as_str()),
                        location.clone(),
                    ));
                }
            }
            (Some(_), None) => {}
        }

        if toolchain.triple.trim().is_empty() {
            diagnostics.push(error(
                "toolchain_triple_missing",
                format!("{label} needs a triple, such as aarch64-buildroot-linux-gnu"),
                location.clone(),
            ));
        }
        if let Some(prefix) = &toolchain.prefix
            && prefix.trim().is_empty()
        {
            diagnostics.push(error(
                "toolchain_prefix_empty",
                format!("{label} has an empty compiler prefix"),
                location.clone(),
            ));
        }
        if let Some(page) = toolchain.page_size
            && !is_page_size(page)
        {
            diagnostics.push(error(
                "toolchain_page_size_invalid",
                format!("{label} page_size {page} must be a power of two of at least 4096 bytes"),
                location.clone(),
            ));
        }
        if let Some(target) = &toolchain.rust_target
            && target.trim().is_empty()
        {
            diagnostics.push(error(
                "toolchain_rust_target_empty",
                format!("{label} has an empty rust_target"),
                location,
            ));
        }
    }
}

/// Artifacts that name a toolchain: only rust artifacts take one, the id must
/// be declared, and the artifact needs a Rust target (its own `target`, or the
/// toolchain's `rust_target`).
pub(crate) fn validate_artifact_toolchains(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for artifact in &spec.artifacts {
        let Some(id) = &artifact.toolchain else {
            continue;
        };
        let location = Some(format!("artifact:{}", artifact.id.as_str()));
        if !matches!(artifact.definition, ArtifactDefinition::Rust(_)) {
            diagnostics.push(error(
                "artifact_toolchain_non_rust",
                format!(
                    "artifact '{}' sets toolchain '{id}', but only rust artifacts take a toolchain",
                    artifact.id.as_str()
                ),
                location,
            ));
            continue;
        }
        let Some(toolchain) = spec.toolchains.iter().find(|toolchain| toolchain.id == *id) else {
            diagnostics.push(error(
                "artifact_toolchain_unknown",
                format!(
                    "artifact '{}' names unknown toolchain '{id}'",
                    artifact.id.as_str()
                ),
                location,
            ));
            continue;
        };
        if artifact.target.is_none() && toolchain.rust_target.is_none() {
            diagnostics.push(error(
                "artifact_toolchain_target_missing",
                format!(
                    "artifact '{}' builds with toolchain '{id}', which names no rust_target, and the artifact sets no target",
                    artifact.id.as_str()
                ),
                location,
            ));
        }
    }
}

/// The sysroot of an `[image.assembly.packages.device]`: exactly one of
/// `toolchain` (a declared id) and `sysroot` (a workspace path), and the
/// limits are well formed.
pub(crate) fn validate_device_sysroot(
    spec: &ResolvedBuildSpec,
    toolchain_ids: &HashSet<String>,
    package: &str,
    device: &gaia_spec::AssemblyPackageDeviceSpec,
    location: Option<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    match (&device.toolchain, &device.sysroot) {
        (None, None) => diagnostics.push(error(
            "package_device_sysroot_missing",
            format!("assembly package '{package}' device needs `toolchain` or `sysroot`"),
            location.clone(),
        )),
        (Some(_), Some(_)) => diagnostics.push(error(
            "package_device_sysroot_conflict",
            format!("assembly package '{package}' device sets both `toolchain` and `sysroot`; choose one"),
            location.clone(),
        )),
        (Some(id), None) => {
            if !toolchain_ids.contains(id) {
                diagnostics.push(error(
                    "package_device_toolchain_unknown",
                    format!("assembly package '{package}' device names unknown toolchain '{id}'"),
                    location.clone(),
                ));
            }
        }
        (None, Some(path)) => {
            if let Err(message) = resolve_workspace_path(&spec.workspace, path) {
                diagnostics.push(error(
                    "package_device_sysroot_invalid",
                    format!("assembly package '{package}' device sysroot is invalid: {message}"),
                    location.clone(),
                ));
            }
        }
    }
    if let Some(max) = &device.max_glibc
        && parse_glibc_version(max).is_none()
    {
        diagnostics.push(error(
            "package_device_max_glibc_invalid",
            format!("assembly package '{package}' device max_glibc '{max}' is not a GLIBC version such as \"2.17\""),
            location.clone(),
        ));
    }
    if let Some(page) = device.min_page_size
        && (page == 0 || !page.is_power_of_two())
    {
        diagnostics.push(error(
            "package_device_min_page_size_invalid",
            format!(
                "assembly package '{package}' device min_page_size {page} must be a power of two"
            ),
            location,
        ));
    }
}

/// A page size the device checks and the linker accept: a power of two of at
/// least 4096 bytes.
pub(crate) fn is_page_size(bytes: u64) -> bool {
    bytes >= 4096 && bytes.is_power_of_two()
}

/// The declared toolchain ids, for the device checks.
pub(crate) fn toolchain_ids(toolchains: &[ToolchainSpec]) -> HashSet<String> {
    toolchains
        .iter()
        .map(|toolchain| toolchain.id.clone())
        .collect()
}
