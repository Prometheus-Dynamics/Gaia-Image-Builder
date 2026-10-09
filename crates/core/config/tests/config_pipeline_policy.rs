pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{
    DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS, DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_COMMAND_RETRY_ATTEMPTS, DEFAULT_COMMAND_RETRY_BACKOFF_MS,
    DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS, DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS, DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS, DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS, DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS,
    RetryBackoffStrategySpec, RollbackDomain,
};
use std::path::PathBuf;
use support::{default_config_path, write_temp_config};

#[test]
fn unresolved_provider_policy_uses_shared_release_defaults() {
    let path = write_temp_config(
        r#"
build_name = "policy-defaults"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    assert_eq!(
        spec.policy.providers.rust.retry_attempts,
        DEFAULT_COMMAND_RETRY_ATTEMPTS
    );
    assert_eq!(
        spec.policy.providers.rust.retry_backoff_ms,
        DEFAULT_COMMAND_RETRY_BACKOFF_MS
    );
    assert_eq!(
        spec.policy.providers.rust.timeout_seconds,
        DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.git.timeout_seconds,
        DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.archive.timeout_seconds,
        DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.download.timeout_seconds,
        DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.go.timeout_seconds,
        DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.java.timeout_seconds,
        DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.node.timeout_seconds,
        DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.python.timeout_seconds,
        DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.buildroot.timeout_seconds,
        DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS
    );
    assert_eq!(
        spec.policy.providers.starting_point.timeout_seconds,
        DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_config_with_cli_selection_overrides() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            preset: Some("ci".into()),
            env_files: Vec::new(),
            env_overrides: vec![("GAIA_MODE".into(), "ci-env".into())],
            explicit_overrides: vec![
                ("input.target".into(), "rpi5".into()),
                ("build.version".into(), "9.9.9".into()),
                (
                    "policy.failure.rollback_domains".into(),
                    "artifacts,images,stage,checkpoints".into(),
                ),
            ],
            ..Default::default()
        },
    );

    assert_eq!(spec.selection.selected_preset.as_deref(), Some("ci"));
    let requested_build = default_config_path();
    assert_eq!(
        spec.selection.requested_build.as_deref(),
        Some(requested_build.as_str())
    );
    assert!(
        spec.selection
            .env_overrides
            .iter()
            .any(|(key, value)| key == "GAIA_MODE" && value == "ci-env")
    );
    assert_eq!(
        spec.selection.precedence_order,
        vec![
            "ConfigDefaults".to_string(),
            "SelectedPreset".to_string(),
            "EnvFiles".to_string(),
            "InlineEnv".to_string(),
            "ProcessEnv".to_string(),
            "CliEnvOverrides".to_string(),
            "CliSetOverrides".to_string(),
        ]
    );
    assert!(
        spec.selection
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "input.target" && value == "rpi5")
    );
    assert!(
        spec.selection
            .explicit_overrides
            .iter()
            .any(|(key, value)| key == "build.version" && value == "9.9.9")
    );
    assert!(
        spec.selection
            .selected_inputs
            .iter()
            .any(|(key, value)| key == "target" && value == "rpi5")
    );
    assert_eq!(spec.metadata.branch.as_deref(), Some("main"));
    assert_eq!(spec.metadata.target.as_deref(), Some("rpi5"));
    assert_eq!(spec.metadata.profile.as_deref(), Some("ci"));
    assert_eq!(spec.identity.version.as_deref(), Some("9.9.9"));
    let out_dir = PathBuf::from(&spec.workspace.out_dir);
    assert_eq!(
        out_dir.file_name().and_then(|name| name.to_str()),
        Some("out-ci")
    );
    let stage_env = &spec.stage.env_sets[0];
    assert_eq!(stage_env.entries[0].1, "ci-env");
    assert_eq!(spec.provenance.identity.channel.as_deref(), Some("ci"));
    assert!(spec.policy.failure.rollback_on_error);
    assert!(!spec.policy.failure.preserve_failed_outputs);
    assert_eq!(spec.policy.execution.jobs, 0);
    assert_eq!(
        spec.policy.failure.rollback_domains,
        vec![
            RollbackDomain::Artifacts,
            RollbackDomain::Images,
            RollbackDomain::Stage,
            RollbackDomain::Checkpoints,
        ]
    );
    assert!(!spec.policy.providers.rust.allow_nested_build);
    assert_eq!(spec.policy.providers.rust.retry_attempts, 1);
    assert_eq!(spec.policy.providers.rust.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.rust.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.rust.timeout_seconds, 300);
    assert!(!spec.policy.providers.git.allow_remote_resolution);
    assert_eq!(spec.policy.providers.git.retry_attempts, 1);
    assert_eq!(spec.policy.providers.git.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.git.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.git.timeout_seconds, 60);
    assert_eq!(spec.policy.providers.archive.retry_attempts, 1);
    assert_eq!(spec.policy.providers.archive.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.archive.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.archive.timeout_seconds, 120);
    assert_eq!(spec.policy.providers.download.retry_attempts, 1);
    assert_eq!(spec.policy.providers.download.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.download.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.download.timeout_seconds, 120);
    assert_eq!(spec.policy.providers.go.retry_attempts, 1);
    assert_eq!(spec.policy.providers.go.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.go.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.go.timeout_seconds, 300);
    assert_eq!(spec.policy.providers.java.retry_attempts, 1);
    assert_eq!(spec.policy.providers.java.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.java.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.java.timeout_seconds, 300);
    assert_eq!(spec.policy.providers.node.retry_attempts, 1);
    assert_eq!(spec.policy.providers.node.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.node.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.node.timeout_seconds, 300);
    assert_eq!(spec.policy.providers.python.retry_attempts, 1);
    assert_eq!(spec.policy.providers.python.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.python.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.python.timeout_seconds, 300);
    assert_eq!(spec.policy.providers.buildroot.retry_attempts, 1);
    assert_eq!(spec.policy.providers.buildroot.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.buildroot.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.buildroot.timeout_seconds, 600);
    assert_eq!(spec.policy.providers.starting_point.retry_attempts, 1);
    assert_eq!(spec.policy.providers.starting_point.retry_backoff_ms, 0);
    assert_eq!(
        spec.policy.providers.starting_point.retry_backoff_strategy,
        RetryBackoffStrategySpec::Fixed
    );
    assert_eq!(spec.policy.providers.starting_point.timeout_seconds, 120);
    assert!(
        spec.metadata
            .labels
            .iter()
            .any(|(key, value)| key == "mode" && value == "ci")
    );
    assert!(
        spec.metadata
            .labels
            .iter()
            .any(|(key, value)| key == "target" && value == "rpi5")
    );
}

#[test]
fn resolves_global_docker_execution_backend() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                ("execution.docker.enabled".into(), "true".into()),
                (
                    "execution.docker.image".into(),
                    "ghcr.io/example/gaia-cross:latest".into(),
                ),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let docker = spec
        .policy
        .execution
        .docker
        .as_ref()
        .expect("docker backend should be configured");
    assert_eq!(docker.image, "ghcr.io/example/gaia-cross:latest");
}

#[test]
fn resolves_execution_output_retention_overrides() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                (
                    "execution.output_retention.stdout_bytes".into(),
                    "4096".into(),
                ),
                (
                    "policy.execution.output_retention.stderr_bytes".into(),
                    "8192".into(),
                ),
                (
                    "execution.output_retention.stdout_lines".into(),
                    "12".into(),
                ),
                (
                    "policy.execution.output_retention.stderr_lines".into(),
                    "24".into(),
                ),
                (
                    "execution.output_retention.failure_tail_lines".into(),
                    "7".into(),
                ),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let retention = spec.policy.execution.output_retention;
    assert_eq!(retention.stdout_bytes, 4096);
    assert_eq!(retention.stderr_bytes, 8192);
    assert_eq!(retention.stdout_lines, 12);
    assert_eq!(retention.stderr_lines, 24);
    assert_eq!(retention.failure_tail_lines, 7);
}

#[test]
fn resolves_buildroot_local_jobs_without_changing_scheduler_jobs() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                ("execution.jobs".into(), "8".into()),
                ("policy.providers.buildroot.local_jobs".into(), "2".into()),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    assert_eq!(spec.policy.execution.jobs, 8);
    assert_eq!(spec.policy.providers.buildroot.local_jobs, 2);
}

#[test]
fn resolves_buildroot_local_jobs_separately_from_scheduler_jobs() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                ("execution.jobs".into(), "3".into()),
                ("policy.providers.buildroot.local_jobs".into(), "2".into()),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    assert_eq!(spec.policy.execution.jobs, 3);
    assert_eq!(spec.policy.providers.buildroot.local_jobs, 2);
}

#[test]
fn resolves_buildroot_cache_policy() {
    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                (
                    "policy.providers.buildroot.download_dir".into(),
                    ".gaia/cache/buildroot/dl".into(),
                ),
                (
                    "policy.providers.buildroot.ccache.enabled".into(),
                    "true".into(),
                ),
                (
                    "policy.providers.buildroot.ccache.dir".into(),
                    ".gaia/cache/buildroot/ccache".into(),
                ),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    assert_eq!(
        spec.policy.providers.buildroot.download_dir.as_deref(),
        Some(".gaia/cache/buildroot/dl")
    );
    assert!(spec.policy.providers.buildroot.ccache.enabled);
    assert_eq!(
        spec.policy.providers.buildroot.ccache.dir.as_deref(),
        Some(".gaia/cache/buildroot/ccache")
    );
}

#[test]
fn resolves_buildroot_work_dir_policy() {
    let defaults = gaia_config::resolve_config(&default_config_path());
    assert_eq!(
        defaults.policy.providers.buildroot.work_dir.work_dir,
        "disk"
    );
    assert_eq!(
        defaults.policy.providers.buildroot.work_dir.ram_budget,
        None
    );
    assert!(defaults.policy.providers.buildroot.work_dir.keep_ram_tree);

    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                ("policy.providers.buildroot.work_dir".into(), "ram".into()),
                ("policy.providers.buildroot.ram_budget".into(), "60G".into()),
                (
                    "policy.providers.buildroot.keep_ram_tree".into(),
                    "false".into(),
                ),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let work_dir = &spec.policy.providers.buildroot.work_dir;
    assert_eq!(work_dir.work_dir, "ram");
    assert_eq!(work_dir.ram_budget.as_deref(), Some("60G"));
    assert!(!work_dir.keep_ram_tree);
}

#[test]
fn resolves_clean_profiles() {
    let config = write_temp_config(
        r#"
build_name = "clean-config"

[workspace]
root_dir = "."
build_dir = ".gaia/build/clean-config"
out_dir = ".gaia/out/clean-config"

[[workspace.named_paths]]
alias = "generated"
path = "generated"
kind = "logical"

[clean]
default = "dist"

[clean.profiles.dist]
description = "Remove dist outputs"
build = true
out = true
paths = [".cache/gaia", "@generated"]
"#,
    );

    let spec = resolve_config(&config.display().to_string());
    let profile = spec
        .clean
        .profiles
        .get("dist")
        .expect("clean profile should resolve");

    assert_eq!(spec.clean.default_profile.as_deref(), Some("dist"));
    assert_eq!(profile.description.as_deref(), Some("Remove dist outputs"));
    assert!(profile.build);
    assert!(profile.out);
    assert_eq!(
        profile.paths,
        vec![".cache/gaia".to_string(), "@generated".to_string()]
    );
}

#[test]
fn presets_override_buildroot_config_and_enable_shared_output() {
    let path = write_temp_config(
        r#"
build_name = "buildroot-dev-preset"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "raspberrypicm5io_defconfig"
config_overrides = [["BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "y"], ["BR2_PACKAGE_FOO", "y"]]

[presets.dev]
overrides = [
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "n"],
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD", "y"],
  ["policy.providers.buildroot.shared_output", "true"],
]
"#,
    );
    let config_overrides = |spec: &gaia_spec::ResolvedBuildSpec| match &spec.image.definition {
        gaia_spec::ImageDefinition::Buildroot(buildroot) => buildroot.config_overrides.clone(),
        _ => panic!("expected buildroot image"),
    };
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());

    let release = resolve_config(path.to_str().expect("path"));
    assert!(!release.policy.providers.buildroot.shared_output);
    assert!(config_overrides(&release).contains(&pair("BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "y")));

    let dev = gaia_config::resolve_config_with_options(
        path.to_str().expect("path"),
        &gaia_config::ResolveOptions {
            preset: Some("dev".into()),
            explicit_overrides: vec![(
                "policy.providers.buildroot.shared_output_dir".into(),
                "/var/cache/gaia-shared".into(),
            )],
            ..gaia_config::ResolveOptions::default()
        },
    );
    let overrides = config_overrides(&dev);
    assert!(overrides.contains(&pair("BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "n")));
    assert!(overrides.contains(&pair("BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD", "y")));
    assert!(overrides.contains(&pair("BR2_PACKAGE_FOO", "y")));
    assert!(dev.policy.providers.buildroot.shared_output);
    assert_eq!(
        dev.policy.providers.buildroot.shared_output_dir.as_deref(),
        Some("/var/cache/gaia-shared")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_buildroot_host_tools_policy() {
    let defaults = gaia_config::resolve_config(&default_config_path());
    assert_eq!(
        defaults.policy.providers.buildroot.host_tools,
        gaia_spec::BuildrootHostToolsPolicySpec::default()
    );

    let spec = gaia_config::resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![
                (
                    "policy.providers.buildroot.host_tools.default".into(),
                    "fail".into(),
                ),
                (
                    "policy.providers.buildroot.host_tools.ccache".into(),
                    "system,build".into(),
                ),
            ],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let host_tools = &spec.policy.providers.buildroot.host_tools;
    assert_eq!(
        host_tools.steps_for("ccache"),
        &[
            gaia_spec::HostToolStepSpec::System,
            gaia_spec::HostToolStepSpec::Build,
        ]
    );
    assert_eq!(
        host_tools.steps_for("pkgconf"),
        &[gaia_spec::HostToolStepSpec::Fail]
    );
}
