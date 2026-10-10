use crate::*;
use gaia_spec::{
    ArtifactDefinition, ArtifactExecutionSpec, ArtifactOutputSpec, ArtifactVariantSpec,
    DockerExecutionSpec, GradleHomeSpec,
};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{}-{nonce}", std::process::id()))
}

struct DummyArtifactProvider;

impl ArtifactProvider for DummyArtifactProvider {
    fn id(&self) -> &'static str {
        "artifact.dummy"
    }

    fn kind(&self) -> ArtifactProviderKind {
        ArtifactProviderKind::Rust
    }
}

#[test]
fn default_artifact_execution_fails_instead_of_materializing_placeholder() {
    let artifact = ArtifactSpec::new(
        "dummy-artifact",
        ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "dummy".into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: temp_path("gaia-artifact-default-exec")
                .join("dummy.bin")
                .display()
                .to_string(),
        },
    );
    let contract = DummyArtifactProvider.plan_artifact(&artifact).contract;

    let error = DummyArtifactProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect_err("default artifact execution should fail");

    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(!PathBuf::from(&contract.output.path).exists());
}

#[test]
fn materialize_artifact_output_cleans_temp_file_when_rename_fails() {
    let root = temp_path("gaia-artifact-output-failure");
    fs::create_dir_all(&root).expect("root dir");
    let output_path = root.join("existing-dir");
    fs::create_dir_all(&output_path).expect("existing output dir");
    fs::write(output_path.join("sentinel"), "keep").expect("non-empty output dir");
    let contract = ArtifactExecutionContract {
        provider: ArtifactProviderKind::Rust,
        source: None,
        source_dir: None,
        workspace_root: None,
        execution_backend_explicit: false,
        execution_backend: ArtifactExecutionBackend::Host,
        build_version: None,
        build_branch: None,
        build_target: None,
        build_profile: None,
        artifact_target: None,
        allow_nested_build: false,
        job_budget: None,
        gradle_home: GradleHomeSpec::default(),
        rust_shared_target_dir: false,
        retry_attempts: 1,
        retry_backoff_ms: 0,
        retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
        timeout_seconds: 300,
        output_retention: gaia_spec::OutputRetentionPolicySpec::default(),
        build_mode: None,
        dependencies: Vec::new(),
        output: ArtifactOutputContract {
            path: output_path.display().to_string(),
            kind: ArtifactOutputKind::File,
        },
    };

    let error = materialize_artifact_output(&contract, "payload")
        .expect_err("rename into existing directory should fail");

    assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
    assert!(error.message.contains("failed to move artifact output"));
    assert!(!output_path.with_extension("gaia.tmp").exists());
}

#[test]
fn copy_artifact_file_to_output_creates_parent_and_finalizes_temp_file() {
    let root = temp_path("gaia-artifact-copy-output");
    fs::create_dir_all(&root).expect("root dir");
    let source = root.join("built.bin");
    let output = root.join("nested").join("artifact.bin");
    fs::write(&source, "artifact").expect("source artifact");

    copy_artifact_file_to_output(&source, &output, "test artifact").expect("copy artifact");

    assert_eq!(fs::read_to_string(&output).expect("output"), "artifact");
    assert!(!output.with_extension("gaia.copy.tmp").exists());
}

#[test]
fn artifact_package_root_resolves_relative_package_dirs_against_source() {
    assert_eq!(
        artifact_package_root("/workspace/source", "packages/app"),
        PathBuf::from("/workspace/source/packages/app")
    );
    assert_eq!(
        artifact_package_root("/workspace/source", "/opt/app"),
        PathBuf::from("/opt/app")
    );
}

#[test]
fn ensure_artifact_output_parent_creates_parent_directories() {
    let output_path = temp_path("gaia-artifact-parent").join("nested/out.bin");

    ensure_artifact_output_parent(&output_path).expect("output parent");

    assert!(output_path.parent().expect("parent").is_dir());
    let _ = fs::remove_dir_all(output_path.ancestors().nth(1).expect("test root"));
}

#[test]
fn materialize_artifact_marker_and_state_writes_both_files() {
    let output_path = temp_path("gaia-artifact-marker-state").join("artifact.bin");
    let contract = ArtifactExecutionContract {
        provider: ArtifactProviderKind::Rust,
        source: None,
        source_dir: None,
        workspace_root: None,
        execution_backend_explicit: false,
        execution_backend: ArtifactExecutionBackend::Host,
        build_version: None,
        build_branch: None,
        build_target: None,
        build_profile: None,
        artifact_target: None,
        allow_nested_build: false,
        job_budget: None,
        gradle_home: GradleHomeSpec::default(),
        rust_shared_target_dir: false,
        retry_attempts: 1,
        retry_backoff_ms: 0,
        retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
        timeout_seconds: 300,
        output_retention: gaia_spec::OutputRetentionPolicySpec::default(),
        build_mode: None,
        dependencies: Vec::new(),
        output: ArtifactOutputContract {
            path: output_path.display().to_string(),
            kind: ArtifactOutputKind::File,
        },
    };

    materialize_artifact_marker_and_state(&contract, "marker", "state").expect("marker and state");

    assert_eq!(
        fs::read_to_string(
            output_path
                .parent()
                .expect("parent")
                .join(".gaia/artifact.bin.gaia-build.txt")
        )
        .expect("marker"),
        "marker"
    );
    assert_eq!(
        fs::read_to_string(
            output_path
                .parent()
                .expect("parent")
                .join(".gaia/artifact.bin.gaia-state.txt")
        )
        .expect("state"),
        "state"
    );
    let _ = fs::remove_dir_all(output_path.ancestors().nth(1).expect("test root"));
}

#[test]
fn retry_backoff_duration_supports_fixed_and_exponential_strategies() {
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Fixed, 25, 1),
        Duration::from_millis(25)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Fixed, 25, 4),
        Duration::from_millis(25)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Exponential, 25, 1),
        Duration::from_millis(25)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Exponential, 25, 2),
        Duration::from_millis(50)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Exponential, 25, 4),
        Duration::from_millis(200)
    );
}

#[test]
fn command_for_execution_wraps_docker_backend() {
    let workspace_root = temp_path("gaia-docker-workspace");
    fs::create_dir_all(&workspace_root).expect("workspace dir");
    let source_dir = workspace_root.join("src");
    fs::create_dir_all(&source_dir).expect("source dir");
    let contract = ArtifactExecutionContract {
        provider: ArtifactProviderKind::Rust,
        source: None,
        source_dir: Some(source_dir.display().to_string()),
        workspace_root: Some(workspace_root.display().to_string()),
        execution_backend_explicit: true,
        execution_backend: ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
            image: "ghcr.io/example/rust-cross:latest".to_string(),
            build: None,
        }),
        artifact_target: Some("aarch64-unknown-linux-gnu".to_string()),
        build_version: None,
        build_branch: None,
        build_target: None,
        build_profile: None,
        allow_nested_build: false,
        job_budget: None,
        gradle_home: GradleHomeSpec::default(),
        rust_shared_target_dir: false,
        retry_attempts: 1,
        retry_backoff_ms: 0,
        retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
        timeout_seconds: 300,
        output_retention: gaia_spec::OutputRetentionPolicySpec::default(),
        build_mode: None,
        dependencies: Vec::new(),
        output: ArtifactOutputContract {
            path: source_dir.join("out.bin").display().to_string(),
            kind: ArtifactOutputKind::File,
        },
    };
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--target")
        .arg("aarch64-unknown-linux-gnu")
        .current_dir(&source_dir)
        .env(
            "CARGO_TARGET_DIR",
            workspace_root.join(".gaia/cargo-target"),
        );

    let wrapped = command_for_execution(&command, &contract).expect("docker wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(wrapped.get_program(), OsStr::new("docker"));
    assert!(args.starts_with(&["run".to_string(), "--rm".to_string(),]));
    assert!(args.iter().any(|arg| {
        arg == &format!("{}:{}", workspace_root.display(), workspace_root.display())
    }));
    assert!(
        args.iter()
            .any(|arg| arg == &source_dir.display().to_string())
    );
    assert!(args.iter().any(|arg| {
        arg == &format!(
            "HOME={}",
            workspace_root.join(".gaia/docker-home").display()
        )
    }));
    assert!(args.iter().any(|arg| {
        arg == &format!(
            "XDG_CACHE_HOME={}",
            workspace_root.join(".gaia/docker-cache").display()
        )
    }));
    assert!(args.contains(&"ghcr.io/example/rust-cross:latest".to_string()));
    assert!(args.contains(&"cargo".to_string()));
    assert!(args.contains(&"build".to_string()));
    assert!(args.contains(&"--target".to_string()));
    assert!(args.contains(&"aarch64-unknown-linux-gnu".to_string()));
    assert!(args.iter().any(|arg| arg.starts_with("CARGO_TARGET_DIR=")));
}

#[test]
fn run_command_with_retries_reuses_contract_retry_policy() {
    let root = temp_path("gaia-artifact-retry");
    fs::create_dir_all(&root).expect("root dir");
    let marker = root.join("attempted");
    let output_path = root.join("out.bin");
    let contract = ArtifactExecutionContract {
        provider: ArtifactProviderKind::Rust,
        source: None,
        source_dir: Some(root.display().to_string()),
        workspace_root: Some(root.display().to_string()),
        execution_backend_explicit: false,
        execution_backend: ArtifactExecutionBackend::Host,
        build_version: None,
        build_branch: None,
        build_target: None,
        build_profile: None,
        artifact_target: None,
        allow_nested_build: false,
        job_budget: None,
        gradle_home: GradleHomeSpec::default(),
        rust_shared_target_dir: false,
        retry_attempts: 2,
        retry_backoff_ms: 0,
        retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
        timeout_seconds: 5,
        output_retention: gaia_spec::OutputRetentionPolicySpec::default(),
        build_mode: None,
        dependencies: Vec::new(),
        output: ArtifactOutputContract {
            path: output_path.display().to_string(),
            kind: ArtifactOutputKind::File,
        },
    };
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("if [ ! -f attempted ]; then touch attempted; exit 7; fi")
        .current_dir(&root);

    run_command_with_retries(&command, &contract, "retry smoke", None, None)
        .expect("second attempt should succeed");

    assert!(marker.exists());
}

#[test]
fn artifact_execution_backend_prefers_explicit_setting_over_global_default() {
    let mut spec = ResolvedBuildSpec::new("docker-selection");
    spec.workspace.root_dir = std::env::temp_dir()
        .join("gaia-docker-selection")
        .display()
        .to_string();
    spec.policy.execution.docker = Some(DockerExecutionSpec {
        image: "docker.io/library/rust:1.90".to_string(),
    });

    let inherited = ArtifactSpec::new(
        "inherited",
        ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "demo".to_string(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: "out/inherited".to_string(),
        },
    );
    let mut explicit_host = inherited.clone();
    explicit_host.id = gaia_spec::ArtifactId::new("explicit-host");
    explicit_host.execution = Some(ArtifactExecutionSpec::Host);

    let inherited_contract = ArtifactExecutionContract::from_spec(
        &inherited,
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    )
    .with_build_context(&spec);
    let explicit_host_contract = ArtifactExecutionContract::from_spec(
        &explicit_host,
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    )
    .with_build_context(&spec);

    assert!(matches!(
        inherited_contract.execution_backend,
        ArtifactExecutionBackend::Docker(_)
    ));
    assert!(matches!(
        explicit_host_contract.execution_backend,
        ArtifactExecutionBackend::Host
    ));
}

#[test]
fn artifact_contract_rejects_empty_inherited_docker_image() {
    let mut spec = gaia_spec::ResolvedBuildSpec::new("artifact-contract");
    spec.policy.execution.docker = Some(DockerExecutionSpec {
        image: String::new(),
    });
    let artifact = ArtifactSpec::new(
        "inherited",
        ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "demo".to_string(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: "out/inherited".to_string(),
        },
    );

    let error = ArtifactExecutionContract::from_spec(
        &artifact,
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    )
    .try_with_build_context(&spec)
    .expect_err("empty inherited docker image should be rejected");

    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("non-empty image"));
}

const FAKE_DOCKER: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/calls.log"
case "$1" in
  image)
    key=$(printf %s "$5" | tr '/:' '__')
    if [ -f "$dir/images/$key" ]; then echo "sha256:fake-$key"; exit 0; fi
    echo "Error: No such image: $5" >&2
    exit 1
    ;;
  build)
    key=$(printf %s "$5" | tr '/:' '__')
    mkdir -p "$dir/images" && touch "$dir/images/$key"
    exit 0
    ;;
esac
exit 2
"#;

fn dockerfile_artifact() -> ArtifactSpec {
    let mut artifact = ArtifactSpec::new(
        "helios-engine",
        ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "demo".to_string(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: "out/helios-engine".to_string(),
        },
    );
    artifact.execution = Some(ArtifactExecutionSpec::Docker(
        gaia_spec::DockerArtifactExecutionSpec {
            image: Some("registry.example/helios-cross-rust194:latest".into()),
            dockerfile: Some("docker/rust/Dockerfile".into()),
            context: None,
        },
    ));
    artifact
}

fn dockerfile_contract(
    spec: &ResolvedBuildSpec,
) -> Result<ArtifactExecutionContract, ArtifactProviderError> {
    ArtifactExecutionContract::from_spec(
        &dockerfile_artifact(),
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    )
    .try_with_build_context(spec)
}

fn docker_tag(contract: &ArtifactExecutionContract) -> String {
    match &contract.execution_backend {
        ArtifactExecutionBackend::Docker(docker) => docker.image.clone(),
        ArtifactExecutionBackend::Host => panic!("expected docker backend"),
    }
}

#[test]
fn dockerfile_execution_image_is_built_once_and_tagged_by_content() {
    let workspace = temp_path("gaia-dockerfile-exec");
    let docker_dir = workspace.join("docker/rust");
    fs::create_dir_all(&docker_dir).expect("docker dir");
    fs::write(docker_dir.join("Dockerfile"), "FROM rust:1.94\n").expect("dockerfile");
    let tools = temp_path("gaia-dockerfile-fake-docker");
    fs::create_dir_all(&tools).expect("tools dir");
    let fake_docker = tools.join("docker");
    fs::write(&fake_docker, FAKE_DOCKER).expect("fake docker");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_docker, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let mut spec = ResolvedBuildSpec::new("dockerfile-exec");
    spec.workspace.root_dir = workspace.display().to_string();

    let mut contract = dockerfile_contract(&spec).expect("contract");
    let tag = docker_tag(&contract);
    assert!(
        tag.starts_with("gaia-local/helios-cross-rust194:"),
        "unexpected tag {tag}"
    );

    let messages = crate::command::ensure_docker_execution_image_with(
        fake_docker.as_os_str(),
        &mut contract,
        None,
        None,
    )
    .expect("image build");
    assert!(
        messages[0].contains("built docker execution image"),
        "{messages:?}"
    );
    let state = render_build_context_state(&contract);
    assert!(state.contains(&format!("execution_backend_image={tag}")));
    assert!(state.contains("execution_backend_image_id=sha256:fake-"));
    assert!(state.contains("execution_backend_image_hash="));

    // The tag exists now, so a second run only inspects it.
    let mut again = dockerfile_contract(&spec).expect("contract");
    let messages = crate::command::ensure_docker_execution_image_with(
        fake_docker.as_os_str(),
        &mut again,
        None,
        None,
    )
    .expect("image reuse");
    assert!(messages[0].contains("up to date"), "{messages:?}");
    let calls = fs::read_to_string(tools.join("calls.log")).expect("calls");
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("build"))
            .count(),
        1
    );

    // Editing the Dockerfile yields a new tag, so the image is rebuilt.
    fs::write(docker_dir.join("Dockerfile"), "FROM rust:1.95\n").expect("dockerfile");
    let changed = dockerfile_contract(&spec).expect("contract");
    assert_ne!(docker_tag(&changed), tag);
}

#[test]
fn dockerfile_execution_rejects_missing_dockerfile() {
    let workspace = temp_path("gaia-dockerfile-missing");
    fs::create_dir_all(&workspace).expect("workspace");
    let mut spec = ResolvedBuildSpec::new("dockerfile-missing");
    spec.workspace.root_dir = workspace.display().to_string();
    let error = dockerfile_contract(&spec).expect_err("missing dockerfile");
    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(
        error.message.contains("failed to hash docker build"),
        "{}",
        error.message
    );
}
