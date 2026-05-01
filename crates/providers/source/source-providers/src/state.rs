use super::*;

pub(crate) fn materialized_dir(spec: &ResolvedBuildSpec, source: &SourceSpec) -> PathBuf {
    spec.workspace
        .build_path()
        .join("sources")
        .join(source.id.as_str())
}

pub(crate) fn prepare_materialized_dir(dir: &Path) -> Result<(), SourceProviderError> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|error| {
            SourceProviderError::runtime_state(format!(
                "failed to clear materialized source dir '{}': {error}",
                dir.display()
            ))
        })?;
    }
    fs::create_dir_all(dir).map_err(|error| {
        SourceProviderError::runtime_state(format!(
            "failed to create materialized source dir '{}': {error}",
            dir.display()
        ))
    })
}

pub(crate) fn write_source_marker(
    spec: &ResolvedBuildSpec,
    provider_id: &str,
    source: &SourceSpec,
    materialized_dir: &Path,
    extra: &str,
) -> Result<(), SourceProviderError> {
    let marker_path = materialized_dir.join("source.txt");
    let (refresh_policy, pin_policy) = source_policy(source);
    let execution_backend = if spec.policy.execution.docker.is_some() {
        "docker"
    } else {
        "host"
    };
    let execution_backend_image = spec
        .policy
        .execution
        .docker
        .as_ref()
        .map(|docker| docker.image.as_str())
        .unwrap_or_default();
    let mut state = gaia_spec::KeyValueState::new()
        .with("provider", provider_id)
        .with("source", source.id.as_str())
        .with("execution_backend", execution_backend)
        .with("execution_backend_image", execution_backend_image)
        .with(
            "build_version",
            spec.identity.version.as_deref().unwrap_or_default(),
        )
        .with(
            "build_branch",
            spec.metadata.branch.as_deref().unwrap_or_default(),
        )
        .with(
            "build_target",
            spec.metadata.target.as_deref().unwrap_or_default(),
        )
        .with(
            "build_profile",
            spec.metadata.profile.as_deref().unwrap_or_default(),
        )
        .with("refresh_policy", refresh_policy.as_str())
        .with("pin_policy", pin_policy.as_str())
        .with("resolved_refresh_decision", refresh_policy.as_str())
        .with("resolved_pin_decision", pin_policy.as_str());
    state.extend_pairs(gaia_spec::KeyValueState::parse(extra).into_map());
    let state = state.render();
    fs::write(&marker_path, &state).map_err(|error| {
        SourceProviderError::runtime_state(format!(
            "failed to write source marker '{}': {error}",
            marker_path.display()
        ))
    })?;
    let state_path = materialized_dir.join(".gaia-source-state.txt");
    fs::write(&state_path, state).map_err(|error| {
        SourceProviderError::runtime_state(format!(
            "failed to write source state '{}': {error}",
            state_path.display()
        ))
    })
}

pub(crate) fn source_policy(source: &SourceSpec) -> (SourceRefreshPolicySpec, SourcePinPolicySpec) {
    match &source.definition {
        SourceDefinition::Git(git) => (git.refresh_policy, git.pin_policy),
        SourceDefinition::Path(path) => (path.refresh_policy, path.pin_policy),
        SourceDefinition::Archive(archive) => (archive.refresh_policy, archive.pin_policy),
        SourceDefinition::Download(download) => (download.refresh_policy, download.pin_policy),
    }
}

pub(crate) fn resolve_workspace_path(
    workspace: &WorkspaceSpec,
    path: &str,
) -> Result<PathBuf, SourceProviderError> {
    let resolved = gaia_spec::resolve_workspace_path(workspace, path).map_err(|error| {
        SourceProviderError::output_missing(format!(
            "failed to resolve source path '{path}': {error}"
        ))
    })?;
    fs::canonicalize(&resolved).map_err(|error| {
        SourceProviderError::output_missing(format!(
            "failed to resolve path source '{}': {error}",
            resolved.display()
        ))
    })
}

pub(crate) fn create_symlink_or_manifest(
    source_path: &Path,
    link_path: &Path,
) -> Result<(), SourceProviderError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs as unix_fs;
        if unix_fs::symlink(source_path, link_path).is_ok() {
            return Ok(());
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs as windows_fs;
        let result = if source_path.is_dir() {
            windows_fs::symlink_dir(source_path, link_path)
        } else {
            windows_fs::symlink_file(source_path, link_path)
        };
        if result.is_ok() {
            return Ok(());
        }
    }

    let manifest_path = link_path.with_extension("txt");
    fs::write(&manifest_path, source_path.display().to_string()).map_err(|error| {
        SourceProviderError::runtime_state(format!(
            "failed to write path source manifest '{}': {error}",
            manifest_path.display()
        ))
    })
}
