use super::*;
use gaia_spec::{CleanPolicy, SourceSpec, WorkspaceSpec};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}"))
}

fn test_spec(build_dir: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("test");
    spec.workspace = WorkspaceSpec {
        root_dir: ".".into(),
        build_dir: build_dir.display().to_string(),
        out_dir: build_dir.join("out").display().to_string(),
        clean_policy: CleanPolicy::None,
        named_paths: Vec::new(),
    };
    spec
}

struct DummySourceProvider;

impl SourceProvider for DummySourceProvider {
    fn id(&self) -> &'static str {
        "source.dummy"
    }

    fn kind(&self) -> SourceProviderKind {
        SourceProviderKind::Path
    }
}

#[test]
fn default_source_execution_fails_instead_of_materializing_placeholder() {
    let build_dir = temp_path("gaia-source-default-exec");
    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "dummy-source",
        SourceDefinition::Path(PathSourceSpec {
            path: ".".into(),
            identity_ignore: Vec::new(),
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    let error = DummySourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("default source execution should fail");

    assert_eq!(error.kind, SourceProviderErrorKind::PolicyBlocked);
    assert!(!materialized_dir(&spec, &source).exists());
}

#[test]
fn path_provider_creates_materialized_link_or_manifest() {
    let build_dir = temp_path("gaia-source-path-build");
    let source_root = temp_path("gaia-source-path-root");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(source_root.join("hello.txt"), "hi").expect("source file");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "workspace-root",
        SourceDefinition::Path(PathSourceSpec {
            path: source_root.display().to_string(),
            identity_ignore: Vec::new(),
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    PathSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("path source execution");

    assert!(
        materialized_dir(&spec, &source)
            .join("source.txt")
            .is_file()
    );
    let state = fs::read_to_string(materialized_dir(&spec, &source).join(".gaia-source-state.txt"))
        .expect("path source state");
    assert!(state.contains("provider=source.path"));
    assert!(state.contains("path_digest="));
    assert!(state.contains("content_identity_mode=live-reference"));
    assert!(state.contains("refresh_policy=auto"));
    assert!(state.contains("pin_policy=floating"));
}

#[test]
fn command_for_execution_mounts_parent_dir_for_output_files() {
    let workspace_root = temp_path("gaia-source-docker-parent");
    let archive_path = workspace_root.join("downloads/archive.tar.gz");
    fs::create_dir_all(workspace_root.join("downloads")).expect("downloads dir");
    let execution = SourceExecutionContext {
        workspace_root: workspace_root.clone(),
        docker: Some(SourceDockerExecution {
            image: "docker.io/library/alpine:latest".to_string(),
        }),
    };
    let mut command = Command::new("curl");
    command
        .arg("-L")
        .arg("https://example.invalid/archive.tar.gz")
        .arg("-o")
        .arg(&archive_path);

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let mount = format!(
        "{}:{}",
        workspace_root.join("downloads").display(),
        workspace_root.join("downloads").display()
    );

    assert!(args.contains(&mount));
    assert!(!args.contains(&format!(
        "{}:{}",
        archive_path.display(),
        archive_path.display()
    )));
}

#[test]
fn git_provider_persists_resolved_commit_and_selected_ref() {
    let build_dir = temp_path("gaia-source-git-build");
    let repo_dir = temp_path("gaia-source-git-repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");

    Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(&repo_dir)
        .output()
        .expect("git init");
    Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.email")
        .arg("gaia@example.com")
        .output()
        .expect("git user email");
    Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.name")
        .arg("Gaia Test")
        .output()
        .expect("git user name");
    fs::write(repo_dir.join("README.md"), "hello").expect("repo file");
    Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("add")
        .arg(".")
        .output()
        .expect("git add");
    Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("commit")
        .arg("-m")
        .arg("init")
        .output()
        .expect("git commit");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "gaia-upstream",
        SourceDefinition::Git(GitSourceSpec {
            repo: repo_dir.display().to_string(),
            branch: Some("main".into()),
            tag: None,
            rev: None,
            subdir: None,
            update: true,
            refresh_policy: SourceRefreshPolicySpec::Always,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("git source execution");

    let state = fs::read_to_string(materialized_dir(&spec, &source).join(".gaia-source-state.txt"))
        .expect("git source state");
    assert!(state.contains("provider=source.git"));
    assert!(state.contains("selected_ref_type=branch"));
    assert!(state.contains("selected_ref_value=main"));
    assert!(state.contains("resolved_mode=local"));
    assert!(state.contains("resolved_commit_sha="));
    assert!(state.contains("resolved_refresh_decision=always"));
}

#[test]
fn download_provider_fetches_local_file_url() {
    let build_dir = temp_path("gaia-source-download-build");
    let download_root = temp_path("gaia-source-download-root");
    fs::create_dir_all(&download_root).expect("download root");
    let source_file = download_root.join("payload.txt");
    fs::write(&source_file, "payload").expect("payload file");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "downloaded",
        SourceDefinition::Download(DownloadSourceSpec {
            url: format!("file://{}", source_file.display()),
            sha256: None,
            output_path: "payload.txt".into(),
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    DownloadSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("download source execution");

    let fetched = materialized_dir(&spec, &source).join("payload.txt");
    assert_eq!(
        fs::read_to_string(fetched).expect("downloaded file"),
        "payload"
    );
    let state = fs::read_to_string(materialized_dir(&spec, &source).join(".gaia-source-state.txt"))
        .expect("download source state");
    assert!(state.contains("provider=source.download"));
    assert!(state.contains("output_sha256="));
    assert!(state.contains("checksum_policy=observed-only"));
    assert!(state.contains("checksum_source=downloaded-file"));
    assert!(state.contains("refresh_policy=auto"));
    assert!(state.contains("pin_policy=floating"));
}

#[test]
fn path_provider_errors_for_missing_path() {
    let build_dir = temp_path("gaia-source-missing-path-build");
    let missing_root = temp_path("gaia-source-missing-path-root");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "workspace-root",
        SourceDefinition::Path(PathSourceSpec {
            path: missing_root.display().to_string(),
            identity_ignore: Vec::new(),
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    let error = PathSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("missing path should fail");
    assert_eq!(error.kind, SourceProviderErrorKind::OutputMissing);
    assert!(error.message.contains("failed to resolve path source"));
}

#[test]
fn archive_provider_errors_for_missing_archive() {
    let build_dir = temp_path("gaia-source-missing-archive-build");
    let missing_archive = temp_path("gaia-source-missing-archive-root").join("missing.tar");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "archive-source",
        SourceDefinition::Archive(ArchiveSourceSpec {
            path: missing_archive.display().to_string(),
            strip_components: 0,
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Floating,
        }),
    );

    let error = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("missing archive should fail");
    assert_eq!(error.kind, SourceProviderErrorKind::OutputMissing);
    assert!(error.message.contains("failed to resolve path source"));
}

#[test]
fn download_provider_detects_sha_mismatch() {
    let build_dir = temp_path("gaia-source-download-sha-build");
    let download_root = temp_path("gaia-source-download-sha-root");
    fs::create_dir_all(&download_root).expect("download root");
    let source_file = download_root.join("payload.txt");
    fs::write(&source_file, "payload").expect("payload file");

    let spec = test_spec(&build_dir);
    let source = SourceSpec::new(
        "downloaded",
        SourceDefinition::Download(DownloadSourceSpec {
            url: format!("file://{}", source_file.display()),
            sha256: Some("deadbeef".into()),
            output_path: "payload.txt".into(),
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Locked,
        }),
    );

    let error = DownloadSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("sha mismatch should fail");
    assert_eq!(error.kind, SourceProviderErrorKind::OutputMissing);
    assert!(error.message.contains("sha256 mismatch"));
}

#[test]
fn run_command_reports_missing_tool() {
    let execution = SourceExecutionContext {
        workspace_root: std::env::temp_dir(),
        docker: None,
    };
    let error = run_command_with_policy(
        Command::new("gaia-missing-source-tool"),
        &execution,
        "materialize source backend",
        SourceCommandPolicy {
            attempts: 1,
            retry_backoff_ms: 0,
            retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
            timeout_seconds: 60,
            output_retention: ProcessOutputRetention::default(),
        },
        None,
        None,
    )
    .expect_err("missing tool should fail");

    assert_eq!(error.kind, SourceProviderErrorKind::ToolStart);
    assert!(
        error
            .message
            .contains("failed to start materialize source backend")
    );
}

#[test]
fn command_for_execution_wraps_docker_backend() {
    let workspace_root = temp_path("gaia-source-docker-workspace");
    fs::create_dir_all(&workspace_root).expect("workspace dir");
    let repo_dir = workspace_root.join("repo");
    let out_dir = workspace_root.join("out");
    fs::create_dir_all(&repo_dir).expect("repo dir");
    fs::create_dir_all(&out_dir).expect("out dir");
    let execution = SourceExecutionContext {
        workspace_root: workspace_root.clone(),
        docker: Some(SourceDockerExecution {
            image: "docker.io/library/alpine:latest".to_string(),
        }),
    };
    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg(&repo_dir)
        .arg(&out_dir)
        .current_dir(&workspace_root);

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(wrapped.get_program(), OsStr::new("docker"));
    assert!(args.contains(&"docker.io/library/alpine:latest".to_string()));
    assert!(args.contains(&"git".to_string()));
    assert!(args.contains(&"clone".to_string()));
    assert!(args.iter().any(
            |arg| arg == &format!("{}:{}", workspace_root.display(), workspace_root.display())
        ));
}

#[test]
fn retry_backoff_duration_supports_fixed_and_exponential_strategies() {
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Fixed, 10, 1),
        Duration::from_millis(10)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Fixed, 10, 5),
        Duration::from_millis(10)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Exponential, 10, 1),
        Duration::from_millis(10)
    );
    assert_eq!(
        retry_backoff_duration(RetryBackoffStrategySpec::Exponential, 10, 3),
        Duration::from_millis(40)
    );
}
