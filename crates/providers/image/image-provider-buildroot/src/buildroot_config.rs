//! Applying `.config` changes: fragments, `config_overrides`, the download
//! and compiler cache settings, and the environment every `make` gets.
use super::*;

pub(crate) struct BuildrootConfigOverrideRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) output_dir: &'a Path,
    pub(crate) overrides: &'a [(String, String)],
    pub(crate) external_tree: Option<&'a str>,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) command: ImageCommandContext<'a>,
}

pub(crate) fn apply_buildroot_config_overrides(
    request: BuildrootConfigOverrideRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootConfigOverrideRequest {
        spec,
        output_dir,
        overrides,
        external_tree,
        buildroot_dir,
        command: command_context,
    } = request;
    let config_path = output_dir.join(".config");
    if !config_path.is_file() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "buildroot config overrides require an existing '{}'",
                config_path.display()
            ),
        ));
    }

    let mut merged = fs::read_to_string(&config_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;
    let normalized_overrides = normalize_buildroot_config_overrides(spec, overrides);
    merged = merge_buildroot_config_assignments(&merged, &normalized_overrides);
    fs::write(&config_path, merged).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write overridden buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("olddefconfig")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(external_tree) = external_tree {
        command.env("BR2_EXTERNAL", external_tree);
    }
    run_command(
        command,
        "buildroot olddefconfig",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )
}

pub(crate) fn merge_buildroot_config_assignments(
    base: &str,
    overrides: &[(String, String)],
) -> String {
    let override_keys = overrides
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut merged = String::new();

    for line in base.lines() {
        let replaces_assignment = line
            .split_once('=')
            .is_some_and(|(key, _)| override_keys.contains(key));
        let replaces_unset = line
            .strip_prefix("# ")
            .and_then(|line| line.strip_suffix(" is not set"))
            .is_some_and(|key| override_keys.contains(key));
        if !replaces_assignment && !replaces_unset {
            merged.push_str(line);
            merged.push('\n');
        }
    }

    for (key, value) in overrides {
        merged.push_str(key);
        merged.push('=');
        merged.push_str(value);
        merged.push('\n');
    }

    merged
}

pub(crate) fn normalize_buildroot_config_overrides(
    spec: &ResolvedBuildSpec,
    overrides: &[(String, String)],
) -> Vec<(String, String)> {
    overrides
        .iter()
        .map(|(key, value)| {
            if key == "BR2_GLOBAL_PATCH_DIR" {
                (key.clone(), normalize_global_patch_dir_value(spec, value))
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

pub(crate) fn normalize_global_patch_dir_value(spec: &ResolvedBuildSpec, value: &str) -> String {
    let Some(unquoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_string();
    };
    let workspace_root = Path::new(&spec.workspace.root_dir);
    let normalized = unquoted
        .split_whitespace()
        .map(|entry| {
            let path = Path::new(entry);
            if path.is_absolute() {
                return entry.to_string();
            }
            let workspace_path = workspace_root.join(path);
            if workspace_path.exists() {
                workspace_path.display().to_string()
            } else {
                entry.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("\"{normalized}\"")
}

pub(crate) fn apply_buildroot_config_fragments(
    spec: &ResolvedBuildSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    fragments: &[String],
    external_tree: Option<&str>,
    command_context: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let config_path = output_dir.join(".config");
    if !config_path.is_file() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "buildroot config fragments require an existing '{}'",
                config_path.display()
            ),
        ));
    }

    let mut merged = fs::read_to_string(&config_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    for fragment in fragments {
        let resolved = resolve_workspace_path(spec, fragment)?;
        let fragment_contents = fs::read_to_string(&resolved).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read buildroot config fragment '{}': {error}",
                    resolved.display()
                ),
            )
        })?;
        merged.push('\n');
        merged.push_str(&fragment_contents);
        if !fragment_contents.ends_with('\n') {
            merged.push('\n');
        }
    }
    fs::write(&config_path, merged).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write merged buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("olddefconfig")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(external_tree) = external_tree {
        command.env("BR2_EXTERNAL", external_tree);
    }
    run_command(
        command,
        "buildroot olddefconfig",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )
}

pub(crate) fn materialize_defconfig_support_files(
    defconfig_path: &Path,
    output_dir: &Path,
) -> Result<(), ImageProviderError> {
    let Some(defconfig_dir) = defconfig_path.parent() else {
        return Ok(());
    };
    copy_dir_contents(defconfig_dir, output_dir, Some(defconfig_path))
}

pub(crate) fn copy_dir_contents(
    src_dir: &Path,
    dest_dir: &Path,
    skip_file: Option<&Path>,
) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot support dir '{}': {error}",
            dest_dir.display()
        ))
    })?;
    for entry in fs::read_dir(src_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read buildroot support dir '{}': {error}",
            src_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read buildroot support entry in '{}': {error}",
                src_dir.display()
            ))
        })?;
        let path = entry.path();
        if skip_file.is_some_and(|skip| path == skip) {
            continue;
        }
        let dest = dest_dir.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read file type for '{}' : {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            copy_dir_contents(&path, &dest, None)?;
        } else if file_type.is_file() {
            fs::create_dir_all(dest.parent().unwrap_or(dest_dir)).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to create buildroot support parent dir '{}': {error}",
                    dest.parent().unwrap_or(dest_dir).display()
                ))
            })?;
            fs::copy(&path, &dest).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to copy buildroot support file '{}' to '{}': {error}",
                    path.display(),
                    dest.display()
                ))
            })?;
        }
    }
    Ok(())
}

pub(crate) fn append_make_jobs(command: &mut Command, jobs: u32) {
    let jobs = if jobs == 0 {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    } else {
        usize::try_from(jobs).unwrap_or(1)
    };
    command.arg(format!("-j{jobs}"));
}

pub(crate) fn apply_buildroot_cache_config(
    spec: &ResolvedBuildSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    external_tree: Option<&str>,
    command_context: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let config_path = output_dir.join(".config");
    if !config_path.is_file() {
        return Ok(Vec::new());
    }
    let mut overrides = Vec::new();
    if let Some(download_dir) = buildroot_download_dir(spec, command_context.policy)? {
        overrides.push(("BR2_DL_DIR".to_string(), quoted_path_value(&download_dir)?));
    }
    if command_context.policy.ccache_enabled {
        overrides.push(("BR2_CCACHE".to_string(), "y".to_string()));
        if let Some(ccache_dir) = buildroot_ccache_dir(spec, command_context.policy)? {
            overrides.push((
                "BR2_CCACHE_DIR".to_string(),
                quoted_path_value(&ccache_dir)?,
            ));
        }
    }
    if overrides.is_empty() {
        return Ok(Vec::new());
    }

    let original = fs::read_to_string(&config_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;
    let merged = merge_buildroot_config_assignments(&original, &overrides);
    if merged == original {
        return Ok(Vec::new());
    }
    fs::write(&config_path, merged).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write buildroot cache config '{}': {error}",
                config_path.display()
            ),
        )
    })?;

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("olddefconfig")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(external_tree) = external_tree {
        command.env("BR2_EXTERNAL", external_tree);
    }
    let mut messages = run_command(
        command,
        "buildroot cache config olddefconfig",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )?;
    messages.push("applied buildroot download/compiler cache configuration".to_string());
    Ok(messages)
}

const DEFAULT_BUILDROOT_DOWNLOAD_DIR: &str = ".gaia/cache/buildroot/dl";

pub(crate) fn apply_buildroot_policy_env(
    command: &mut Command,
    spec: &ResolvedBuildSpec,
    policy: &ImageExecutionPolicy,
) -> Result<(), ImageProviderError> {
    // Without an explicit download_dir, Buildroot would download into its own
    // source tree, which is deleted whenever the source is re-materialized.
    // Default to a workspace-wide cache shared by every build. It is passed
    // only through the environment (which Buildroot honors over .config), so
    // existing output trees see no config change.
    let download_dir = match buildroot_download_dir(spec, policy)? {
        Some(download_dir) => download_dir,
        None => ensure_cache_dir(spec, DEFAULT_BUILDROOT_DOWNLOAD_DIR, "buildroot download")?,
    };
    command.env("BR2_DL_DIR", download_dir);
    if policy.ccache_enabled
        && let Some(ccache_dir) = buildroot_ccache_dir(spec, policy)?
    {
        command.env("BR2_CCACHE_DIR", ccache_dir);
    }
    Ok(())
}

fn buildroot_download_dir(
    spec: &ResolvedBuildSpec,
    policy: &ImageExecutionPolicy,
) -> Result<Option<PathBuf>, ImageProviderError> {
    policy
        .download_dir
        .as_deref()
        .map(|path| ensure_cache_dir(spec, path, "buildroot download"))
        .transpose()
}

fn buildroot_ccache_dir(
    spec: &ResolvedBuildSpec,
    policy: &ImageExecutionPolicy,
) -> Result<Option<PathBuf>, ImageProviderError> {
    policy
        .ccache_dir
        .as_deref()
        .map(|path| ensure_cache_dir(spec, path, "buildroot ccache"))
        .transpose()
}

fn ensure_cache_dir(
    spec: &ResolvedBuildSpec,
    path: &str,
    label: &str,
) -> Result<PathBuf, ImageProviderError> {
    let raw = Path::new(path);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        Path::new(&spec.workspace.root_dir).join(raw)
    };
    fs::create_dir_all(&resolved).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create {label} dir '{}': {error}",
                resolved.display()
            ),
        )
    })?;
    Ok(fs::canonicalize(&resolved).unwrap_or(resolved))
}

fn quoted_path_value(path: &Path) -> Result<String, ImageProviderError> {
    kconfig_string_value(&path.display().to_string())
}

pub(crate) fn kconfig_string_value(value: &str) -> Result<String, ImageProviderError> {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' | '\r' => {
                return Err(ImageProviderError::backend_command(
                    "Buildroot Kconfig string values cannot contain newline characters",
                ));
            }
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    Ok(escaped)
}
