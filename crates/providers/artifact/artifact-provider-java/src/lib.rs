use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactPlan,
    ArtifactProvider, ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    command_version_line, copy_artifact_file_to_output, materialize_artifact_marker_and_state,
    render_artifact_backend_state, run_command_with_retries,
};
use gaia_process::register_docker_mount;
use gaia_spec::{ArtifactDefinition, ArtifactSpec, GradleHomeSpec, ResolvedBuildSpec};
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
        let gradle_home = effective_gradle_home(
            contract.gradle_home,
            std::env::var(GRADLE_HOME_SETTING_ENV).ok().as_deref(),
        )?;
        let (build_tool, mut messages) = run_java_build(
            artifact,
            source_dir,
            contract,
            gradle_home,
            log_sink,
            cancel_check,
        )?;
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
    gradle_home: GradleHomeSpec,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(String, Vec<String>), ArtifactProviderError> {
    let source_dir = Path::new(source_dir);
    let user_cache = gaia_spec::user_cache_root();
    if let ArtifactDefinition::Java(java) = &artifact.definition
        && let Some((program, args)) = java.build_command.split_first()
    {
        let mut command = Command::new(program);
        command.args(args);
        apply_java_build_env(
            &mut command,
            artifact,
            contract,
            gradle_home,
            user_cache.as_deref(),
        );
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
        } else {
            command.arg("-q").arg("-DskipTests").arg("package");
        }
        apply_java_build_env(
            &mut command,
            artifact,
            contract,
            gradle_home,
            user_cache.as_deref(),
        );
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
        } else {
            command.arg("build").arg("-q");
        }
        apply_java_build_env(
            &mut command,
            artifact,
            contract,
            gradle_home,
            user_cache.as_deref(),
        );
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
        } else {
            command.arg("build").arg("-q");
        }
        apply_java_build_env(
            &mut command,
            artifact,
            contract,
            gradle_home,
            user_cache.as_deref(),
        );
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

/// Deprecated environment override for `[providers.java] gradle_home`. When
/// set to `workspace` or `user-cache` it wins over the configured value; any
/// other value fails the build rather than being ignored.
const GRADLE_HOME_SETTING_ENV: &str = "GAIA_GRADLE_HOME";

/// The Gradle home mode a build uses: the `GAIA_GRADLE_HOME` override when
/// it is set and non-empty, else the configured `gradle_home`.
fn effective_gradle_home(
    configured: GradleHomeSpec,
    env_override: Option<&str>,
) -> Result<GradleHomeSpec, ArtifactProviderError> {
    match env_override.filter(|text| !text.is_empty()) {
        None => Ok(configured),
        Some(text) => GradleHomeSpec::parse(text).ok_or_else(|| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "{GRADLE_HOME_SETTING_ENV}='{text}' is not a Gradle home mode; \
                     use 'workspace' or 'user-cache' (or set [providers.java] gradle_home)"
                ),
            )
        }),
    }
}

/// Applies the artifact's `build_env` (and the Gradle home redirect) to a
/// build command. Every Java build branch goes through here, so the same
/// environment applies whether or not the build sets its own arguments.
fn apply_java_build_env(
    command: &mut Command,
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    gradle_home: GradleHomeSpec,
    user_cache: Option<&Path>,
) {
    let env: &[(String, String)] = match &artifact.definition {
        ArtifactDefinition::Java(java) => &java.build_env,
        _ => &[],
    };
    apply_build_env(command, env, contract, gradle_home, user_cache);
}

fn apply_build_env(
    command: &mut Command,
    env: &[(String, String)],
    contract: &ArtifactExecutionContract,
    gradle_home: GradleHomeSpec,
    user_cache: Option<&Path>,
) {
    let docker = matches!(
        contract.execution_backend,
        ArtifactExecutionBackend::Docker(_)
    );
    let user_cache_requested = gradle_home == GradleHomeSpec::UserCache;
    let (env, redirected) = redirect_gradle_home(env, docker, user_cache_requested, user_cache);
    let (env, home) = persistent_home(&env, docker, user_cache_requested, user_cache);
    for dir in redirected.iter().chain(&home) {
        // The container mounts these at their host paths, so they are
        // visible there under the same paths. If a directory cannot be
        // created, the build reports the failure itself.
        let _ = std::fs::create_dir_all(dir);
        register_docker_mount(dir);
    }
    for (key, value) in &env {
        command.env(key, value);
    }
}

/// Points `GRADLE_USER_HOME` at `<user cache>/gradle-home` when the build's
/// Gradle home mode is `user-cache` and the build runs in Docker, unless the
/// build sets its own home outside the workspace's `.gaia/docker-home`. A
/// build that sets no home would otherwise use one inside the `--rm`
/// container and start with a cold cache every time. Returns the environment
/// and the directory to mount, if any.
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
    let home = env
        .iter()
        .find(|(key, _)| key == "GRADLE_USER_HOME")
        .map(|(_, value)| value);
    if home.is_some_and(|home| !home.contains("/.gaia/docker-home")) {
        return (unchanged(), None);
    }
    let dir = cache.join("gradle-home");
    let value = dir.display().to_string();
    let mut rewritten = env
        .iter()
        .map(|(key, existing)| {
            if key == "GRADLE_USER_HOME" {
                (key.clone(), value.clone())
            } else {
                (key.clone(), existing.clone())
            }
        })
        .collect::<Vec<_>>();
    if home.is_none() {
        rewritten.push(("GRADLE_USER_HOME".to_string(), value));
    }
    (rewritten, Some(dir))
}

/// Gives a Docker build in `user-cache` mode a persistent `HOME`
/// (`<user cache>/java-home`), unless the build sets its own outside the
/// workspace's `.gaia/docker-home`. Tools a Java build runs keep their caches
/// under `HOME` (pnpm's store, npm, Python virtualenvs); inside the `--rm`
/// container they would start cold on every build. Returns the environment
/// and the directory to mount, if any.
fn persistent_home(
    env: &[(String, String)],
    docker: bool,
    user_cache_requested: bool,
    user_cache: Option<&Path>,
) -> (Vec<(String, String)>, Option<PathBuf>) {
    let (Some(cache), true) = (user_cache, docker && user_cache_requested) else {
        return (env.to_vec(), None);
    };
    let home = env
        .iter()
        .find(|(key, _)| key == "HOME")
        .map(|(_, value)| value);
    if home.is_some_and(|home| !home.contains("/.gaia/docker-home")) {
        return (env.to_vec(), None);
    }
    let dir = cache.join("java-home");
    let value = dir.display().to_string();
    let mut rewritten = env
        .iter()
        .map(|(key, existing)| {
            if key == "HOME" {
                (key.clone(), value.clone())
            } else {
                (key.clone(), existing.clone())
            }
        })
        .collect::<Vec<_>>();
    if home.is_none() {
        rewritten.push(("HOME".to_string(), value));
    }
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
mod tests;
