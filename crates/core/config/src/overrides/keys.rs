//! Typed parsing of CLI override keys.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KnownOverrideKey {
    BuildName,
    BuildDisplayName,
    BuildVersion,
    BuildDescription,
    BuildBranch,
    BuildTarget,
    BuildProfile,
    Preset,
    ProductFamily,
    ProductName,
    ProductSku,
    WorkspaceRootDir,
    WorkspaceBuildDir,
    WorkspaceOutDir,
    ImageFeedInstallEntries,
    ImageFeedStageFiles,
    ImageFeedStageEnvSets,
    ImageFeedStageServices,
    ImageBuildrootDefconfig,
    ImageBuildrootAllowFallback,
    ImageBuildrootExternalTree,
    ImageBuildrootSource,
    ImageBuildrootExternalTreeMode,
    ImageStartingPointRootfsPath,
    ImageStartingPointSource,
    ImageStartingPointSourcePath,
    ImageStartingPointRootfsValidationMode,
    ImageStartingPointOutputMode,
    ImageOutputCollectDir,
    ImageOutputArchiveName,
    ReportingPostBuildTimeoutSeconds,
    ProvenanceIdentityProject,
    ProvenanceIdentityVendor,
    ProvenanceIdentityChannel,
    PolicyFailureRollbackOnError,
    ExecutionJobs,
    ExecutionDockerEnabled,
    ExecutionDockerImage,
    ExecutionOutputRetentionStdoutBytes,
    ExecutionOutputRetentionStderrBytes,
    ExecutionOutputRetentionStdoutLines,
    ExecutionOutputRetentionStderrLines,
    ExecutionOutputRetentionFailureTailLines,
    PolicyFailurePreserveFailedOutputs,
    PolicyFailureRollbackDomains,
    PolicyFailureKeepGoing,
    PolicyFailureRollbackCompleted,
    PolicyProvidersRustAllowNestedBuild,
    PolicyProvidersRustBatchBuilds,
    PolicyProvidersRustSharedTargetDir,
    PolicyProvidersRustRetryAttempts,
    PolicyProvidersRustTimeoutSeconds,
    PolicyProvidersGitAllowRemoteResolution,
    PolicyProvidersGitRetryAttempts,
    PolicyProvidersGitTimeoutSeconds,
    PolicyProvidersArchiveRetryAttempts,
    PolicyProvidersArchiveTimeoutSeconds,
    PolicyProvidersDownloadRetryAttempts,
    PolicyProvidersDownloadTimeoutSeconds,
    PolicyProvidersGoRetryAttempts,
    PolicyProvidersGoTimeoutSeconds,
    PolicyProvidersJavaRetryAttempts,
    PolicyProvidersJavaTimeoutSeconds,
    PolicyProvidersJavaGradleHome,
    PolicyProvidersNodeRetryAttempts,
    PolicyProvidersNodeTimeoutSeconds,
    PolicyProvidersPythonRetryAttempts,
    PolicyProvidersPythonTimeoutSeconds,
    PolicyProvidersBuildrootRetryAttempts,
    PolicyProvidersBuildrootTimeoutSeconds,
    PolicyProvidersBuildrootLocalJobs,
    PolicyProvidersBuildrootDownloadDir,
    PolicyProvidersBuildrootCcacheEnabled,
    PolicyProvidersBuildrootCcacheDir,
    PolicyProvidersBuildrootCcacheMaxSize,
    PolicyProvidersBuildrootParallelPackages,
    PolicyProvidersBuildrootPackageCacheEnabled,
    PolicyProvidersBuildrootPackageCacheDir,
    PolicyProvidersBuildrootPackageCacheLevel,
    PolicyProvidersBuildrootPackageCacheProjectDir,
    PolicyProvidersBuildrootPackageCacheMaxSize,
    PolicyProvidersBuildrootWorkDir,
    PolicyProvidersBuildrootRamBudget,
    PolicyProvidersBuildrootKeepRamTree,
    PolicyProvidersBuildrootHostToolsDefault,
    PolicyProvidersBuildrootHostToolsCcache,
    PolicyProvidersBuildrootHostToolsPkgconf,
    PolicyProvidersBuildrootSharedOutput,
    PolicyProvidersBuildrootSharedOutputDir,
    PolicyProvidersBuildrootOverrideCheck,
    PolicyProvidersBuildrootKernelModulesCheck,
    PolicyProvidersStartingPointRetryAttempts,
    PolicyProvidersStartingPointTimeoutSeconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverrideKey<'a> {
    Known(KnownOverrideKey),
    Input(&'a str),
    Env(&'a str),
    InterpolationValue(&'a str),
    BuildLabel(&'a str),
    ProvenanceIdentityLabel(&'a str),
    WorkspacePath(&'a str),
    BuildrootConfigOverride(&'a str),
    /// `sources.<id>.path`: read a git source from a local directory.
    SourcePath(&'a str),
    Unknown,
}

/// Source id of a `sources.<id>.path` override key.
pub(crate) fn source_path_override_id(key: &str) -> Option<&str> {
    key.strip_prefix("sources.")?
        .strip_suffix(".path")
        .filter(|id| !id.is_empty() && !id.contains('.'))
}

impl<'a> OverrideKey<'a> {
    pub(super) fn parse(key: &'a str) -> Self {
        match key {
            "build.name" => Self::Known(KnownOverrideKey::BuildName),
            "build.display_name" => Self::Known(KnownOverrideKey::BuildDisplayName),
            "build.version" => Self::Known(KnownOverrideKey::BuildVersion),
            "build.description" => Self::Known(KnownOverrideKey::BuildDescription),
            "build.branch" => Self::Known(KnownOverrideKey::BuildBranch),
            "build.target" => Self::Known(KnownOverrideKey::BuildTarget),
            "build.profile" => Self::Known(KnownOverrideKey::BuildProfile),
            "preset" | "preset.name" => Self::Known(KnownOverrideKey::Preset),
            "product.family" => Self::Known(KnownOverrideKey::ProductFamily),
            "product.name" => Self::Known(KnownOverrideKey::ProductName),
            "product.sku" => Self::Known(KnownOverrideKey::ProductSku),
            "workspace.root_dir" => Self::Known(KnownOverrideKey::WorkspaceRootDir),
            "workspace.build_dir" => Self::Known(KnownOverrideKey::WorkspaceBuildDir),
            "workspace.out_dir" => Self::Known(KnownOverrideKey::WorkspaceOutDir),
            "image.feed.install_entries" => Self::Known(KnownOverrideKey::ImageFeedInstallEntries),
            "image.feed.stage_files" => Self::Known(KnownOverrideKey::ImageFeedStageFiles),
            "image.feed.stage_env_sets" => Self::Known(KnownOverrideKey::ImageFeedStageEnvSets),
            "image.feed.stage_services" => Self::Known(KnownOverrideKey::ImageFeedStageServices),
            "image.buildroot.defconfig" => Self::Known(KnownOverrideKey::ImageBuildrootDefconfig),
            "image.allow_fallback" | "image.buildroot.allow_fallback" => {
                Self::Known(KnownOverrideKey::ImageBuildrootAllowFallback)
            }
            "image.buildroot.external_tree" => {
                Self::Known(KnownOverrideKey::ImageBuildrootExternalTree)
            }
            "image.buildroot.source" => Self::Known(KnownOverrideKey::ImageBuildrootSource),
            "image.buildroot.external_tree_mode" => {
                Self::Known(KnownOverrideKey::ImageBuildrootExternalTreeMode)
            }
            "image.starting-point.rootfs_path" => {
                Self::Known(KnownOverrideKey::ImageStartingPointRootfsPath)
            }
            "image.starting-point.source" => {
                Self::Known(KnownOverrideKey::ImageStartingPointSource)
            }
            "image.starting-point.source_path" => {
                Self::Known(KnownOverrideKey::ImageStartingPointSourcePath)
            }
            "image.starting-point.rootfs_validation_mode" => {
                Self::Known(KnownOverrideKey::ImageStartingPointRootfsValidationMode)
            }
            "image.starting-point.output_mode" => {
                Self::Known(KnownOverrideKey::ImageStartingPointOutputMode)
            }
            "image.output.collect_dir" => Self::Known(KnownOverrideKey::ImageOutputCollectDir),
            "image.output.archive_name" => Self::Known(KnownOverrideKey::ImageOutputArchiveName),
            "reporting.post_build.timeout_seconds" => {
                Self::Known(KnownOverrideKey::ReportingPostBuildTimeoutSeconds)
            }
            "provenance.identity.project" => {
                Self::Known(KnownOverrideKey::ProvenanceIdentityProject)
            }
            "provenance.identity.vendor" => Self::Known(KnownOverrideKey::ProvenanceIdentityVendor),
            "provenance.identity.channel" => {
                Self::Known(KnownOverrideKey::ProvenanceIdentityChannel)
            }
            "policy.failure.rollback_on_error" => {
                Self::Known(KnownOverrideKey::PolicyFailureRollbackOnError)
            }
            "execution.jobs" | "policy.execution.jobs" => {
                Self::Known(KnownOverrideKey::ExecutionJobs)
            }
            "execution.docker.enabled" | "policy.execution.docker.enabled" => {
                Self::Known(KnownOverrideKey::ExecutionDockerEnabled)
            }
            "execution.docker.image" | "policy.execution.docker.image" => {
                Self::Known(KnownOverrideKey::ExecutionDockerImage)
            }
            "execution.output_retention.stdout_bytes"
            | "policy.execution.output_retention.stdout_bytes" => {
                Self::Known(KnownOverrideKey::ExecutionOutputRetentionStdoutBytes)
            }
            "execution.output_retention.stderr_bytes"
            | "policy.execution.output_retention.stderr_bytes" => {
                Self::Known(KnownOverrideKey::ExecutionOutputRetentionStderrBytes)
            }
            "execution.output_retention.stdout_lines"
            | "policy.execution.output_retention.stdout_lines" => {
                Self::Known(KnownOverrideKey::ExecutionOutputRetentionStdoutLines)
            }
            "execution.output_retention.stderr_lines"
            | "policy.execution.output_retention.stderr_lines" => {
                Self::Known(KnownOverrideKey::ExecutionOutputRetentionStderrLines)
            }
            "execution.output_retention.failure_tail_lines"
            | "policy.execution.output_retention.failure_tail_lines" => {
                Self::Known(KnownOverrideKey::ExecutionOutputRetentionFailureTailLines)
            }
            "policy.failure.preserve_failed_outputs" => {
                Self::Known(KnownOverrideKey::PolicyFailurePreserveFailedOutputs)
            }
            "policy.failure.rollback_domains" => {
                Self::Known(KnownOverrideKey::PolicyFailureRollbackDomains)
            }
            "policy.failure.keep_going" => Self::Known(KnownOverrideKey::PolicyFailureKeepGoing),
            "policy.failure.rollback_completed" => {
                Self::Known(KnownOverrideKey::PolicyFailureRollbackCompleted)
            }
            "policy.providers.rust.allow_nested_build" => {
                Self::Known(KnownOverrideKey::PolicyProvidersRustAllowNestedBuild)
            }
            "policy.providers.rust.batch_builds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersRustBatchBuilds)
            }
            "policy.providers.rust.shared_target_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersRustSharedTargetDir)
            }
            "policy.providers.rust.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersRustRetryAttempts)
            }
            "policy.providers.rust.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersRustTimeoutSeconds)
            }
            "policy.providers.git.allow_remote_resolution" => {
                Self::Known(KnownOverrideKey::PolicyProvidersGitAllowRemoteResolution)
            }
            "policy.providers.git.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersGitRetryAttempts)
            }
            "policy.providers.git.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersGitTimeoutSeconds)
            }
            "policy.providers.archive.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersArchiveRetryAttempts)
            }
            "policy.providers.archive.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersArchiveTimeoutSeconds)
            }
            "policy.providers.download.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersDownloadRetryAttempts)
            }
            "policy.providers.download.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersDownloadTimeoutSeconds)
            }
            "policy.providers.go.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersGoRetryAttempts)
            }
            "policy.providers.go.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersGoTimeoutSeconds)
            }
            "policy.providers.java.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersJavaRetryAttempts)
            }
            "policy.providers.java.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersJavaTimeoutSeconds)
            }
            "policy.providers.java.gradle_home" => {
                Self::Known(KnownOverrideKey::PolicyProvidersJavaGradleHome)
            }
            "policy.providers.node.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersNodeRetryAttempts)
            }
            "policy.providers.node.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersNodeTimeoutSeconds)
            }
            "policy.providers.python.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersPythonRetryAttempts)
            }
            "policy.providers.python.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersPythonTimeoutSeconds)
            }
            "policy.providers.buildroot.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootRetryAttempts)
            }
            "policy.providers.buildroot.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootTimeoutSeconds)
            }
            "policy.providers.buildroot.local_jobs" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootLocalJobs)
            }
            "policy.providers.buildroot.download_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootDownloadDir)
            }
            "policy.providers.buildroot.ccache.enabled" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootCcacheEnabled)
            }
            "policy.providers.buildroot.ccache.dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootCcacheDir)
            }
            "policy.providers.buildroot.ccache.max_size" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootCcacheMaxSize)
            }
            "policy.providers.buildroot.parallel_packages" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootParallelPackages)
            }
            "policy.providers.buildroot.package_cache.enabled" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootPackageCacheEnabled)
            }
            "policy.providers.buildroot.package_cache.dir"
            | "policy.providers.buildroot.package_cache.system_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootPackageCacheDir)
            }
            "policy.providers.buildroot.package_cache.level" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootPackageCacheLevel)
            }
            "policy.providers.buildroot.package_cache.project_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootPackageCacheProjectDir)
            }
            "policy.providers.buildroot.package_cache.max_size" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootPackageCacheMaxSize)
            }
            "policy.providers.buildroot.work_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootWorkDir)
            }
            "policy.providers.buildroot.ram_budget" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootRamBudget)
            }
            "policy.providers.buildroot.keep_ram_tree" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootKeepRamTree)
            }
            "policy.providers.buildroot.host_tools.default" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootHostToolsDefault)
            }
            "policy.providers.buildroot.host_tools.ccache" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootHostToolsCcache)
            }
            "policy.providers.buildroot.host_tools.pkgconf" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootHostToolsPkgconf)
            }
            "policy.providers.buildroot.shared_output" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootSharedOutput)
            }
            "policy.providers.buildroot.shared_output_dir" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootSharedOutputDir)
            }
            "policy.providers.buildroot.override_check" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootOverrideCheck)
            }
            "policy.providers.buildroot.kernel_modules_check" => {
                Self::Known(KnownOverrideKey::PolicyProvidersBuildrootKernelModulesCheck)
            }
            "policy.providers.starting_point.retry_attempts" => {
                Self::Known(KnownOverrideKey::PolicyProvidersStartingPointRetryAttempts)
            }
            "policy.providers.starting_point.timeout_seconds" => {
                Self::Known(KnownOverrideKey::PolicyProvidersStartingPointTimeoutSeconds)
            }
            _ => {
                if let Some(name) = key
                    .strip_prefix("input.")
                    .or_else(|| key.strip_prefix("inputs."))
                {
                    Self::Input(name)
                } else if let Some(name) = key.strip_prefix("env.") {
                    Self::Env(name)
                } else if let Some(name) = key.strip_prefix("interpolation.values.") {
                    Self::InterpolationValue(name)
                } else if let Some(name) = key.strip_prefix("build.labels.") {
                    Self::BuildLabel(name)
                } else if let Some(name) = key.strip_prefix("provenance.identity.labels.") {
                    Self::ProvenanceIdentityLabel(name)
                } else if let Some(name) = key.strip_prefix("workspace.paths.") {
                    Self::WorkspacePath(name)
                } else if let Some(name) = key
                    .strip_prefix("image.buildroot.config_overrides.")
                    .filter(|name| !name.trim().is_empty())
                {
                    Self::BuildrootConfigOverride(name)
                } else if let Some(id) = source_path_override_id(key) {
                    Self::SourcePath(id)
                } else {
                    Self::Unknown
                }
            }
        }
    }
}
