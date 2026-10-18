//! The tools a rust build runs with: the host's and the docker image's
//! versions (recorded in each artifact's state), the host's toolchain identity
//! (the shared target dir key), sccache, and the strip tool for collected
//! outputs. Host probes run once per process; a docker probe once per image.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gaia_artifact_providers::{
    ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactProviderError,
    ArtifactProviderErrorKind, ProcessLogLine, ProcessLogSink, ProcessLogStream,
    command_for_execution, command_output_with_timeout, command_version_line,
};

pub(crate) fn host_toolchain_identity() -> String {
    static IDENTITY: OnceLock<String> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let rustc = Command::new("rustc")
                .arg("-vV")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
                .unwrap_or_else(|| "rustc unavailable".into());
            format!("{rustc}{}", host_version_line("cargo", &["--version"]))
        })
        .clone()
}

/// The version line of a tool on the host, probed once per process: every
/// artifact of a build asks for the same tools.
fn host_version_line(program: &str, args: &[&str]) -> String {
    static VERSIONS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let key = format!("{program} {}", args.join(" "));
    if let Some(line) = VERSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return line.clone();
    }
    let line = command_version_line(program, args);
    VERSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, line.clone());
    line
}

/// The toolchain a build of this contract ran with. On the host, the host's
/// tool. In docker, the tools are the image's, so the record names the image
/// (its id once built): the host's versions are not what compiled the output.
pub(crate) fn toolchain_version(
    contract: &ArtifactExecutionContract,
    program: &str,
    args: &[&str],
) -> String {
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => host_version_line(program, args),
        ArtifactExecutionBackend::Docker(docker) => format!(
            "docker:{}",
            docker
                .build
                .as_ref()
                .and_then(|build| build.image_id.as_deref())
                .unwrap_or(docker.image.as_str())
        ),
    }
}

/// Whether `program --version` runs where the build runs (the host, or the
/// docker image). Probed once per backend and program.
fn tool_available(contract: &ArtifactExecutionContract, program: &str) -> bool {
    static PROBES: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());
    let backend = match &contract.execution_backend {
        ArtifactExecutionBackend::Host => "host".to_string(),
        ArtifactExecutionBackend::Docker(docker) => format!(
            "docker:{}",
            docker
                .build
                .as_ref()
                .and_then(|build| build.image_id.as_deref())
                .unwrap_or(docker.image.as_str())
        ),
    };
    let key = format!("{backend} {program}");
    if let Some(available) = PROBES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return *available;
    }
    let mut probe = Command::new(program);
    probe.arg("--version");
    let available = command_for_execution(&probe, contract)
        .ok()
        .and_then(|mut command| {
            command_output_with_timeout(&mut command, Duration::from_secs(60), program).ok()
        })
        .is_some_and(|output| output.status.success());
    PROBES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, available);
    available
}

/// `RUSTC_WRAPPER=sccache` for a build with `[providers.rust] sccache = true`
/// when sccache is available where cargo runs. Empty otherwise; the log says
/// why. On the host the cache lives under the user cache root; in docker the
/// container's cache is the workspace's (see `docker_command`).
pub(crate) fn sccache_env(
    contract: &ArtifactExecutionContract,
    log_sink: Option<&ProcessLogSink>,
) -> Vec<(String, String)> {
    if !contract.rust_sccache {
        return Vec::new();
    }
    if !tool_available(contract, "sccache") {
        if let Some(sink) = log_sink {
            sink(ProcessLogLine {
                stream: ProcessLogStream::Stdout,
                line: "sccache is not available where cargo runs; building without it".into(),
            });
        }
        return Vec::new();
    }
    let mut env = vec![("RUSTC_WRAPPER".to_string(), "sccache".to_string())];
    if matches!(contract.execution_backend, ArtifactExecutionBackend::Host)
        && std::env::var_os("SCCACHE_DIR").is_none()
        && let Some(root) = gaia_spec::user_cache_root()
    {
        env.push((
            "SCCACHE_DIR".to_string(),
            root.join("sccache").display().to_string(),
        ));
    }
    env
}

/// Strips the collected copy of a Rust output with the strip tool of the
/// target: `<triple>-strip`, then `llvm-strip`; without a target triple,
/// `strip`, then `llvm-strip`. Runs on the host, where the copy is.
pub(crate) fn strip_collected_output(
    output_path: &Path,
    contract: &ArtifactExecutionContract,
) -> Result<(), ArtifactProviderError> {
    let candidates = match contract.artifact_target.as_deref() {
        Some(triple) => vec![format!("{triple}-strip"), "llvm-strip".to_string()],
        None => vec!["strip".to_string(), "llvm-strip".to_string()],
    };
    let tool = candidates
        .iter()
        .find(|tool| {
            Command::new(tool)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .ok_or_else(|| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "providers.rust strip is set, but none of {} is on the host's PATH",
                    candidates.join(", ")
                ),
            )
        })?;
    let mut command = Command::new(tool);
    command.arg(output_path);
    let output = command_output_with_timeout(
        &mut command,
        Duration::from_secs(contract.timeout_seconds.max(1)),
        &format!("strip {}", output_path.display()),
    )?;
    if !output.status.success() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::BackendCommand,
            format!(
                "{tool} failed on '{}': {}",
                output_path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}
