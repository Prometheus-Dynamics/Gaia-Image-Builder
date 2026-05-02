pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{
    ArtifactInstallClassSpec, BuildModeSpec, BuildrootExpectedImageFormatSpec,
    BuildrootExternalTreeModeSpec, CheckpointAnchorRef, DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS, DEFAULT_COMMAND_RETRY_ATTEMPTS,
    DEFAULT_COMMAND_RETRY_BACKOFF_MS, DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS, DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS, DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS, DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS, ImageDefinition, InputKindSpec,
    RetryBackoffStrategySpec, RollbackDomain, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec, StageContentOriginSpec, WorkspacePathKindSpec,
};
use std::path::PathBuf;
use support::{default_config_path, write_temp_config};

#[test]
fn resolves_layered_default_config() {
    let spec = resolve_config(&default_config_path());

    assert_eq!(spec.identity.display_name, "default");
    assert_eq!(spec.identity.build_name, "default");
    assert_eq!(spec.identity.version.as_deref(), Some("2.0.0"));
    assert_eq!(spec.metadata.branch.as_deref(), Some("main"));
    assert_eq!(spec.metadata.target.as_deref(), Some("cm5"));
    assert_eq!(spec.metadata.profile.as_deref(), Some("dev"));
    assert!(
        spec.selection
            .selected_build_file
            .as_deref()
            .is_some_and(|path| path.ends_with("examples/default-workspace/configs/default.toml"))
    );
    assert_eq!(
        spec.selection.selected_preset.as_deref(),
        Some("rewrite-dev")
    );
    assert_eq!(spec.selection.env_files, vec!["runtime.env".to_string()]);
    assert_eq!(
        spec.selection.precedence_order,
        vec![
            "ConfigDefaults".to_string(),
            "SelectedPreset".to_string(),
            "EnvFiles".to_string(),
            "InlineEnv".to_string(),
            "ProcessEnv".to_string(),
        ]
    );
    assert_eq!(spec.metadata.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        spec.metadata.description.as_deref(),
        Some("Gaia rewrite default build for workspace-level image assembly.")
    );
    assert!(
        spec.metadata
            .labels
            .iter()
            .any(|(key, value)| key == "mode" && value == "rewrite")
    );
    assert!(
        spec.metadata
            .labels
            .iter()
            .any(|(key, value)| key == "target" && value == "cm5")
    );
    assert_eq!(spec.metadata.product.family.as_deref(), Some("gaia"));
    assert_eq!(spec.metadata.product.name.as_deref(), Some("image-builder"));
    assert_eq!(
        spec.metadata.product.sku.as_deref(),
        Some("gaia-rewrite-dev")
    );
    assert_eq!(spec.policy.preset.selected.as_deref(), Some("rewrite-dev"));
    assert_eq!(spec.policy.preset.applied, vec!["rewrite-dev".to_string()]);
    assert_eq!(
        spec.selection.selected_inputs,
        vec![
            ("profile".to_string(), "dev".to_string()),
            ("target".to_string(), "cm5".to_string()),
        ]
    );
    assert_eq!(spec.inputs.declared.len(), 2);
    assert!(spec.inputs.declared.iter().any(|input| {
        input.name == "target"
            && input.kind == InputKindSpec::String
            && input.default.as_deref() == Some("cm5")
    }));
    assert!(spec.inputs.declared.iter().any(|input| {
        input.name == "profile"
            && input.kind == InputKindSpec::Enum
            && input.default.as_deref() == Some("dev")
            && input.choices == vec!["dev", "ci", "release"]
    }));
    assert!(spec.policy.interpolation.allow_unresolved);
    assert!(spec.policy.failure.rollback_on_error);
    assert!(!spec.policy.failure.preserve_failed_outputs);
    assert_eq!(spec.policy.failure.rollback_domains, RollbackDomain::all());
    assert_eq!(spec.policy.execution.jobs, 0);
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
        spec.policy
            .interpolation
            .values
            .iter()
            .any(|(key, value)| key == "release_channel" && value == "rewrite")
    );
    assert_eq!(
        spec.provenance.identity.project.as_deref(),
        Some("gaia-image-builder")
    );
    assert_eq!(
        spec.provenance.identity.vendor.as_deref(),
        Some("Prometheus Dynamics")
    );
    assert_eq!(spec.provenance.identity.channel.as_deref(), Some("rewrite"));
    assert!(
        spec.provenance
            .identity
            .labels
            .iter()
            .any(|(key, value)| key == "preset" && value == "rewrite-dev")
    );
    let root_dir = PathBuf::from(&spec.workspace.root_dir);
    assert!(root_dir.is_absolute());
    assert!(root_dir.join("Cargo.toml").is_file());
    let out_dir = PathBuf::from(&spec.workspace.out_dir);
    assert!(out_dir.is_absolute());
    assert_eq!(
        out_dir.file_name().and_then(|name| name.to_str()),
        Some("out")
    );
    assert!(out_dir.starts_with(root_dir.join(".gaia/examples/default-workspace")));
    assert!(spec.workspace.named_paths.iter().any(|path| {
        let named_path = PathBuf::from(&path.path);
        path.alias == "assets"
            && path.kind == WorkspacePathKindSpec::Host
            && named_path.ends_with(PathBuf::from("examples/default-workspace/assets"))
            && named_path.is_absolute()
    }));
    assert_eq!(spec.sources.len(), 2);
    assert_eq!(spec.artifacts.len(), 1);
    assert_eq!(
        spec.artifacts[0].build_mode,
        Some(BuildModeSpec::Custom("dev".into()))
    );
    let install_identity = spec.artifacts[0]
        .install_identity
        .as_ref()
        .expect("artifact install identity");
    assert_eq!(install_identity.install_name, "default");
    assert_eq!(
        install_identity.install_class,
        ArtifactInstallClassSpec::Binary
    );
    assert_eq!(
        install_identity.destination_hint.as_deref(),
        Some("/usr/bin/default")
    );
    assert_eq!(spec.install.entries.len(), 1);
    assert_eq!(spec.stage.files.len(), 1);
    assert_eq!(spec.stage.env_sets.len(), 1);
    assert_eq!(spec.stage.services.len(), 1);

    let git_source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "gaia-upstream")
        .expect("gaia-upstream source");
    match &git_source.definition {
        SourceDefinition::Git(git) => {
            assert_eq!(git.branch.as_deref(), Some("main"));
            assert_eq!(git.refresh_policy, SourceRefreshPolicySpec::Always);
            assert_eq!(git.pin_policy, SourcePinPolicySpec::Floating);
        }
        definition => panic!("expected git source, got {definition:?}"),
    }

    let workspace_source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "workspace-root")
        .expect("workspace-root source");
    match &workspace_source.definition {
        SourceDefinition::Path(path) => {
            assert_eq!(path.refresh_policy, SourceRefreshPolicySpec::Never);
            assert_eq!(path.pin_policy, SourcePinPolicySpec::Locked);
            assert_eq!(
                path.identity_ignore,
                vec![
                    ".git".to_string(),
                    ".gaia".to_string(),
                    "build".to_string(),
                    "out".to_string(),
                    "out-ci".to_string(),
                    "target".to_string(),
                ]
            );
        }
        definition => panic!("expected path source, got {definition:?}"),
    }

    let stage_env = &spec.stage.env_sets[0];
    assert_eq!(stage_env.entries[0].0, "GAIA_MODE");
    assert_eq!(stage_env.entries[0].1, "rewrite");
    assert_eq!(
        spec.stage.files[0].origin,
        StageContentOriginSpec::StaticAsset
    );
    assert_eq!(
        spec.checkpoints.points[0].anchor,
        CheckpointAnchorRef::Image
    );

    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            assert_eq!(
                buildroot.defconfig.as_deref(),
                Some("raspberrypi_defconfig")
            );
            assert_eq!(
                buildroot.external_tree_mode,
                BuildrootExternalTreeModeSpec::Auto
            );
            assert_eq!(buildroot.expected_images.len(), 1);
            assert_eq!(buildroot.expected_images[0].name, "rootfs.tar");
            assert_eq!(
                buildroot.expected_images[0].format,
                BuildrootExpectedImageFormatSpec::Tar
            );
            assert!(!buildroot.expected_images[0].required);
        }
        definition => panic!("expected buildroot image, got {definition:?}"),
    }
    assert_eq!(
        spec.image
            .feed
            .install_entries
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["install-gaia-app"]
    );
    assert_eq!(
        spec.image
            .feed
            .stage_files
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["motd"]
    );
    assert_eq!(
        spec.image
            .feed
            .stage_env_sets
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["runtime-env"]
    );
    assert_eq!(
        spec.image
            .feed
            .stage_services
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["gaia-service"]
    );

    assert!(
        spec.image
            .output
            .collect_dir
            .as_deref()
            .is_some_and(|path| {
                let collect_dir = PathBuf::from(path);
                collect_dir.file_name().and_then(|name| name.to_str()) == Some("images")
                    && collect_dir.parent() == Some(out_dir.as_path())
            })
    );
    assert_eq!(
        spec.image.output.archive_name.as_deref(),
        Some("default-2.0.0.tar")
    );
    assert!(spec.reporting.outputs.summary);
    assert!(spec.reporting.outputs.provenance);
    assert!(spec.reporting.outputs.manifest);
    assert!(spec.reporting.masking.enabled);
    assert_eq!(spec.reporting.masking.replacement, "***");
    assert!(spec.reporting.post_build.is_none());
    assert!(
        spec.reporting
            .masking
            .patterns
            .iter()
            .any(|pattern| pattern == "TOKEN")
    );
}

#[test]
fn resolves_string_and_conditional_table_imports() {
    let root = std::env::temp_dir().join(format!(
        "gaia-conditional-imports-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp config dir");
    let base = root.join("base.toml");
    let full = root.join("full.toml");
    let build = root.join("build.toml");
    std::fs::write(
        &base,
        r#"
[[sources]]
id = "base-source"
kind = "path"
path = "."
"#,
    )
    .expect("base");
    std::fs::write(
        &full,
        r#"
[[sources]]
id = "full-source"
kind = "path"
path = "."
"#,
    )
    .expect("full");
    std::fs::write(
        &build,
        r#"
build_name = "conditional"
profile = "base-os"
imports = [
  "base.toml",
  { path = "full.toml", when = { profile = "full" } },
]
"#,
    )
    .expect("build");

    let base_spec = gaia_config::resolve_config(&build.display().to_string());
    assert!(
        base_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "base-source")
    );
    assert!(
        !base_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "full-source")
    );

    let full_spec = gaia_config::resolve_config_with_options(
        &build.display().to_string(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![("build.profile".into(), "full".into())],
            ..Default::default()
        },
    );
    assert!(
        full_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "base-source")
    );
    assert!(
        full_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "full-source")
    );
}

#[test]
fn conditional_imports_see_selected_input_backed_build_profile() {
    let root = std::env::temp_dir().join(format!(
        "gaia-conditional-input-imports-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp config dir");
    let full = root.join("full.toml");
    let build = root.join("build.toml");
    std::fs::write(
        &full,
        r#"
[[sources]]
id = "full-source"
kind = "path"
path = "."
"#,
    )
    .expect("full");
    std::fs::write(
        &build,
        r#"
build_name = "conditional"
profile = "${input.profile}"
imports = [
  { path = "full.toml", when = { profile = "full" } },
]

[inputs.profile]
kind = "enum"
default = "base-os"
choices = ["base-os", "full"]
"#,
    )
    .expect("build");

    let base_spec = gaia_config::resolve_config(&build.display().to_string());
    assert!(
        !base_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "full-source")
    );

    let full_spec = gaia_config::resolve_config_with_options(
        &build.display().to_string(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![("input.profile".into(), "full".into())],
            ..Default::default()
        },
    );
    assert!(
        full_spec
            .sources
            .iter()
            .any(|source| source.id.as_str() == "full-source")
    );
}

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
