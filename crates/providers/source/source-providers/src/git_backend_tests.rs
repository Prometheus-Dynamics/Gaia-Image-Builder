//! Where a git source's git commands run (`git_backend`), credential
//! redaction in what Gaia writes, and the unattended git environment.

use super::*;
use gaia_spec::{CleanPolicy, DockerExecutionSpec, GitBackendSpec, SourceSpec, WorkspaceSpec};
use gaia_testkit::TempDir;

/// A build whose execution backend is docker, as in `[policy.execution]`.
fn docker_spec(build_dir: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("test");
    spec.workspace = WorkspaceSpec {
        root_dir: ".".into(),
        build_dir: build_dir.display().to_string(),
        out_dir: build_dir.join("out").display().to_string(),
        clean_policy: CleanPolicy::None,
        named_paths: Vec::new(),
    };
    spec.policy.execution.docker = Some(DockerExecutionSpec {
        image: "docker.io/library/alpine:latest".into(),
        reuse_container: false,
    });
    spec.policy.providers.git.allow_remote_resolution = true;
    spec.policy.providers.git.retry_attempts = 1;
    spec
}

fn docker_context(workspace_root: &Path) -> SourceExecutionContext {
    SourceExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        docker: Some(SourceDockerExecution {
            image: "docker.io/library/alpine:latest".into(),
        }),
    }
}

fn git_spec(repo: &str, backend: GitBackendSpec) -> GitSourceSpec {
    GitSourceSpec {
        repo: repo.into(),
        branch: Some("main".into()),
        tag: None,
        rev: None,
        subdir: None,
        update: true,
        refresh_policy: SourceRefreshPolicySpec::Always,
        pin_policy: SourcePinPolicySpec::Floating,
        locked_commit: None,
        git_backend: backend,
    }
}

fn program_of(command: &Command) -> String {
    command.get_program().to_string_lossy().into_owned()
}

#[test]
fn git_commands_never_prompt_for_credentials() {
    let command = git_command();
    let envs = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<Vec<_>>();
    assert!(envs.contains(&("GIT_TERMINAL_PROMPT".into(), Some("0".into()))));
    assert!(envs.contains(&("GCM_INTERACTIVE".into(), Some("never".into()))));
}

#[test]
fn host_backend_builds_git_for_the_host_even_with_docker_execution() {
    let git = git_spec("https://example.invalid/org/repo.git", GitBackendSpec::Host);
    let (execution, note) =
        git_command_execution_with(docker_context(Path::new("/work")), &git, "upstream", true);
    assert!(execution.docker.is_none());
    assert!(note.is_none());

    let mut ls_remote = git_command();
    ls_remote
        .arg("ls-remote")
        .arg(&git.repo)
        .arg("refs/heads/main");
    let wrapped = command_for_execution(&ls_remote, &execution).expect("host command");
    assert_eq!(program_of(&wrapped), "git");
    assert!(!wrapped.get_args().any(|arg| arg == OsStr::new("docker")));
}

#[test]
fn execution_backend_keeps_the_docker_wrapping() {
    let git = git_spec(
        "https://example.invalid/org/repo.git",
        GitBackendSpec::Execution,
    );
    let (execution, note) =
        git_command_execution_with(docker_context(Path::new("/work")), &git, "upstream", true);
    assert!(execution.docker.is_some());
    assert!(note.is_none());

    let mut ls_remote = git_command();
    ls_remote.arg("ls-remote").arg(&git.repo);
    let wrapped = command_for_execution(&ls_remote, &execution).expect("docker command");
    assert_eq!(program_of(&wrapped), "docker");
}

#[test]
fn host_backend_falls_back_to_execution_with_a_note_when_host_git_is_missing() {
    let git = git_spec("https://example.invalid/org/repo.git", GitBackendSpec::Host);
    let (execution, note) =
        git_command_execution_with(docker_context(Path::new("/work")), &git, "upstream", false);
    assert!(execution.docker.is_some());
    let note = note.expect("fallback note");
    assert!(note.contains("not installed"), "{note}");
    assert!(note.contains("'upstream'"), "{note}");
}

#[test]
fn host_backend_without_docker_needs_no_selection() {
    let git = git_spec("https://example.invalid/org/repo.git", GitBackendSpec::Host);
    let host = SourceExecutionContext {
        workspace_root: PathBuf::from("/work"),
        docker: None,
    };
    let (execution, note) = git_command_execution_with(host.clone(), &git, "upstream", false);
    assert_eq!(execution, host);
    assert!(note.is_none());
}

#[test]
fn local_repo_materializes_when_the_build_runs_in_docker() {
    let scratch = TempDir::new("gaia-git-backend-local-build");
    let build_dir = scratch.join("gaia-git-backend-local-build");
    let repo_dir = scratch.join("gaia-git-backend-local-repo");
    fs::create_dir_all(&repo_dir).expect("repo dir");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "gaia@example.com"],
        vec!["config", "user.name", "Gaia Test"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo_dir)
                .args(&args)
                .status()
                .expect("git")
                .success()
        );
    }
    fs::write(repo_dir.join("VERSION"), "one").expect("repo file");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo_dir)
            .args(["add", "."])
            .status()
            .expect("git add")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo_dir)
            .args(["commit", "-m", "init"])
            .status()
            .expect("git commit")
            .success()
    );

    let spec = docker_spec(&build_dir);
    let source = SourceSpec::new(
        "local-upstream",
        SourceDefinition::Git(git_spec(
            &format!("file://{}", repo_dir.display()),
            GitBackendSpec::Host,
        )),
    );
    let messages = GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("local git source materializes on the host");
    assert!(messages.iter().all(|message| !message.contains("docker")));

    let checkout = materialized_dir(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("VERSION")).expect("checked out file"),
        "one"
    );
    let state = fs::read_to_string(checkout.join(".gaia-source-state.txt")).expect("state");
    assert!(state.contains("git_backend=host"), "{state}");
    assert!(state.contains("git_command_backend=host"), "{state}");
}

#[test]
fn state_file_never_holds_url_credentials() {
    let scratch = TempDir::new("gaia-git-backend-redact-build");
    let build_dir = scratch.join("gaia-git-backend-redact-build");
    let mut spec = docker_spec(&build_dir);
    // Metadata-only materialization: nothing is fetched, but the state is
    // written with the repository.
    spec.policy.providers.git.allow_remote_resolution = false;
    let source = SourceSpec::new(
        "private-upstream",
        SourceDefinition::Git(git_spec(
            "https://ghp_secret_token@example.invalid/org/private.git",
            GitBackendSpec::Host,
        )),
    );
    GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("metadata-only materialization");

    let dir = materialized_dir(&spec, &source);
    for file in [
        ".gaia-source-state.txt",
        "source.txt",
        "remote-resolution-error.txt",
    ] {
        let contents = fs::read_to_string(dir.join(file)).expect(file);
        assert!(!contents.contains("ghp_secret_token"), "{file}: {contents}");
        assert!(
            contents.contains("https://***@example.invalid/org/private.git"),
            "{file}: {contents}"
        );
    }
}

#[test]
fn failed_host_git_names_the_source_and_suggests_credentials() {
    let scratch = TempDir::new("gaia-git-backend-fail-build");
    let build_dir = scratch.join("gaia-git-backend-fail-build");
    let spec = docker_spec(&build_dir);
    // Connection refused: fails fast without a network round trip.
    let source = SourceSpec::new(
        "private-upstream",
        SourceDefinition::Git(git_spec(
            "https://ghp_secret_token@127.0.0.1:9/org/private.git",
            GitBackendSpec::Host,
        )),
    );
    let error = GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("unreachable remote fails");
    assert!(
        error
            .message
            .contains("clone remote git source cache failed"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("'private-upstream'"),
        "{}",
        error.message
    );
    assert!(
        error
            .message
            .contains("https://***@127.0.0.1:9/org/private.git"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("credential helper"),
        "{}",
        error.message
    );
    assert!(
        !error.message.contains("ghp_secret_token"),
        "{}",
        error.message
    );
}

#[test]
fn git_backend_leaves_the_operation_fingerprint_input_unchanged() {
    // Operation fingerprints encode the serialized source spec, so the
    // backend must not appear there: where git runs never changes the checkout.
    let host = gaia_spec::canonical_json(&git_spec(
        "https://example.invalid/org/repo.git",
        GitBackendSpec::Host,
    ));
    let execution = gaia_spec::canonical_json(&git_spec(
        "https://example.invalid/org/repo.git",
        GitBackendSpec::Execution,
    ));
    assert_eq!(host, execution);
    assert!(!host.contains("git_backend"), "{host}");
}
