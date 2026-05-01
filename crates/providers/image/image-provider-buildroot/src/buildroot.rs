use super::*;

pub(crate) fn resolve_buildroot_dir(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Option<PathBuf> {
    if let Some(source_id) = buildroot_source_id(image) {
        let candidate = buildroot_source_dir(spec, source_id);
        if candidate.join("Makefile").is_file() {
            return Some(candidate);
        }
    }
    for key in ["GAIA_BUILDROOT_DIR", "BUILDROOT_DIR"] {
        if let Some(candidate) = env::var_os(key).map(PathBuf::from)
            && candidate.join("Makefile").is_file()
        {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn buildroot_source_id(image: &ImageSpec) -> Option<&SourceId> {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.source.as_ref(),
        _ => None,
    }
}

pub(crate) fn buildroot_allow_fallback(image: &ImageSpec) -> bool {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.allow_fallback,
        _ => false,
    }
}

pub(crate) fn buildroot_source_dir(spec: &ResolvedBuildSpec, source_id: &SourceId) -> PathBuf {
    Path::new(&spec.workspace.root_dir)
        .join(&spec.workspace.build_dir)
        .join("sources")
        .join(source_id.as_str())
}

#[derive(Clone)]
pub(crate) struct ImageCommandContext<'a> {
    pub(crate) execution: &'a ImageExecutionContext,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

pub(crate) struct BuildrootRunRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) command: ImageCommandContext<'a>,
}

pub(crate) fn run_buildroot(
    request: BuildrootRunRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootRunRequest {
        spec,
        image,
        buildroot_dir,
        output_dir,
        command: command_context,
    } = request;
    fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot output dir '{}': {error}",
            output_dir.display()
        ))
    })?;
    let mut messages = Vec::new();

    let (defconfig, defconfig_path, config_fragments, config_overrides, external_tree) =
        match &image.definition {
            ImageDefinition::Buildroot(buildroot) => (
                buildroot.defconfig.as_deref(),
                buildroot.defconfig_path.as_deref(),
                buildroot.config_fragments.as_slice(),
                buildroot.config_overrides.as_slice(),
                buildroot.external_tree.as_deref(),
            ),
            _ => (None, None, &[][..], &[][..], None),
        };

    if let Some(defconfig_path) = defconfig_path {
        let resolved_defconfig_path = resolve_workspace_path(
            &ResolvedBuildSpec {
                workspace: spec.workspace.clone(),
                ..spec.clone()
            },
            defconfig_path,
        )?;
        materialize_defconfig_support_files(&resolved_defconfig_path, output_dir)?;
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg("defconfig")
            .arg(format!(
                "BR2_DEFCONFIG={}",
                resolved_defconfig_path.display()
            ))
            .current_dir(buildroot_dir);
        if let Some(external_tree) = external_tree {
            command.env("BR2_EXTERNAL", external_tree);
        }
        messages.extend(run_command(
            command,
            "buildroot defconfig",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if !config_fragments.is_empty() {
            messages.extend(apply_buildroot_config_fragments(
                spec,
                buildroot_dir,
                output_dir,
                config_fragments,
                external_tree,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
    } else if let Some(defconfig) = defconfig {
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg(defconfig)
            .current_dir(buildroot_dir);
        if let Some(external_tree) = external_tree {
            command.env("BR2_EXTERNAL", external_tree);
        }
        messages.extend(run_command(
            command,
            "buildroot defconfig",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if !config_fragments.is_empty() {
            messages.extend(apply_buildroot_config_fragments(
                spec,
                buildroot_dir,
                output_dir,
                config_fragments,
                external_tree,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
    } else if !config_fragments.is_empty() || !config_overrides.is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "buildroot config_fragments/config_overrides require defconfig or defconfig_path",
        ));
    }

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .current_dir(buildroot_dir);
    append_make_jobs(&mut command, command_context.policy.local_jobs);
    if let Some(external_tree) = external_tree {
        command.env("BR2_EXTERNAL", external_tree);
    }
    messages.extend(run_command(
        command,
        "buildroot make",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )?);
    Ok(messages)
}

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
    if jobs == 0 {
        return;
    }
    let jobs = usize::try_from(jobs).unwrap_or(1);
    command.arg(format!("-j{jobs}"));
}
