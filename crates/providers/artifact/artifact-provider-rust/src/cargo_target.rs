//! Where a cargo build writes: the source's own target dir, or one shared by
//! the sources built with the same toolchain, target, profile and backend.

use super::{cargo_profile_dir, host_toolchain_identity};
use gaia_artifact_providers::{
    ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactProviderError,
    ArtifactProviderErrorKind,
};
use gaia_process::register_docker_mount;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A source's own cargo target dir, `<source>/.gaia/cargo-target`.
pub(crate) fn cargo_target_dir(source_dir: &str) -> PathBuf {
    PathBuf::from(source_dir).join(".gaia").join("cargo-target")
}

/// The target dir of builds that record links (`link_record`): every crate
/// there is built for the wrapper linker, which cargo would otherwise
/// rebuild everything for each time a build without it ran in between.
const RECORDING_SUFFIX: &str = "-linked";

/// Directory inside a shared target dir that records which directory each
/// local package was first built from.
pub(crate) const SHARED_OWNERS_DIR: &str = "gaia-owners";

/// Where one cargo invocation writes.
pub(crate) struct CargoTarget {
    pub(crate) dir: PathBuf,
    /// Whether `dir` is shared with other sources: it is created up front and
    /// registered as a docker mount.
    pub(crate) shared: bool,
    /// Logged before the build: the shared dir in use, or why the source
    /// keeps its own dir although a shared one was asked for.
    pub(crate) note: Option<String>,
}

pub(crate) fn cargo_target_for(
    source_dir: &str,
    contract: &ArtifactExecutionContract,
    records_links: bool,
) -> CargoTarget {
    cargo_target_for_in(
        gaia_spec::user_cache_root().as_deref(),
        source_dir,
        contract,
        records_links,
    )
}

/// Chooses the target dir of a build of `source_dir`. With
/// `shared_target_dir` it is `<cache_root>/cargo-target/<key>`, shared by
/// every source built with the same toolchain, target, profile and backend.
/// Cargo identifies a package by name and version, so a source whose local
/// packages an earlier source in that dir built from another directory keeps
/// its own dir instead of reusing those outputs.
pub(crate) fn cargo_target_for_in(
    cache_root: Option<&Path>,
    source_dir: &str,
    contract: &ArtifactExecutionContract,
    records_links: bool,
) -> CargoTarget {
    let suffix = if records_links { RECORDING_SUFFIX } else { "" };
    let mut own = cargo_target_dir(source_dir).into_os_string();
    own.push(suffix);
    let own = PathBuf::from(own);
    if !contract.rust_shared_target_dir {
        return CargoTarget {
            dir: own,
            shared: false,
            note: None,
        };
    }
    let Some(root) = cache_root else {
        return CargoTarget {
            dir: own,
            shared: false,
            note: Some(
                "shared cargo target dir needs a user cache directory (GAIA_CACHE_DIR, XDG_CACHE_HOME or HOME); using the source's own"
                    .into(),
            ),
        };
    };
    let dir = root
        .join("cargo-target")
        .join(format!("{}{suffix}", shared_target_key(contract)));
    match claim_local_packages(source_dir, &dir) {
        Ok(None) => CargoTarget {
            note: Some(format!(
                "cargo target dir shared with other sources: {}",
                dir.display()
            )),
            dir,
            shared: true,
        },
        Ok(Some(conflict)) => CargoTarget {
            dir: own,
            shared: false,
            note: Some(format!(
                "{conflict}; using the source's own cargo target dir"
            )),
        },
        Err(reason) => CargoTarget {
            dir: own,
            shared: false,
            note: Some(format!("shared cargo target dir not used: {reason}")),
        },
    }
}

/// Name of a shared target dir: a hash of the toolchain (the host rustc and
/// cargo, or the docker image), target triple, profile and execution backend.
/// Cargo already keys its outputs by rustc and target, so the key only keeps
/// unrelated builds in separate dirs.
pub(crate) fn shared_target_key(contract: &ArtifactExecutionContract) -> String {
    let (backend, toolchain) = match &contract.execution_backend {
        ArtifactExecutionBackend::Host => ("host", host_toolchain_identity()),
        ArtifactExecutionBackend::Docker(docker) => ("docker", format!("image={}", docker.image)),
    };
    let identity = format!(
        "backend={backend}\ntoolchain={toolchain}\ntarget={}\nprofile={}\n",
        contract.artifact_target.as_deref().unwrap_or("host"),
        cargo_profile_dir(contract),
    );
    let digest = Sha256::digest(identity.as_bytes());
    digest
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Claims the local packages of `source_dir` (workspace and path packages;
/// registry and git packages are immutable and shared freely) for
/// `shared_dir`. Returns the first package claimed by another directory, or
/// `Err` when the packages cannot be listed or claimed.
pub(crate) fn claim_local_packages(
    source_dir: &str,
    shared_dir: &Path,
) -> Result<Option<String>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(source_dir)
        .output()
        .map_err(|error| format!("cargo metadata did not start: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "cargo metadata failed: {}",
            stderr.lines().next().unwrap_or("no output")
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata output is not JSON: {error}"))?;
    let owners = shared_dir.join(SHARED_OWNERS_DIR);
    fs::create_dir_all(&owners)
        .map_err(|error| format!("failed to create '{}': {error}", owners.display()))?;
    for package in metadata["packages"].as_array().into_iter().flatten() {
        if !package["source"].is_null() {
            continue;
        }
        let (Some(name), Some(version), Some(manifest)) = (
            package["name"].as_str(),
            package["version"].as_str(),
            package["manifest_path"].as_str(),
        ) else {
            continue;
        };
        let Some(dir) = Path::new(manifest).parent() else {
            continue;
        };
        let dir = fs::canonicalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string();
        let claim = owners.join(format!("{name}-{version}"));
        match OpenOptions::new().write(true).create_new(true).open(&claim) {
            Ok(mut file) => file
                .write_all(dir.as_bytes())
                .map_err(|error| format!("failed to write '{}': {error}", claim.display()))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let owner = fs::read_to_string(&claim)
                    .map_err(|error| format!("failed to read '{}': {error}", claim.display()))?;
                if owner != dir {
                    return Ok(Some(format!(
                        "package '{name}' {version} is built from '{owner}' in the shared target dir"
                    )));
                }
            }
            Err(error) => {
                return Err(format!("failed to claim '{}': {error}", claim.display()));
            }
        }
    }
    Ok(None)
}

/// Creates a shared target dir and registers it as a docker mount, so
/// containers see it at its own path. Per-source dirs need nothing.
pub(crate) fn prepare_cargo_target(target: &CargoTarget) -> Result<(), ArtifactProviderError> {
    if !target.shared {
        return Ok(());
    }
    fs::create_dir_all(&target.dir).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create shared cargo target dir '{}': {error}",
                target.dir.display()
            ),
        )
    })?;
    register_docker_mount(&target.dir);
    Ok(())
}
