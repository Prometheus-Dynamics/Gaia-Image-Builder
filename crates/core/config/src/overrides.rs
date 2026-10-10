use crate::{ConfigError, ResolveOptions, raw};

mod keys;

pub(crate) use keys::source_path_override_id;
use keys::{KnownOverrideKey, OverrideKey};

pub(crate) fn apply_cli_overrides(
    mut raw: raw::RawBuildConfig,
    options: &ResolveOptions,
) -> Result<raw::RawBuildConfig, ConfigError> {
    for env_file in &options.env_files {
        if !raw.env_files.contains(env_file) {
            raw.env_files.push(env_file.clone());
        }
    }
    for (key, value) in &options.env_overrides {
        raw.env.insert(key.clone(), value.clone());
    }
    for (key, value) in &options.explicit_overrides {
        apply_override(&mut raw, key, value)?;
    }
    if raw.source_path.is_some()
        && raw
            .requested_build
            .as_deref()
            .is_some_and(|requested_build| requested_build != raw.build_name)
    {
        raw.env
            .entry("GAIA_REQUESTED_BUILD".into())
            .or_insert_with(|| raw.requested_build.clone().unwrap_or_default());
    }
    raw.selected_inputs = collect_selected_inputs(&raw);
    Ok(raw)
}

pub(crate) fn apply_selected_preset(
    mut raw: raw::RawBuildConfig,
) -> Result<raw::RawBuildConfig, ConfigError> {
    let Some(selected_preset) = raw.preset.clone() else {
        return Ok(raw);
    };

    let preset = raw
        .presets
        .get(&selected_preset)
        .cloned()
        .ok_or(ConfigError::MissingPreset {
            preset: selected_preset,
        })?;

    for env_file in preset.env_files {
        if !raw.env_files.contains(&env_file) {
            raw.env_files.push(env_file);
        }
    }
    raw.env.extend(preset.env);
    for (key, value) in preset.overrides {
        apply_override(&mut raw, &key, &value)?;
    }

    raw.selected_inputs = collect_selected_inputs(&raw);

    Ok(raw)
}

fn apply_override(
    raw: &mut raw::RawBuildConfig,
    key: &str,
    value: &str,
) -> Result<(), ConfigError> {
    match OverrideKey::parse(key) {
        OverrideKey::Known(known) => apply_known_override(raw, known, key, value)?,
        OverrideKey::Input(name) => {
            if raw.inputs.contains_key(name) {
                upsert_pair(&mut raw.selected_inputs, name, value);
            }
        }
        OverrideKey::Env(env_key) => {
            raw.env.insert(env_key.to_string(), value.to_string());
        }
        OverrideKey::InterpolationValue(name) => {
            upsert_pair(&mut raw.interpolation.values, name, value);
        }
        OverrideKey::BuildLabel(name) => {
            upsert_pair(&mut raw.labels, name, value);
        }
        OverrideKey::ProvenanceIdentityLabel(name) => {
            upsert_pair(&mut raw.provenance.identity.labels, name, value);
        }
        OverrideKey::WorkspacePath(name) => {
            upsert_workspace_named_path(
                &mut raw.workspace.named_paths,
                name,
                value,
                raw::RawWorkspacePathKind::Host,
            );
        }
        OverrideKey::BuildrootConfigOverride(name) => {
            if let raw::RawImageDefinition::Buildroot {
                config_overrides, ..
            } = &mut raw.image.definition
            {
                upsert_pair(config_overrides, name, value);
            }
        }
        OverrideKey::SourcePath(id) => {
            // The source keeps its id but materializes from the local
            // directory, which import resolution also reads from.
            let source = raw
                .sources
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| {
                    ConfigError::invalid_override_value(key, value, "the id of a declared source")
                })?;
            let refresh = match &source.definition {
                raw::RawSourceDefinition::Git { refresh, .. }
                | raw::RawSourceDefinition::Path { refresh, .. }
                | raw::RawSourceDefinition::Archive { refresh, .. }
                | raw::RawSourceDefinition::Download { refresh, .. } => *refresh,
            };
            source.definition = raw::RawSourceDefinition::Path {
                path: value.to_string(),
                identity_ignore: Vec::new(),
                refresh,
                pin: None,
            };
        }
        OverrideKey::Unknown => {}
    }
    Ok(())
}

fn apply_known_override(
    raw: &mut raw::RawBuildConfig,
    known: KnownOverrideKey,
    key: &str,
    value: &str,
) -> Result<(), ConfigError> {
    match known {
        KnownOverrideKey::BuildName => raw.build_name = value.to_string(),
        KnownOverrideKey::BuildDisplayName => raw.display_name = Some(value.to_string()),
        KnownOverrideKey::BuildVersion => raw.version = Some(value.to_string()),
        KnownOverrideKey::BuildDescription => raw.description = Some(value.to_string()),
        KnownOverrideKey::BuildBranch => raw.branch = Some(value.to_string()),
        KnownOverrideKey::BuildTarget => raw.target = Some(value.to_string()),
        KnownOverrideKey::BuildProfile => raw.profile = Some(value.to_string()),
        KnownOverrideKey::Preset => raw.preset = Some(value.to_string()),
        KnownOverrideKey::ProductFamily => raw.product.family = Some(value.to_string()),
        KnownOverrideKey::ProductName => raw.product.name = Some(value.to_string()),
        KnownOverrideKey::ProductSku => raw.product.sku = Some(value.to_string()),
        KnownOverrideKey::WorkspaceRootDir => raw.workspace.root_dir = value.to_string(),
        KnownOverrideKey::WorkspaceBuildDir => raw.workspace.build_dir = value.to_string(),
        KnownOverrideKey::WorkspaceOutDir => raw.workspace.out_dir = value.to_string(),
        KnownOverrideKey::WorkspaceExportDir => raw.workspace.export_dir = Some(value.to_string()),
        KnownOverrideKey::ImageFeedInstallEntries => {
            raw.image.feed.install_entries = split_csv(value)
        }
        KnownOverrideKey::ImageFeedStageFiles => raw.image.feed.stage_files = split_csv(value),
        KnownOverrideKey::ImageFeedStageEnvSets => raw.image.feed.stage_env_sets = split_csv(value),
        KnownOverrideKey::ImageFeedStageServices => {
            raw.image.feed.stage_services = split_csv(value)
        }
        KnownOverrideKey::ImageBuildrootDefconfig => {
            if let raw::RawImageDefinition::Buildroot { defconfig, .. } = &mut raw.image.definition
            {
                *defconfig = Some(value.to_string());
            }
        }
        KnownOverrideKey::ImageBuildrootAllowFallback => {
            if let raw::RawImageDefinition::Buildroot { allow_fallback, .. } =
                &mut raw.image.definition
            {
                *allow_fallback = parse_bool_override(key, value)?;
            }
        }
        KnownOverrideKey::ImageBuildrootExternalTree => {
            if let raw::RawImageDefinition::Buildroot { external_tree, .. } =
                &mut raw.image.definition
            {
                *external_tree = Some(value.to_string());
            }
        }
        KnownOverrideKey::ImageBuildrootSource => {
            if let raw::RawImageDefinition::Buildroot { source, .. } = &mut raw.image.definition {
                *source = Some(value.to_string());
            }
        }
        KnownOverrideKey::ImageBuildrootExternalTreeMode => {
            if let raw::RawImageDefinition::Buildroot {
                external_tree_mode, ..
            } = &mut raw.image.definition
            {
                *external_tree_mode = match value {
                    "auto" => Some(raw::RawBuildrootExternalTreeMode::Auto),
                    "required" => Some(raw::RawBuildrootExternalTreeMode::Required),
                    "disabled" => Some(raw::RawBuildrootExternalTreeMode::Disabled),
                    _ => {
                        return Err(ConfigError::invalid_override_value(
                            key,
                            value,
                            "one of auto, required, disabled",
                        ));
                    }
                };
            }
        }
        KnownOverrideKey::ImageStartingPointRootfsPath => {
            if let raw::RawImageDefinition::StartingPoint { rootfs_path, .. } =
                &mut raw.image.definition
            {
                *rootfs_path = value.to_string();
            }
        }
        KnownOverrideKey::ImageStartingPointSource => {
            if let raw::RawImageDefinition::StartingPoint { source, .. } = &mut raw.image.definition
            {
                *source = Some(value.to_string());
            }
        }
        KnownOverrideKey::ImageStartingPointSourcePath => {
            if let raw::RawImageDefinition::StartingPoint { source_path, .. } =
                &mut raw.image.definition
            {
                *source_path = Some(value.to_string());
            }
        }
        KnownOverrideKey::ImageStartingPointRootfsValidationMode => {
            if let raw::RawImageDefinition::StartingPoint {
                rootfs_validation_mode,
                ..
            } = &mut raw.image.definition
            {
                *rootfs_validation_mode = match value {
                    "require-exists" => {
                        Some(raw::RawStartingPointRootfsValidationMode::RequireExists)
                    }
                    "require-directory" => {
                        Some(raw::RawStartingPointRootfsValidationMode::RequireDirectory)
                    }
                    "require-file" => Some(raw::RawStartingPointRootfsValidationMode::RequireFile),
                    "allow-missing" => {
                        Some(raw::RawStartingPointRootfsValidationMode::AllowMissing)
                    }
                    _ => {
                        return Err(ConfigError::invalid_override_value(
                            key,
                            value,
                            "one of require-exists, require-directory, require-file, allow-missing",
                        ));
                    }
                };
            }
        }
        KnownOverrideKey::ImageStartingPointOutputMode => {
            if let raw::RawImageDefinition::StartingPoint { output_mode, .. } =
                &mut raw.image.definition
            {
                *output_mode = match value {
                    "copy-rootfs" => Some(raw::RawStartingPointOutputMode::CopyRootfs),
                    "archive-only" => Some(raw::RawStartingPointOutputMode::ArchiveOnly),
                    "copy-and-archive" => Some(raw::RawStartingPointOutputMode::CopyAndArchive),
                    _ => {
                        return Err(ConfigError::invalid_override_value(
                            key,
                            value,
                            "one of copy-rootfs, archive-only, copy-and-archive",
                        ));
                    }
                };
            }
        }
        KnownOverrideKey::ImageOutputCollectDir => {
            raw.image.output.collect_dir = Some(value.to_string())
        }
        KnownOverrideKey::ImageOutputArchiveName => {
            raw.image.output.archive_name = Some(value.to_string())
        }
        KnownOverrideKey::ImageOutputExportDir => {
            raw.image.output.export_dir = Some(value.to_string())
        }
        KnownOverrideKey::ReportingPostBuildTimeoutSeconds => {
            if let Some(post_build) = &mut raw.reporting.post_build {
                post_build.timeout_seconds = parse_u64_override(key, value)?;
            }
        }
        KnownOverrideKey::ProvenanceIdentityProject => {
            raw.provenance.identity.project = Some(value.to_string())
        }
        KnownOverrideKey::ProvenanceIdentityVendor => {
            raw.provenance.identity.vendor = Some(value.to_string())
        }
        KnownOverrideKey::ProvenanceIdentityChannel => {
            raw.provenance.identity.channel = Some(value.to_string())
        }
        KnownOverrideKey::PolicyFailureRollbackOnError => {
            raw.failure.rollback_on_error = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::ExecutionJobs => raw.execution.jobs = parse_u32_override(key, value)?,
        KnownOverrideKey::ExecutionDockerEnabled => {
            raw.execution.docker.enabled = parse_bool_override(key, value)?
        }
        KnownOverrideKey::ExecutionDockerImage => {
            raw.execution.docker.image = Some(value.to_string())
        }
        KnownOverrideKey::ExecutionOutputRetentionStdoutBytes => {
            raw.execution.output_retention.stdout_bytes = parse_usize_override(key, value)?
        }
        KnownOverrideKey::ExecutionOutputRetentionStderrBytes => {
            raw.execution.output_retention.stderr_bytes = parse_usize_override(key, value)?
        }
        KnownOverrideKey::ExecutionOutputRetentionStdoutLines => {
            raw.execution.output_retention.stdout_lines = parse_usize_override(key, value)?
        }
        KnownOverrideKey::ExecutionOutputRetentionStderrLines => {
            raw.execution.output_retention.stderr_lines = parse_usize_override(key, value)?
        }
        KnownOverrideKey::ExecutionOutputRetentionFailureTailLines => {
            raw.execution.output_retention.failure_tail_lines = parse_usize_override(key, value)?
        }
        KnownOverrideKey::PolicyFailurePreserveFailedOutputs => {
            raw.failure.preserve_failed_outputs = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyFailureRollbackDomains => {
            raw.failure.rollback_domains = Some(parse_rollback_domains_csv(key, value)?)
        }
        KnownOverrideKey::PolicyFailureKeepGoing => {
            raw.failure.keep_going = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyFailureRollbackCompleted => {
            raw.failure.rollback_completed = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyProvidersRustAllowNestedBuild => {
            raw.providers.rust.allow_nested_build = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersRustBatchBuilds => {
            raw.providers.rust.batch_builds = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyProvidersRustSharedTargetDir => {
            raw.providers.rust.shared_target_dir = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyProvidersRustRetryAttempts => {
            raw.providers.rust.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersRustTimeoutSeconds => {
            raw.providers.rust.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersGitAllowRemoteResolution => {
            raw.providers.git.allow_remote_resolution = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersGitRetryAttempts => {
            raw.providers.git.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersGitTimeoutSeconds => {
            raw.providers.git.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersArchiveRetryAttempts => {
            raw.providers.archive.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersArchiveTimeoutSeconds => {
            raw.providers.archive.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersDownloadRetryAttempts => {
            raw.providers.download.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersDownloadTimeoutSeconds => {
            raw.providers.download.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersGoRetryAttempts => {
            raw.providers.go.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersGoTimeoutSeconds => {
            raw.providers.go.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersJavaRetryAttempts => {
            raw.providers.java.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersJavaTimeoutSeconds => {
            raw.providers.java.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersJavaGradleHome => {
            if gaia_spec::GradleHomeSpec::parse(value).is_none() {
                return Err(ConfigError::invalid_override_value(
                    key,
                    value,
                    "one of: workspace, user-cache",
                ));
            }
            raw.providers.java.gradle_home = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersNodeRetryAttempts => {
            raw.providers.node.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersNodeTimeoutSeconds => {
            raw.providers.node.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersPythonRetryAttempts => {
            raw.providers.python.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersPythonTimeoutSeconds => {
            raw.providers.python.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootRetryAttempts => {
            raw.providers.buildroot.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootTimeoutSeconds => {
            raw.providers.buildroot.timeout_seconds = parse_u64_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootLocalJobs => {
            raw.providers.buildroot.local_jobs = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootDownloadDir => {
            raw.providers.buildroot.download_dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootCcacheEnabled => {
            raw.providers.buildroot.ccache.enabled = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootCcacheDir => {
            raw.providers.buildroot.ccache.dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootCcacheMaxSize => {
            raw.providers.buildroot.ccache.max_size = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootParallelPackages => {
            raw.providers.buildroot.parallel_packages = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootPackageCacheEnabled => {
            raw.providers.buildroot.package_cache.enabled = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootPackageCacheDir => {
            raw.providers.buildroot.package_cache.system_dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootPackageCacheLevel => {
            raw.providers.buildroot.package_cache.level = Some(match value {
                "system" => crate::raw::RawPackageCacheLevel::System,
                "project" => crate::raw::RawPackageCacheLevel::Project,
                _ => {
                    return Err(ConfigError::invalid_override_value(
                        key,
                        value,
                        "one of: system, project",
                    ));
                }
            })
        }
        KnownOverrideKey::PolicyProvidersBuildrootPackageCacheProjectDir => {
            raw.providers.buildroot.package_cache.project_dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootPackageCacheMaxSize => {
            raw.providers.buildroot.package_cache.max_size = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootWorkDir => {
            raw.providers.buildroot.work_dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootRamBudget => {
            value.parse::<gaia_spec::ByteSize>().map_err(|_| {
                ConfigError::invalid_override_value(key, value, "a byte size such as 60G")
            })?;
            raw.providers.buildroot.ram_budget = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootKeepRamTree => {
            raw.providers.buildroot.keep_ram_tree = Some(parse_bool_override(key, value)?)
        }
        KnownOverrideKey::PolicyProvidersBuildrootHostToolsDefault => {
            parse_host_tool_steps_override(key, value)?;
            raw.providers.buildroot.host_tools.default = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootHostToolsCcache => {
            parse_host_tool_steps_override(key, value)?;
            raw.providers
                .buildroot
                .host_tools
                .tools
                .insert("ccache".to_string(), value.to_string());
        }
        KnownOverrideKey::PolicyProvidersBuildrootHostToolsPkgconf => {
            parse_host_tool_steps_override(key, value)?;
            raw.providers
                .buildroot
                .host_tools
                .tools
                .insert("pkgconf".to_string(), value.to_string());
        }
        KnownOverrideKey::PolicyProvidersBuildrootSharedOutput => {
            raw.providers.buildroot.shared_output = parse_bool_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersBuildrootSharedOutputDir => {
            raw.providers.buildroot.shared_output_dir = Some(value.to_string())
        }
        KnownOverrideKey::PolicyProvidersBuildrootKernelModulesCheck => {
            raw.providers.buildroot.kernel_modules_check = Some(match value {
                "error" => crate::raw::RawBuildrootOverrideCheck::Error,
                "warn" => crate::raw::RawBuildrootOverrideCheck::Warn,
                "off" => crate::raw::RawBuildrootOverrideCheck::Off,
                _ => {
                    return Err(ConfigError::invalid_override_value(
                        key,
                        value,
                        "one of: error, warn, off",
                    ));
                }
            })
        }
        KnownOverrideKey::PolicyProvidersBuildrootOverrideCheck => {
            raw.providers.buildroot.override_check = Some(match value {
                "error" => crate::raw::RawBuildrootOverrideCheck::Error,
                "warn" => crate::raw::RawBuildrootOverrideCheck::Warn,
                "off" => crate::raw::RawBuildrootOverrideCheck::Off,
                _ => {
                    return Err(ConfigError::invalid_override_value(
                        key,
                        value,
                        "one of: error, warn, off",
                    ));
                }
            })
        }
        KnownOverrideKey::PolicyProvidersStartingPointRetryAttempts => {
            raw.providers.starting_point.retry_attempts = parse_u32_override(key, value)?
        }
        KnownOverrideKey::PolicyProvidersStartingPointTimeoutSeconds => {
            raw.providers.starting_point.timeout_seconds = parse_u64_override(key, value)?
        }
    }
    Ok(())
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn collect_selected_inputs(raw: &raw::RawBuildConfig) -> Vec<(String, String)> {
    let mut selected = raw
        .inputs
        .iter()
        .filter_map(|(name, input)| input.default.clone().map(|value| (name.clone(), value)))
        .collect::<Vec<_>>();
    for (name, value) in &raw.selected_inputs {
        if raw.inputs.contains_key(name) {
            upsert_pair(&mut selected, name, value);
        }
    }
    for (key, value) in &raw.explicit_overrides {
        if let OverrideKey::Input(name) = OverrideKey::parse(key)
            && raw.inputs.contains_key(name)
        {
            upsert_pair(&mut selected, name, value);
        }
    }
    selected
}

/// Checks a `host_tools` policy such as `system,build` before it is stored.
fn parse_host_tool_steps_override(key: &str, value: &str) -> Result<(), ConfigError> {
    gaia_spec::HostToolStepSpec::parse_list(value)
        .map(|_| ())
        .map_err(|_| {
            ConfigError::invalid_override_value(
                key,
                value,
                "a comma-separated list of: system, build, fail",
            )
        })
}

fn parse_bool_override(key: &str, value: &str) -> Result<bool, ConfigError> {
    match value {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::invalid_override_value(
            key,
            value,
            "a boolean: true/false, yes/no, on/off, or 1/0",
        )),
    }
}

fn parse_u32_override(key: &str, value: &str) -> Result<u32, ConfigError> {
    value
        .parse::<u32>()
        .map_err(|_| ConfigError::invalid_override_value(key, value, "an unsigned integer"))
}

fn parse_u64_override(key: &str, value: &str) -> Result<u64, ConfigError> {
    value
        .parse::<u64>()
        .map_err(|_| ConfigError::invalid_override_value(key, value, "an unsigned integer"))
}

fn parse_usize_override(key: &str, value: &str) -> Result<usize, ConfigError> {
    value
        .parse::<usize>()
        .map_err(|_| ConfigError::invalid_override_value(key, value, "an unsigned integer"))
}

fn upsert_pair(entries: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, existing)) = entries.iter_mut().find(|(entry_key, _)| entry_key == key) {
        *existing = value.to_string();
    } else {
        entries.push((key.to_string(), value.to_string()));
    }
}

fn upsert_workspace_named_path(
    entries: &mut Vec<raw::RawWorkspaceNamedPathConfig>,
    alias: &str,
    path: &str,
    kind: raw::RawWorkspacePathKind,
) {
    if let Some(existing) = entries.iter_mut().find(|entry| entry.alias == alias) {
        existing.path = path.to_string();
        existing.kind = kind;
    } else {
        entries.push(raw::RawWorkspaceNamedPathConfig {
            alias: alias.to_string(),
            path: path.to_string(),
            kind,
        });
    }
}

fn parse_rollback_domains_csv(
    key: &str,
    value: &str,
) -> Result<Vec<raw::RawRollbackDomain>, ConfigError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry {
            "sources" | "source" => Ok(raw::RawRollbackDomain::Sources),
            "artifacts" | "artifact" => Ok(raw::RawRollbackDomain::Artifacts),
            "installs" | "install" => Ok(raw::RawRollbackDomain::Installs),
            "stage" | "runtime" => Ok(raw::RawRollbackDomain::Stage),
            "images" | "image" => Ok(raw::RawRollbackDomain::Images),
            "checkpoints" | "checkpoint" => Ok(raw::RawRollbackDomain::Checkpoints),
            _ => Err(ConfigError::invalid_override_value(
                key,
                entry,
                "comma-separated rollback domains: sources, artifacts, installs, stage, images, checkpoints",
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests;
