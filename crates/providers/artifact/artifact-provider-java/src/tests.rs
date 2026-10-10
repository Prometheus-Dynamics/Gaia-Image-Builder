use super::*;
use std::ffi::OsStr;
use std::fs;
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

#[test]
fn run_command_reports_missing_tool() {
    let error = run_command(
        Command::new("gaia-missing-java-tool"),
        "java build",
        &ArtifactExecutionContract::from_spec(
            &ArtifactSpec::new(
                "java-missing-tool",
                ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                    build_target: "app.jar".into(),
                    build_args: Vec::new(),
                    build_command: Vec::new(),
                    build_env: Vec::new(),
                }),
                None,
                gaia_spec::ArtifactOutputSpec {
                    path: "out/app.jar".into(),
                },
            ),
            None,
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        ),
        None,
        None,
    )
    .expect_err("missing tool should fail");

    assert_eq!(
        error.kind,
        gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
    );
    assert!(error.message.contains("failed to start java build"));
}

#[test]
fn java_artifact_state_persists_backend_native_fields() {
    let output_path = temp_path("gaia-java-provider-state");
    fs::write(&output_path, "artifact").expect("output");
    let artifact = ArtifactSpec::new(
        "java-artifact",
        ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
            build_target: "build/libs/app.jar".into(),
            build_args: Vec::new(),
            build_command: Vec::new(),
            build_env: Vec::new(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: output_path.display().to_string(),
        },
    );
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(temp_path("gaia-java-provider-src").display().to_string()),
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    let state = artifact_state_contents(
        "artifact.java",
        artifact.id.as_str(),
        &contract,
        "build/libs/app.jar",
        "maven",
    );

    assert!(state.contains("resolved_identifier_kind=build-target"));
    assert!(state.contains("resolved_identifier=build/libs/app.jar"));
    assert!(state.contains("output_class=jar"));
    assert!(state.contains("build_tool=maven"));
    assert!(state.contains("build_target=build/libs/app.jar"));
}

#[test]
fn execute_artifact_uses_custom_build_command() {
    let source_dir = temp_path("gaia-java-provider-custom-src");
    let output_path = temp_path("gaia-java-provider-custom-out").join("app.jar");
    fs::create_dir_all(&source_dir).expect("source dir");
    let artifact = ArtifactSpec::new(
        "java-custom-command",
        ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
            build_target: "build/libs/app.jar".into(),
            build_args: Vec::new(),
            build_command: vec![
                "bash".into(),
                "-c".into(),
                "mkdir -p build/libs && printf custom > build/libs/app.jar".into(),
            ],
            build_env: Vec::new(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: output_path.display().to_string(),
        },
    );
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(source_dir.display().to_string()),
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    JavaProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect("custom command artifact");

    assert_eq!(
        fs::read_to_string(&output_path).expect("copied artifact"),
        "custom"
    );
}

fn gradle_env(home: &str) -> Vec<(String, String)> {
    vec![
        ("GRADLE_USER_HOME".into(), home.into()),
        ("MAVEN_LOCAL_REPO".into(), "/ws/build/m2".into()),
    ]
}

#[test]
fn gradle_home_stays_in_workspace_by_default() {
    let env = gradle_env("/ws/.gaia/docker-home/.gradle");
    let cache = Path::new("/home/u/.cache/gaia");
    let (rewritten, mount) = redirect_gradle_home(&env, true, false, Some(cache));
    assert_eq!(rewritten, env);
    assert_eq!(mount, None);
}

#[test]
fn gradle_home_moves_to_user_cache_when_requested_for_docker() {
    let env = gradle_env("/ws/.gaia/docker-home/.gradle");
    let cache = Path::new("/home/u/.cache/gaia");
    let (rewritten, mount) = redirect_gradle_home(&env, true, true, Some(cache));
    let expected = PathBuf::from("/home/u/.cache/gaia/gradle-home");
    assert_eq!(mount.as_deref(), Some(expected.as_path()));
    assert_eq!(rewritten[0].1, expected.display().to_string());
    // Other variables pass through untouched.
    assert_eq!(rewritten[1], env[1]);
}

#[test]
fn gradle_home_redirect_ignores_host_builds_and_custom_homes() {
    let cache = Path::new("/home/u/.cache/gaia");
    let workspace_home = gradle_env("/ws/.gaia/docker-home/.gradle");
    // Host builds keep the spec's home.
    assert_eq!(
        redirect_gradle_home(&workspace_home, false, true, Some(cache)).0,
        workspace_home
    );
    // A home the spec placed elsewhere is not moved.
    let elsewhere = gradle_env("/opt/gradle-home");
    assert_eq!(
        redirect_gradle_home(&elsewhere, true, true, Some(cache)).0,
        elsewhere
    );
    // No user cache root: keep the spec's home.
    assert_eq!(
        redirect_gradle_home(&workspace_home, true, true, None).0,
        workspace_home
    );
}

#[test]
fn configured_gradle_home_drives_the_redirect() {
    let env = gradle_env("/ws/.gaia/docker-home/.gradle");
    let cache = Path::new("/home/u/.cache/gaia");
    let mode = effective_gradle_home(GradleHomeSpec::UserCache, None).expect("config mode");
    let (rewritten, mount) =
        redirect_gradle_home(&env, true, mode == GradleHomeSpec::UserCache, Some(cache));
    assert_eq!(
        mount.as_deref(),
        Some(Path::new("/home/u/.cache/gaia/gradle-home"))
    );
    assert_eq!(rewritten[0].1, "/home/u/.cache/gaia/gradle-home");

    let mode = effective_gradle_home(GradleHomeSpec::Workspace, None).expect("config mode");
    assert_eq!(
        redirect_gradle_home(&env, true, mode == GradleHomeSpec::UserCache, Some(cache)).0,
        env
    );
}

#[test]
fn gradle_home_env_override_wins_over_config() {
    assert_eq!(
        effective_gradle_home(GradleHomeSpec::Workspace, Some("user-cache")).expect("env"),
        GradleHomeSpec::UserCache
    );
    assert_eq!(
        effective_gradle_home(GradleHomeSpec::UserCache, Some("workspace")).expect("env"),
        GradleHomeSpec::Workspace
    );
    // An empty override is no override.
    assert_eq!(
        effective_gradle_home(GradleHomeSpec::UserCache, Some("")).expect("empty"),
        GradleHomeSpec::UserCache
    );
}

#[test]
fn gradle_home_env_override_rejects_unknown_values() {
    let error = effective_gradle_home(GradleHomeSpec::Workspace, Some("cache"))
        .expect_err("unknown mode should fail");
    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("GAIA_GRADLE_HOME='cache'"));
}

/// A default-mode Gradle artifact (no `build_args`, no `build_command`)
/// whose `build_env` is `env`.
fn default_gradle_artifact(env: Vec<(String, String)>) -> ArtifactSpec {
    ArtifactSpec::new(
        "java-default-gradle",
        ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
            build_target: "build/libs/app.jar".into(),
            build_args: Vec::new(),
            build_command: Vec::new(),
            build_env: env,
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: "out/app.jar".into(),
        },
    )
}

fn docker_contract(artifact: &ArtifactSpec) -> ArtifactExecutionContract {
    let mut contract = ArtifactExecutionContract::from_spec(
        artifact,
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );
    contract.execution_backend = gaia_artifact_providers::ArtifactExecutionBackend::Docker(
        gaia_artifact_providers::ArtifactDockerExecution {
            image: "gaia-java:test".into(),
            build: None,
        },
    );
    contract
}

#[test]
fn default_gradle_build_redirects_gradle_home_when_configured() {
    let artifact = default_gradle_artifact(vec![(
        "GRADLE_USER_HOME".into(),
        "/ws/.gaia/docker-home/.gradle".into(),
    )]);
    let contract = docker_contract(&artifact);
    let cache = temp_path("gaia-java-default-gradle-cache");
    let mut command = Command::new("gradle");
    command.arg("build").arg("-q");

    apply_java_build_env(
        &mut command,
        &artifact,
        &contract,
        GradleHomeSpec::UserCache,
        Some(&cache),
    );

    let expected = cache.join("gradle-home");
    let home = cache.join("java-home");
    let envs = command.get_envs().collect::<Vec<_>>();
    assert_eq!(
        envs,
        vec![
            (OsStr::new("GRADLE_USER_HOME"), Some(expected.as_os_str())),
            (OsStr::new("HOME"), Some(home.as_os_str())),
        ]
    );
    assert!(expected.is_dir(), "redirected home should be created");
}

#[test]
fn default_build_without_build_env_gets_a_persistent_gradle_home_only_in_user_cache_mode() {
    let cache = temp_path("gaia-java-default-no-env-cache");
    let artifact = default_gradle_artifact(Vec::new());
    let contract = docker_contract(&artifact);

    // Workspace mode: the build's environment is left as it is.
    let mut command = Command::new("gradle");
    apply_java_build_env(
        &mut command,
        &artifact,
        &contract,
        GradleHomeSpec::Workspace,
        Some(&cache),
    );
    assert_eq!(command.get_envs().count(), 0);
    assert!(!cache.join("gradle-home").exists());

    // User-cache mode: without a home of its own, Gradle would use one inside
    // the --rm container and start cold every build.
    let mut command = Command::new("gradle");
    apply_java_build_env(
        &mut command,
        &artifact,
        &contract,
        GradleHomeSpec::UserCache,
        Some(&cache),
    );
    let envs = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        envs,
        [
            (
                "GRADLE_USER_HOME".to_string(),
                Some(cache.join("gradle-home").display().to_string())
            ),
            (
                "HOME".to_string(),
                Some(cache.join("java-home").display().to_string())
            ),
        ]
    );
    assert!(cache.join("gradle-home").is_dir());
    assert!(cache.join("java-home").is_dir());
}

#[test]
fn gradle_wrapper_version_comes_from_properties_file() {
    let dir = temp_path("gaia-java-wrapper-version");
    fs::create_dir_all(dir.join("gradle/wrapper")).expect("wrapper dir");
    fs::write(
        dir.join("gradle/wrapper/gradle-wrapper.properties"),
        "distributionBase=GRADLE_USER_HOME\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-9.1.0-bin.zip\n",
    )
    .expect("properties");

    assert_eq!(
        gradle_wrapper_version_line(&dir),
        "gradle-wrapper distributionUrl=https\\://services.gradle.org/distributions/gradle-9.1.0-bin.zip"
    );
    assert_eq!(
        gradle_wrapper_version_line(&temp_path("gaia-java-no-wrapper")),
        "unavailable"
    );
}

#[test]
fn validate_artifact_rejects_target_override() {
    let mut artifact = ArtifactSpec::new(
        "java-targeted",
        ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
            build_target: "build/libs/app.jar".into(),
            build_args: Vec::new(),
            build_command: Vec::new(),
            build_env: Vec::new(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: "out/app.jar".into(),
        },
    );
    artifact.target = Some("aarch64-unknown-linux-gnu".into());

    let issues = JavaProvider.validate_artifact(&artifact);

    assert!(
        issues
            .iter()
            .any(|issue| issue.code == "java_artifact_target_unsupported")
    );
}

#[test]
fn execute_artifact_rejects_target_override() {
    let mut artifact = ArtifactSpec::new(
        "java-targeted",
        ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
            build_target: "build/libs/app.jar".into(),
            build_args: Vec::new(),
            build_command: Vec::new(),
            build_env: Vec::new(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: "out/app.jar".into(),
        },
    );
    artifact.target = Some("aarch64-unknown-linux-gnu".into());
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(temp_path("gaia-java-provider-src").display().to_string()),
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    let error = JavaProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect_err("targeted java artifact should fail");

    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(
        error
            .message
            .contains("target-aware builds are not supported yet")
    );
}

#[test]
fn persistent_home_keeps_a_build_s_own_home_and_ignores_host_builds() {
    let cache = temp_path("gaia-java-home-cache");
    let own = vec![("HOME".to_string(), "/opt/builder".to_string())];
    assert_eq!(
        persistent_home(&own, true, true, Some(&cache)),
        (own.clone(), None)
    );
    assert_eq!(
        persistent_home(&[], false, true, Some(&cache)),
        (Vec::new(), None)
    );
    assert_eq!(
        persistent_home(&[], true, false, Some(&cache)),
        (Vec::new(), None)
    );
    let workspace = vec![("HOME".to_string(), "/w/.gaia/docker-home".to_string())];
    let (env, dir) = persistent_home(&workspace, true, true, Some(&cache));
    assert_eq!(dir, Some(cache.join("java-home")));
    assert_eq!(
        env,
        [(
            "HOME".to_string(),
            cache.join("java-home").display().to_string()
        )]
    );
}
