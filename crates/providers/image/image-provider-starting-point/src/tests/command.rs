use super::*;

#[test]
fn run_command_reports_missing_tool() {
    let execution = ImageExecutionContext {
        workspace_root: std::env::temp_dir(),
        docker_image: None,
        docker_reuse_container: false,
    };
    let error = run_command(
        Command::new("gaia-missing-starting-point-tool"),
        Path::new("/tmp/fake-starting-point.tar"),
        &execution,
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect_err("missing tool should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::ToolStart);
    assert!(
        error
            .message
            .contains("failed to start starting-point archive build")
    );
}

#[test]
fn run_command_times_out_long_running_processes() {
    let scratch = TempDir::new("gaia-starting-point-timeout");
    let root = scratch.path().to_path_buf();
    let execution = ImageExecutionContext {
        workspace_root: root.clone(),
        docker_image: None,
        docker_reuse_container: false,
    };
    let policy = ImageExecutionPolicy {
        timeout_seconds: 1,
        ..ImageExecutionPolicy::default()
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 5");

    let started = Instant::now();
    let error = run_command(
        command,
        &root.join("fake-starting-point.tar"),
        &execution,
        &policy,
        None,
        None,
    )
    .expect_err("sleep should time out");

    assert_eq!(error.kind, ImageProviderErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "timeout should interrupt before the child sleep completes"
    );
}

#[test]
fn command_for_execution_wraps_docker_backend() {
    let scratch = TempDir::new("gaia-starting-point-docker");
    let workspace_root = scratch.path().to_path_buf();
    let rootfs_dir = workspace_root.join("rootfs");
    let archive_path = workspace_root.join("out/rootfs.tar");
    fs::create_dir_all(&rootfs_dir).expect("rootfs dir");
    let execution = ImageExecutionContext {
        workspace_root: workspace_root.clone(),
        docker_image: Some("docker.io/library/alpine:latest".to_string()),
        docker_reuse_container: false,
    };
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&workspace_root)
        .arg("rootfs");

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(wrapped.get_program(), std::ffi::OsStr::new("docker"));
    assert!(args.contains(&"docker.io/library/alpine:latest".to_string()));
    assert!(args.contains(&"tar".to_string()));
    assert!(args.contains(&"-cf".to_string()));
}

#[test]
fn command_for_execution_mounts_parent_dir_for_output_files() {
    let scratch = TempDir::new("gaia-starting-point-docker-parent");
    let workspace_root = scratch.path().to_path_buf();
    let rootfs_dir = workspace_root.join("rootfs");
    let archive_path = workspace_root.join("out/rootfs.tar");
    fs::create_dir_all(&rootfs_dir).expect("rootfs dir");
    // As the provider does before building the tar command: docker must not
    // create a missing mount source (it would be root-owned).
    fs::create_dir_all(workspace_root.join("out")).expect("archive dir");
    let execution = ImageExecutionContext {
        workspace_root: workspace_root.clone(),
        docker_image: Some("docker.io/library/alpine:latest".to_string()),
        docker_reuse_container: false,
    };
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&workspace_root)
        .arg("rootfs");

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let mount = format!(
        "{}:{}",
        workspace_root.join("out").display(),
        workspace_root.join("out").display()
    );

    assert!(args.contains(&mount));
    assert!(!args.contains(&format!(
        "{}:{}",
        archive_path.display(),
        archive_path.display()
    )));
}
