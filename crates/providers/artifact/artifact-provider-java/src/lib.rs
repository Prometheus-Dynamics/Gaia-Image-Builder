use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactPlan,
    ArtifactProvider, ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    command_version_line, copy_artifact_file_to_output, materialize_artifact_marker_and_state,
    render_artifact_backend_state, run_command_with_retries,
};
use gaia_process::register_docker_mount;
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct JavaProvider;

impl ArtifactProvider for JavaProvider {
    fn id(&self) -> &'static str {
        "artifact.java"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Java
    }

    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }

    fn plan_artifact(&self, artifact: &ArtifactSpec) -> ArtifactPlan {
        ArtifactPlan {
            operations: vec![ArtifactProviderOperation::Build],
            contract: ArtifactExecutionContract::from_spec(
                artifact,
                None,
                false,
                ArtifactExecutionContract::default_command_policy(),
                gaia_spec::OutputRetentionPolicySpec::default(),
            ),
        }
    }

    fn validate_artifact(&self, artifact: &ArtifactSpec) -> Vec<ArtifactProviderValidationIssue> {
        let mut issues = Vec::new();
        if let ArtifactDefinition::Java(java) = &artifact.definition
            && java.build_target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "java_build_target_empty",
                message: "java build_target cannot be empty".into(),
            });
        }
        if let ArtifactDefinition::Java(java) = &artifact.definition
            && java.build_command.iter().any(|arg| arg.trim().is_empty())
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "java_build_command_empty_arg",
                message: "java build_command entries cannot be empty".into(),
            });
        }
        if let Some(target) = &artifact.target
            && !target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "java_artifact_target_unsupported",
                message: format!(
                    "java artifact target '{}' is not supported; java artifacts are currently host-built only",
                    target
                ),
            });
        }
        issues
    }

    fn execute_artifact(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        reject_unsupported_artifact_target(artifact)?;
        let build_target = match &artifact.definition {
            ArtifactDefinition::Java(java) => java.build_target.clone(),
            _ => artifact.id.as_str().to_string(),
        };
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let (build_tool, mut messages) =
            run_java_build(artifact, source_dir, contract, log_sink, cancel_check)?;
        let built_path = resolve_java_built_path(source_dir, &build_target)?;
        let output_path = artifact_output_path(contract, source_dir);
        copy_artifact_file_to_output(&built_path, &output_path, "built java artifact")?;
        write_marker(self.id(), artifact, contract, &build_target, &build_tool)?;
        messages.push(format!(
            "java artifact '{}' built target '{}' -> {}",
            artifact.id.as_str(),
            build_target,
            contract.output.path
        ));
        Ok(messages)
    }
}

fn reject_unsupported_artifact_target(
    artifact: &ArtifactSpec,
) -> Result<(), ArtifactProviderError> {
    if let Some(target) = &artifact.target
        && !target.trim().is_empty()
    {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "java artifact '{}' declared target '{}', but java target-aware builds are not supported yet",
                artifact.id.as_str(),
                target
            ),
        ));
    }
    Ok(())
}

fn run_java_build(
    artifact: &ArtifactSpec,
    source_dir: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(String, Vec<String>), ArtifactProviderError> {
    let source_dir = Path::new(source_dir);
    if let ArtifactDefinition::Java(java) = &artifact.definition
        && let Some((program, args)) = java.build_command.split_first()
    {
        let mut command = Command::new(program);
        command.args(args);
        apply_build_env(&mut command, &java.build_env, contract);
        command.current_dir(source_dir);
        return Ok((
            "custom-command".to_string(),
            run_command(
                command,
                "java custom build command",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }
    let custom = match &artifact.definition {
        ArtifactDefinition::Java(java) if !java.build_args.is_empty() => Some(java),
        _ => None,
    };
    if source_dir.join("pom.xml").is_file() {
        let mut command = Command::new("mvn");
        if let Some(java) = custom {
            command.args(&java.build_args);
            apply_build_env(&mut command, &java.build_env, contract);
        } else {
            command.arg("-q").arg("-DskipTests").arg("package");
        }
        command.current_dir(source_dir);
        return Ok((
            "maven".to_string(),
            run_command(
                command,
                "maven package",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    if source_dir.join("gradlew").is_file() {
        let mut command = Command::new(source_dir.join("gradlew"));
        if let Some(java) = custom {
            command.args(&java.build_args);
            apply_build_env(&mut command, &java.build_env, contract);
        } else {
            command.arg("build").arg("-q");
        }
        command.current_dir(source_dir);
        return Ok((
            "gradle-wrapper".to_string(),
            run_command(
                command,
                "gradle wrapper build",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    if source_dir.join("build.gradle").is_file() || source_dir.join("build.gradle.kts").is_file() {
        let mut command = Command::new("gradle");
        if let Some(java) = custom {
            command.args(&java.build_args);
            apply_build_env(&mut command, &java.build_env, contract);
        } else {
            command.arg("build").arg("-q");
        }
        command.current_dir(source_dir);
        return Ok((
            "gradle".to_string(),
            run_command(
                command,
                "gradle build",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    Err(ArtifactProviderError::new(
        ArtifactProviderErrorKind::PolicyBlocked,
        format!(
            "java source '{}' did not contain a supported build file (pom.xml, gradlew, build.gradle, build.gradle.kts)",
            source_dir.display()
        ),
    ))
}

/// Environment variable choosing where Gradle's user home (dependency and
/// wrapper caches) lives for Docker-built Java artifacts: `user-cache`
/// moves it to the per-user Gaia cache; unset or `workspace` keeps it in
/// `<workspace>/.gaia/docker-home`.
const GRADLE_HOME_SETTING_ENV: &str = "GAIA_GRADLE_HOME";
const GRADLE_HOME_USER_CACHE: &str = "user-cache";

fn apply_build_env(
    command: &mut Command,
    env: &[(String, String)],
    contract: &ArtifactExecutionContract,
) {
    let docker = matches!(
        contract.execution_backend,
        ArtifactExecutionBackend::Docker(_)
    );
    let user_cache_setting = std::env::var(GRADLE_HOME_SETTING_ENV).ok();
    let user_cache = gaia_spec::user_cache_root();
    let (env, redirected) = redirect_gradle_home(
        env,
        docker,
        user_cache_setting.as_deref() == Some(GRADLE_HOME_USER_CACHE),
        user_cache.as_deref(),
    );
    if let Some(dir) = &redirected {
        // The container mounts Gradle's home at its host path, so the
        // redirected home is visible there under the same path. If the
        // directory cannot be created, Gradle reports the failure itself.
        let _ = std::fs::create_dir_all(dir);
        register_docker_mount(dir);
    }
    for (key, value) in &env {
        command.env(key, value);
    }
}

/// Points `GRADLE_USER_HOME` at `<user cache>/gradle-home` when the user
/// asked for it, the build runs in Docker and the spec puts Gradle's home in
/// the workspace's `.gaia/docker-home`. Returns the rewritten environment and
/// the directory to mount, if any. Every other case keeps the spec's values.
fn redirect_gradle_home(
    env: &[(String, String)],
    docker: bool,
    user_cache_requested: bool,
    user_cache: Option<&Path>,
) -> (Vec<(String, String)>, Option<PathBuf>) {
    let unchanged = || env.to_vec();
    if !docker || !user_cache_requested {
        return (unchanged(), None);
    }
    let Some(cache) = user_cache else {
        return (unchanged(), None);
    };
    let Some(home) = env
        .iter()
        .find(|(key, _)| key == "GRADLE_USER_HOME")
        .map(|(_, value)| value)
    else {
        return (unchanged(), None);
    };
    if !home.contains("/.gaia/docker-home") {
        return (unchanged(), None);
    }
    let dir = cache.join("gradle-home");
    let rewritten = env
        .iter()
        .map(|(key, value)| {
            if key == "GRADLE_USER_HOME" {
                (key.clone(), dir.display().to_string())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect();
    (rewritten, Some(dir))
}

/// Records the Gradle wrapper's distribution from its properties file. Running
/// `gradlew --version` here would start a second Gradle on the host (and
/// could download the distribution) just to name a version the wrapper file
/// already pins.
fn gradle_wrapper_version_line(source_dir: &Path) -> String {
    let properties = source_dir.join("gradle/wrapper/gradle-wrapper.properties");
    let Ok(contents) = std::fs::read_to_string(properties) else {
        return "unavailable".to_string();
    };
    contents
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("distributionUrl="))
        .map(|url| format!("gradle-wrapper distributionUrl={}", url.trim()))
        .unwrap_or_else(|| "unavailable".to_string())
}

fn resolve_java_built_path(
    source_dir: &str,
    build_target: &str,
) -> Result<PathBuf, ArtifactProviderError> {
    let target = PathBuf::from(build_target);
    let candidates = [
        if target.is_absolute() {
            target.clone()
        } else {
            PathBuf::from(source_dir).join(&target)
        },
        PathBuf::from(source_dir).join("target").join(build_target),
        PathBuf::from(source_dir)
            .join("build")
            .join("libs")
            .join(build_target),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| ArtifactProviderError::new(
                ArtifactProviderErrorKind::OutputMissing,
                format!(
                "java build completed but built target '{}' was not found in expected locations",
                build_target
            )))
}

fn run_command(
    command: Command,
    label: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    run_command_with_retries(&command, contract, label, log_sink, cancel_check)?;
    Ok(Vec::new())
}

fn write_marker(
    provider_id: &str,
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    build_target: &str,
    build_tool: &str,
) -> Result<(), ArtifactProviderError> {
    materialize_artifact_marker_and_state(
        contract,
        &format!(
            "provider={provider_id}\nartifact={}\nbuild_target={build_target}\n",
            artifact.id.as_str()
        ),
        &artifact_state_contents(
            provider_id,
            artifact.id.as_str(),
            contract,
            build_target,
            build_tool,
        ),
    )
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    build_target: &str,
    build_tool: &str,
) -> String {
    let build_tool_version = match build_tool {
        "maven" => command_version_line("mvn", &["-version"]),
        "gradle-wrapper" => {
            let source_dir = contract.source_dir.as_deref().unwrap_or(".");
            gradle_wrapper_version_line(Path::new(source_dir))
        }
        _ => command_version_line("gradle", &["--version"]),
    };
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "build-target",
        resolved_identifier: build_target,
        output_class: "jar",
        build_tool,
        build_tool_version: &build_tool_version,
        extra_fields: &[("build_target".to_string(), build_target.to_string())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
