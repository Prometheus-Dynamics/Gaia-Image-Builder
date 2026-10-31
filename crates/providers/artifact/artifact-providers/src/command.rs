use crate::{
    ArtifactDockerExecution, ArtifactExecutionBackend, ArtifactExecutionContract,
    ArtifactProviderError, ArtifactProviderErrorKind, ProcessCancelCheck, ProcessLogSink,
};
use gaia_process::{
    DockerRunSpec, ProcessOutputRetention, ProcessRunErrorKind, docker_run_command,
    run_command_with_timeout, run_command_with_timeout_and_retention,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

pub fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, ArtifactProviderError> {
    command_output_with_timeout_and_sink(command, timeout, label, None, None)
        .map(|result| result.output)
}

pub(crate) fn command_output_with_timeout_and_sink(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<gaia_process::ProcessRunResult, ArtifactProviderError> {
    run_command_with_timeout(command, timeout, label, log_sink, cancel_check).map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ArtifactProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ArtifactProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ArtifactProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ArtifactProviderErrorKind::RuntimeState,
        };
        ArtifactProviderError::new(kind, error.message)
    })
}

pub(crate) fn command_output_with_timeout_sink_and_retention(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<gaia_process::ProcessRunResult, ArtifactProviderError> {
    run_command_with_timeout_and_retention(
        command,
        timeout,
        label,
        retention,
        log_sink,
        cancel_check,
    )
    .map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ArtifactProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ArtifactProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ArtifactProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ArtifactProviderErrorKind::RuntimeState,
        };
        ArtifactProviderError::new(kind, error.message)
    })
}

pub fn command_for_execution(
    command: &Command,
    contract: &ArtifactExecutionContract,
) -> Result<Command, ArtifactProviderError> {
    let mut budgeted;
    let command = match contract.job_budget {
        Some(jobs) => {
            budgeted = gaia_process::clone_command(command);
            apply_job_budget(&mut budgeted, jobs);
            &budgeted
        }
        None => command,
    };
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => Ok(gaia_process::clone_command(command)),
        ArtifactExecutionBackend::Docker(docker) => docker_command(command, contract, docker),
    }
}

/// Environment variables that cap build tool parallelism.
const JOB_BUDGET_ENV: [&str; 3] = [
    "CARGO_BUILD_JOBS",
    "MAKEFLAGS",
    "CMAKE_BUILD_PARALLEL_LEVEL",
];

/// Caps the tool parallelism of `command` at `jobs` through the usual
/// environment variables. A variable the command or the calling environment
/// already sets is left alone, so users keep control.
pub(crate) fn apply_job_budget(command: &mut Command, jobs: usize) {
    let jobs = jobs.max(1);
    for key in JOB_BUDGET_ENV {
        let set_on_command = command.get_envs().any(|(name, _)| name == key);
        if set_on_command || std::env::var_os(key).is_some() {
            continue;
        }
        let value = if key == "MAKEFLAGS" {
            format!("-j{jobs}")
        } else {
            jobs.to_string()
        };
        command.env(key, value);
    }
}

/// Stderr text that marks a failure as network trouble, which another attempt
/// can fix.
const TRANSIENT_FAILURE_MARKERS: [&str; 12] = [
    "failed to download",
    "spurious network error",
    "failed to fetch",
    "network failure",
    "connection reset",
    "connection refused",
    "connection timed out",
    "timed out",
    "could not resolve host",
    "temporary failure in name resolution",
    "failed to connect",
    "too many requests",
];

/// Whether a failed command looks transient, so another attempt may succeed:
/// it was killed by a signal, or its stderr reports network trouble. Anything
/// else, a compile error above all, is deterministic: the same inputs fail the
/// same way, so it is not retried.
pub(crate) fn failure_is_transient(killed_by_signal: bool, stderr: &str) -> bool {
    if killed_by_signal {
        return true;
    }
    let lower = stderr.to_ascii_lowercase();
    TRANSIENT_FAILURE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

pub fn run_command_with_retries(
    command: &Command,
    contract: &ArtifactExecutionContract,
    label: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ArtifactProviderError> {
    let attempts = contract.retry_attempts.max(1);
    let timeout = Duration::from_secs(contract.timeout_seconds.max(1));
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        tracing::debug!(
            command_label = label,
            provider_domain = "artifact",
            provider = %contract.provider.as_str(),
            output = %contract.output.path.as_str(),
            attempt,
            attempts,
            timeout_seconds = timeout.as_secs(),
            backend = contract.execution_backend.name(),
            docker_image = docker_image(contract),
            "running artifact provider command"
        );
        let mut exec_command = command_for_execution(command, contract)?;
        let output = command_output_with_timeout_sink_and_retention(
            &mut exec_command,
            timeout,
            label,
            process_output_retention(contract),
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.output.status.success() {
            tracing::debug!(
                command_label = label,
                provider_domain = "artifact",
                provider = %contract.provider.as_str(),
                output = %contract.output.path.as_str(),
                attempt,
                attempts,
                backend = contract.execution_backend.name(),
                "artifact provider command succeeded"
            );
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.output.stderr);
        last_error = format!(
            "{label} failed on attempt {attempt}/{attempts}: {}",
            stderr.trim()
        );
        if !failure_is_transient(output.output.status.code().is_none(), &stderr) {
            tracing::warn!(
                command_label = label,
                provider_domain = "artifact",
                provider = %contract.provider.as_str(),
                output = %contract.output.path.as_str(),
                attempt,
                attempts,
                backend = contract.execution_backend.name(),
                "artifact provider command failed deterministically; not retried"
            );
            return Err(ArtifactProviderError::new(
                ArtifactProviderErrorKind::BackendCommand,
                last_error,
            ));
        }
        if attempt < attempts {
            let retry_backoff = crate::retry_backoff_duration(
                contract.retry_backoff_strategy,
                contract.retry_backoff_ms,
                attempt,
            );
            tracing::warn!(
                command_label = label,
                provider_domain = "artifact",
                provider = %contract.provider.as_str(),
                output = %contract.output.path.as_str(),
                attempt,
                attempts,
                backend = contract.execution_backend.name(),
                backoff_ms = retry_backoff.as_millis(),
                "artifact provider command failed; retrying"
            );
            if !crate::sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                tracing::warn!(
                    command_label = label,
                    provider_domain = "artifact",
                    provider = %contract.provider.as_str(),
                    output = %contract.output.path.as_str(),
                    attempt,
                    attempts,
                    backend = contract.execution_backend.name(),
                    "artifact provider retry backoff cancelled"
                );
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::Cancelled,
                    format!("{label} cancelled during retry backoff"),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = label,
        provider_domain = "artifact",
        provider = %contract.provider.as_str(),
        output = %contract.output.path.as_str(),
        attempts,
        backend = contract.execution_backend.name(),
        "artifact provider command exhausted retries"
    );
    Err(ArtifactProviderError::new(
        ArtifactProviderErrorKind::BackendCommand,
        last_error,
    ))
}

/// Builds the Dockerfile-backed execution image when its content-addressed
/// tag is missing, and records the image id on the contract. A no-op for
/// host execution and plain `image` references.
pub fn ensure_docker_execution_image(
    contract: &mut ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    ensure_docker_execution_image_with(
        std::ffi::OsStr::new("docker"),
        contract,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn ensure_docker_execution_image_with(
    docker_program: &std::ffi::OsStr,
    contract: &mut ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    let timeout = Duration::from_secs(contract.timeout_seconds.max(1));
    let retention = process_output_retention(contract);
    let ArtifactExecutionBackend::Docker(docker) = &mut contract.execution_backend else {
        return Ok(Vec::new());
    };
    let Some(build) = &mut docker.build else {
        return Ok(Vec::new());
    };
    if let Some(error) = &build.hash_error {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            error.clone(),
        ));
    }
    let tag = docker.image.clone();
    let image_id = |label: &str| -> Result<Option<String>, ArtifactProviderError> {
        let mut inspect = gaia_process::docker_image_id_command(docker_program, &tag);
        let output = command_output_with_timeout_sink_and_retention(
            &mut inspect,
            Duration::from_secs(60),
            label,
            retention,
            None,
            cancel_check.clone(),
        )?
        .output;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((output.status.success() && !id.is_empty()).then_some(id))
    };
    if let Some(id) = image_id("inspect docker execution image")? {
        build.image_id = Some(id);
        return Ok(vec![format!(
            "docker execution image '{tag}' is up to date"
        )]);
    }
    let mut command = gaia_process::docker_image_build_command(
        docker_program,
        Path::new(&build.dockerfile),
        Path::new(build.context.as_deref().unwrap_or(".")),
        &tag,
    );
    tracing::info!(image = %tag, dockerfile = %build.dockerfile, "building docker execution image");
    let output = command_output_with_timeout_sink_and_retention(
        &mut command,
        timeout,
        "build docker execution image",
        retention,
        log_sink,
        cancel_check.clone(),
    )?
    .output;
    if !output.status.success() {
        return Err(ArtifactProviderError::backend_command(format!(
            "docker build of '{}' for image '{tag}' failed: {}",
            build.dockerfile,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let id = image_id("inspect built docker execution image")?.ok_or_else(|| {
        ArtifactProviderError::backend_command(format!(
            "docker build reported success but image '{tag}' is missing"
        ))
    })?;
    build.image_id = Some(id);
    Ok(vec![format!(
        "built docker execution image '{tag}' from '{}'",
        build.dockerfile
    )])
}

fn docker_image(contract: &ArtifactExecutionContract) -> Option<&str> {
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => None,
        ArtifactExecutionBackend::Docker(docker) => Some(docker.image.as_str()),
    }
}

fn process_output_retention(contract: &ArtifactExecutionContract) -> ProcessOutputRetention {
    ProcessOutputRetention {
        stdout_bytes: contract.output_retention.stdout_bytes,
        stderr_bytes: contract.output_retention.stderr_bytes,
        stdout_lines: contract.output_retention.stdout_lines,
        stderr_lines: contract.output_retention.stderr_lines,
    }
}

fn docker_command(
    command: &Command,
    contract: &ArtifactExecutionContract,
    docker: &ArtifactDockerExecution,
) -> Result<Command, ArtifactProviderError> {
    if docker.image.trim().is_empty() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            "docker execution requires a non-empty image",
        ));
    }
    let workspace_root = contract.workspace_root.as_deref().ok_or_else(|| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            "docker execution requires a resolved workspace root",
        )
    })?;
    let (docker_home, docker_cache) = docker_dirs(
        Path::new(workspace_root),
        gaia_spec::user_cache_root().as_deref(),
    );
    fs::create_dir_all(&docker_home).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create docker home dir '{}': {error}",
                docker_home.display()
            ),
        )
    })?;
    fs::create_dir_all(&docker_cache).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create docker cache dir '{}': {error}",
                docker_cache.display()
            ),
        )
    })?;
    let mut spec = DockerRunSpec::workspace_mount(
        docker.image.clone(),
        PathBuf::from(workspace_root),
        command,
    )
    .with_extra_env("HOME", &docker_home)
    .with_extra_env("XDG_CACHE_HOME", &docker_cache)
    .with_pull_never(contract.offline);
    for dir in [&docker_home, &docker_cache] {
        if !dir.starts_with(workspace_root) {
            spec.mounts.push(dir.clone());
        }
    }
    // A cross build's SDK is read at its own host paths (the linker, the
    // sysroot): mount its root and sysroot where they are. Only arguments are
    // scanned for paths, so an environment variable's path is not mounted by
    // itself.
    if let Some(toolchain) = &contract.toolchain {
        spec.mounts.push(toolchain.root.clone());
        spec.mounts.push(toolchain.sysroot.clone());
    }
    // The build runs in its source tree, which lies outside the workspace
    // when the build dir (where sources are materialized) or a path source
    // does: mount it where it is.
    if !spec.current_dir.starts_with(&spec.workspace_root) {
        spec.mounts.push(spec.current_dir.clone());
    }
    // Containers run with --rm, so tool caches inside the image are lost after
    // every build. Keep them in the workspace cache unless the artifact sets
    // its own location.
    for (key, cache_dir) in [("CARGO_HOME", "cargo"), ("SCCACHE_DIR", "sccache")] {
        let already_set = command
            .get_envs()
            .any(|(name, value)| name == key && value.is_some());
        if !already_set {
            spec = spec.with_extra_env(key, docker_cache.join(cache_dir));
        }
    }
    docker_run_command(command, &spec).map_err(|error| {
        ArtifactProviderError::new(ArtifactProviderErrorKind::PolicyBlocked, error.to_string())
    })
}

/// The `HOME` and tool cache (`CARGO_HOME`, `SCCACHE_DIR`,
/// `XDG_CACHE_HOME`) of a Docker build: under the user cache root, so
/// nothing is written into the workspace and every workspace shares the
/// downloaded crates and compiler cache. `HOME` stays per workspace
/// (`docker-home/<key of the workspace root>`). Without a user cache root,
/// both are in the workspace's `.gaia`.
pub fn docker_dirs(workspace_root: &Path, user_cache: Option<&Path>) -> (PathBuf, PathBuf) {
    match user_cache {
        Some(cache) => {
            let mut key = gaia_spec::Fingerprint::new("docker-home");
            key.str(&workspace_root.display().to_string());
            (
                cache.join("docker-home").join(key.finish().short_hex()),
                cache.join("docker-cache"),
            )
        }
        None => (
            workspace_root.join(".gaia/docker-home"),
            workspace_root.join(".gaia/docker-cache"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArtifactToolchainContract;
    use crate::contract::{ArtifactDockerExecution, ArtifactExecutionBackend};
    use gaia_spec::{
        ArtifactDefinition, ArtifactOutputSpec, ArtifactSpec, ArtifactVariantSpec,
        OutputRetentionPolicySpec, RustArtifactSpec,
    };
    use gaia_testkit::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn contract(attempts: u32) -> ArtifactExecutionContract {
        let artifact = ArtifactSpec::new(
            "retry-artifact",
            ArtifactDefinition::Rust(RustArtifactSpec {
                package: "retry".into(),
                target_name: None,
                variant: ArtifactVariantSpec::File,
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
                build_group: None,
                group_packages: Vec::new(),
                cargo_git_sources: Vec::new(),
                build_env: Vec::new(),
                cpu: None,
            }),
            None,
            ArtifactOutputSpec {
                path: "retry-artifact.bin".into(),
            },
        );
        let mut policy = ArtifactExecutionContract::default_command_policy();
        policy.retry_attempts = attempts;
        ArtifactExecutionContract::from_spec(
            &artifact,
            None,
            false,
            policy,
            OutputRetentionPolicySpec::default(),
        )
    }

    #[test]
    fn docker_mounts_the_sdk_root_and_sysroot_of_a_cross_build() {
        let scratch = TempDir::new("gaia-docker-sdk-mount");
        let workspace = scratch.join("ws");
        let root = scratch.join("sdk");
        let sysroot = root.join("aarch64-buildroot-linux-gnu/sysroot");
        fs::create_dir_all(&sysroot).expect("sdk sysroot");
        fs::create_dir_all(&workspace).expect("workspace");
        let mut contract = contract(1);
        contract.workspace_root = Some(workspace.display().to_string());
        contract.execution_backend = ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
            image: "builder:latest".into(),
            build: None,
        });
        contract.toolchain = Some(ArtifactToolchainContract {
            id: "sdk".into(),
            root: root.clone(),
            sysroot: sysroot.clone(),
            prefix: "aarch64-buildroot-linux-gnu-".into(),
            triple: "aarch64-buildroot-linux-gnu".into(),
            rust_target: Some("aarch64-unknown-linux-gnu".into()),
            page_size: None,
            relocate: false,
        });
        // The compiler is named by its path, so the command carries the SDK in
        // an environment variable as the Rust provider sets it.
        let mut command = Command::new("cargo");
        command.env(
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
            root.join("bin/aarch64-buildroot-linux-gnu-gcc"),
        );
        let wrapped = command_for_execution(&command, &contract).expect("docker command");
        let args: Vec<String> = wrapped
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for path in [&root, &sysroot] {
            let mount = format!("{0}:{0}", path.display());
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "-v" && pair[1] == mount),
                "{mount} is mounted: {args:?}"
            );
        }
    }

    #[test]
    fn docker_mounts_a_source_tree_outside_the_workspace() {
        let scratch = TempDir::new("gaia-docker-source-mount");
        let workspace = scratch.join("ws");
        let source = scratch.join("fast-disk/build/sources/atlas");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&source).expect("source");
        let mut contract = contract(1);
        contract.workspace_root = Some(workspace.display().to_string());
        contract.execution_backend = ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
            image: "builder:latest".into(),
            build: None,
        });
        let mut command = Command::new("cargo");
        command.current_dir(&source);
        let args: Vec<String> = command_for_execution(&command, &contract)
            .expect("docker command")
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mount = format!("{0}:{0}", source.display());
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-v" && pair[1] == mount),
            "{mount} is mounted: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-w" && pair[1] == source.display().to_string()),
            "{args:?}"
        );
    }

    /// A real container reads its source tree and writes the build dir when
    /// both lie outside the workspace, as with `--set workspace.build_dir` on
    /// another disk. Skipped without a docker daemon or `debian:sid`.
    #[test]
    fn a_container_builds_from_a_build_dir_outside_the_workspace() {
        const IMAGE: &str = "debian:sid";
        let has_image = Command::new("docker")
            .args(["image", "inspect", IMAGE])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !has_image {
            eprintln!("skipped: no docker daemon or no {IMAGE} image");
            return;
        }
        let scratch = TempDir::new("gaia-docker-outside-build-dir");
        let workspace = scratch.join("ws");
        let build_dir = scratch.join("fast-disk/build");
        let source = build_dir.join("sources/atlas");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("Cargo.toml"), "[package]\n").expect("manifest");
        // As `gaia run` does for a build dir outside the workspace root.
        gaia_process::register_docker_mount(&build_dir);
        let mut contract = contract(1);
        contract.workspace_root = Some(workspace.display().to_string());
        contract.execution_backend = ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
            image: IMAGE.into(),
            build: None,
        });
        let output = build_dir.join("artifacts/out.txt");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "test -f Cargo.toml && mkdir -p {0} && echo built > {1}",
                output.parent().expect("parent").display(),
                output.display()
            ))
            .current_dir(&source);
        let status = command_for_execution(&command, &contract)
            .expect("docker command")
            .status()
            .expect("docker runs");
        assert!(status.success(), "the container sees the source tree");
        assert_eq!(fs::read_to_string(&output).expect("written"), "built\n");
    }

    #[test]
    fn docker_home_and_caches_live_in_the_user_cache_outside_the_workspace() {
        let (home, cache) = docker_dirs(Path::new("/repo"), Some(Path::new("/fast/gaia")));
        assert!(
            home.starts_with("/fast/gaia/docker-home"),
            "{}",
            home.display()
        );
        assert_eq!(cache, Path::new("/fast/gaia/docker-cache"));
        let (other, _) = docker_dirs(Path::new("/other"), Some(Path::new("/fast/gaia")));
        assert_ne!(home, other, "HOME stays per workspace");
        assert_eq!(
            docker_dirs(Path::new("/repo"), None),
            (
                PathBuf::from("/repo/.gaia/docker-home"),
                PathBuf::from("/repo/.gaia/docker-cache")
            )
        );
    }

    /// A fake tool that appends one line to `calls` per run, then runs `body`.
    fn fake_tool(dir: &Path, body: &str) -> Command {
        let script = dir.join("fake-cargo");
        let calls = dir.join("calls");
        fs::write(
            &script,
            format!("#!/bin/sh\necho call >> '{}'\n{body}\n", calls.display()),
        )
        .expect("write fake tool");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
        Command::new(&script)
    }

    fn calls(dir: &Path) -> usize {
        fs::read_to_string(dir.join("calls"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    #[test]
    fn a_compile_error_is_not_retried() {
        let scratch = TempDir::new("gaia-retry-compile-error");
        let command = fake_tool(
            scratch.path(),
            "echo 'error[E0425]: cannot find value `x`' >&2\necho 'error: could not compile `retry` (lib) due to 1 previous error' >&2\nexit 101",
        );
        let error = run_command_with_retries(&command, &contract(3), "cargo build", None, None)
            .expect_err("a compile error fails");
        assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
        assert!(
            error.message.contains("could not compile"),
            "{}",
            error.message
        );
        assert_eq!(calls(scratch.path()), 1, "one attempt, not retry_attempts");
    }

    #[test]
    fn a_network_failure_is_retried() {
        let scratch = TempDir::new("gaia-retry-network");
        let command = fake_tool(
            scratch.path(),
            "echo 'error: failed to download `serde v1.0.0`' >&2\necho 'spurious network error' >&2\nexit 101",
        );
        let error = run_command_with_retries(&command, &contract(3), "cargo build", None, None)
            .expect_err("the network keeps failing");
        assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
        assert_eq!(calls(scratch.path()), 3);
    }

    #[test]
    fn a_process_killed_by_a_signal_is_retried() {
        let scratch = TempDir::new("gaia-retry-signal");
        let command = fake_tool(scratch.path(), "kill -9 $$");
        run_command_with_retries(&command, &contract(2), "cargo build", None, None)
            .expect_err("the process keeps dying");
        assert_eq!(calls(scratch.path()), 2);
    }

    #[test]
    fn a_success_after_a_transient_failure_is_accepted() {
        let scratch = TempDir::new("gaia-retry-recovers");
        // Fails with a network error on the first call, succeeds on the second.
        let body = format!(
            "if [ \"$(wc -l < '{0}')\" -lt 2 ]; then echo 'failed to download `x`' >&2; exit 101; fi\nexit 0",
            scratch.path().join("calls").display()
        );
        let command = fake_tool(scratch.path(), &body);
        run_command_with_retries(&command, &contract(3), "cargo build", None, None)
            .expect("second attempt succeeds");
        assert_eq!(calls(scratch.path()), 2);
    }

    #[test]
    fn transient_markers_and_signals_decide() {
        assert!(failure_is_transient(true, ""));
        assert!(failure_is_transient(
            false,
            "warning: spurious network error"
        ));
        assert!(!failure_is_transient(
            false,
            "error[E0308]: mismatched types"
        ));
        assert!(!failure_is_transient(
            false,
            "error: could not compile `app`"
        ));
    }
}
