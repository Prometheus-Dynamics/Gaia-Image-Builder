use super::*;
use gaia_spec::{DEFAULT_COMMAND_RETRY_ATTEMPTS, DEFAULT_COMMAND_RETRY_BACKOFF_MS};

pub(crate) fn compile_docker_execution(
    execution: &crate::raw::RawExecutionPolicyConfig,
) -> Option<DockerExecutionSpec> {
    execution.docker.enabled.then(|| DockerExecutionSpec {
        image: execution
            .docker
            .image
            .clone()
            .unwrap_or_else(|| "docker.io/library/debian:stable-slim".to_string()),
    })
}

pub(crate) fn compile_output_retention(
    raw: &crate::raw::RawOutputRetentionPolicyConfig,
) -> OutputRetentionPolicySpec {
    let defaults = OutputRetentionPolicySpec::default();
    OutputRetentionPolicySpec {
        stdout_bytes: nonzero_or(raw.stdout_bytes, defaults.stdout_bytes),
        stderr_bytes: nonzero_or(raw.stderr_bytes, defaults.stderr_bytes),
        stdout_lines: nonzero_or(raw.stdout_lines, defaults.stdout_lines),
        stderr_lines: nonzero_or(raw.stderr_lines, defaults.stderr_lines),
        failure_tail_lines: nonzero_or(raw.failure_tail_lines, defaults.failure_tail_lines),
    }
}

fn nonzero_or(value: usize, default: usize) -> usize {
    if value == 0 { default } else { value }
}

fn nonzero_u32_or(value: u32, default: u32) -> u32 {
    if value == 0 { default } else { value }
}

fn nonzero_u64_or(value: u64, default: u64) -> u64 {
    if value == 0 { default } else { value }
}

/// Compiles `[providers.buildroot.host_tools]`. Compilation cannot fail, so an
/// invalid policy string compiles to `fail` (the build stops and names the
/// tool) and is also recorded in `invalid`, so validation reports it before
/// anything runs; unknown tool names are ignored, like other unknown keys.
fn compile_host_tools(
    raw: &crate::raw::RawBuildrootHostToolsConfig,
) -> gaia_spec::BuildrootHostToolsPolicySpec {
    let mut invalid = Vec::new();
    let mut parse = |key: &str, text: &str| match gaia_spec::HostToolStepSpec::parse_list(text) {
        Ok(steps) => steps,
        Err(_) => {
            invalid.push((key.to_string(), text.to_string()));
            vec![gaia_spec::HostToolStepSpec::Fail]
        }
    };
    let defaults = gaia_spec::BuildrootHostToolsPolicySpec::default();
    let default = match raw.default.as_deref() {
        Some(text) => parse("default", text),
        None => defaults.default,
    };
    let tools = raw
        .tools
        .iter()
        .filter(|(tool, _)| gaia_spec::KNOWN_HOST_TOOLS.contains(&tool.as_str()))
        .map(|(tool, text)| (tool.clone(), parse(tool, text)))
        .collect();
    gaia_spec::BuildrootHostToolsPolicySpec {
        default,
        tools,
        invalid,
    }
}

/// Compiles `[providers.java] gradle_home`. An unknown value compiles to
/// `workspace` and is reported by validation (see `gradle_home_invalid`).
fn compile_gradle_home(text: Option<&str>) -> gaia_spec::GradleHomeSpec {
    text.and_then(gaia_spec::GradleHomeSpec::parse)
        .unwrap_or_default()
}

pub(crate) fn compile_command_policy(
    raw: &crate::raw::RawCommandProviderPolicyConfig,
    default_timeout_seconds: u64,
) -> CommandProviderPolicySpec {
    CommandProviderPolicySpec {
        retry_attempts: compile_provider_retry_attempts(raw.retry_attempts),
        retry_backoff_ms: compile_provider_retry_backoff_ms(raw.retry_backoff_ms),
        retry_backoff_strategy: compile_backoff_strategy(raw.retry_backoff_strategy),
        timeout_seconds: nonzero_u64_or(raw.timeout_seconds, default_timeout_seconds),
        local_jobs: raw.local_jobs,
        download_dir: raw.download_dir.clone(),
        ccache: BuildrootCcachePolicySpec {
            enabled: raw.ccache.enabled,
            dir: raw.ccache.dir.clone(),
            max_size: raw.ccache.max_size.clone(),
        },
        parallel_packages: raw.parallel_packages,
        work_dir: gaia_spec::BuildrootWorkDirPolicySpec {
            work_dir: raw.work_dir.clone().unwrap_or_else(|| "disk".to_string()),
            ram_budget: raw.ram_budget.clone(),
            keep_ram_tree: raw.keep_ram_tree.unwrap_or(true),
        },
        host_tools: compile_host_tools(&raw.host_tools),
        gradle_home: compile_gradle_home(raw.gradle_home.as_deref()),
        gradle_home_configured: raw.gradle_home.is_some(),
        gradle_home_invalid: raw
            .gradle_home
            .as_deref()
            .filter(|text| gaia_spec::GradleHomeSpec::parse(text).is_none())
            .map(str::to_string),
        package_cache: gaia_spec::BuildrootPackageCachePolicySpec {
            enabled: raw.package_cache.enabled,
            level: match raw.package_cache.level {
                Some(crate::raw::RawPackageCacheLevel::Project) => {
                    gaia_spec::PackageCacheLevelSpec::Project
                }
                _ => gaia_spec::PackageCacheLevelSpec::System,
            },
            system_dir: raw
                .package_cache
                .system_dir
                .clone()
                .or_else(|| raw.package_cache.dir.clone()),
            project_dir: raw.package_cache.project_dir.clone(),
            project_packages: raw.package_cache.project_packages.clone(),
            system_packages: raw.package_cache.system_packages.clone(),
            max_size: raw.package_cache.max_size.clone(),
        },
        shared_output: raw.shared_output,
        shared_output_dir: raw.shared_output_dir.clone(),
        kernel_modules_check: check_mode(raw.kernel_modules_check),
        override_check: match raw.override_check {
            None | Some(crate::raw::RawBuildrootOverrideCheck::Error) => {
                gaia_spec::BuildrootOverrideCheckSpec::Error
            }
            Some(crate::raw::RawBuildrootOverrideCheck::Warn) => {
                gaia_spec::BuildrootOverrideCheckSpec::Warn
            }
            Some(crate::raw::RawBuildrootOverrideCheck::Off) => {
                gaia_spec::BuildrootOverrideCheckSpec::Off
            }
        },
    }
}

pub(crate) fn compile_provider_retry_attempts(value: u32) -> u32 {
    nonzero_u32_or(value, DEFAULT_COMMAND_RETRY_ATTEMPTS)
}

pub(crate) fn compile_provider_retry_backoff_ms(value: u64) -> u64 {
    nonzero_u64_or(value, DEFAULT_COMMAND_RETRY_BACKOFF_MS)
}

pub(crate) fn compile_provider_timeout_seconds(value: u64, default: u64) -> u64 {
    nonzero_u64_or(value, default)
}

pub(crate) fn compile_backoff_strategy(
    raw: crate::raw::RawRetryBackoffStrategy,
) -> RetryBackoffStrategySpec {
    match raw {
        crate::raw::RawRetryBackoffStrategy::Fixed => RetryBackoffStrategySpec::Fixed,
        crate::raw::RawRetryBackoffStrategy::Exponential => RetryBackoffStrategySpec::Exponential,
    }
}

pub(crate) fn compile_input_kind(raw: crate::raw::RawInputKind) -> InputKindSpec {
    match raw {
        crate::raw::RawInputKind::String => InputKindSpec::String,
        crate::raw::RawInputKind::Integer => InputKindSpec::Integer,
        crate::raw::RawInputKind::Boolean => InputKindSpec::Boolean,
        crate::raw::RawInputKind::Enum => InputKindSpec::Enum,
    }
}

pub(crate) fn compile_rollback_domains(raw: Option<Vec<RawRollbackDomain>>) -> Vec<RollbackDomain> {
    let Some(raw_domains) = raw else {
        return RollbackDomain::all();
    };
    raw_domains
        .into_iter()
        .map(|domain| match domain {
            RawRollbackDomain::Sources => RollbackDomain::Sources,
            RawRollbackDomain::Artifacts => RollbackDomain::Artifacts,
            RawRollbackDomain::Installs => RollbackDomain::Installs,
            RawRollbackDomain::Stage => RollbackDomain::Stage,
            RawRollbackDomain::Images => RollbackDomain::Images,
            RawRollbackDomain::Checkpoints => RollbackDomain::Checkpoints,
        })
        .collect()
}

fn check_mode(
    raw: Option<crate::raw::RawBuildrootOverrideCheck>,
) -> gaia_spec::BuildrootOverrideCheckSpec {
    match raw {
        None | Some(crate::raw::RawBuildrootOverrideCheck::Error) => {
            gaia_spec::BuildrootOverrideCheckSpec::Error
        }
        Some(crate::raw::RawBuildrootOverrideCheck::Warn) => {
            gaia_spec::BuildrootOverrideCheckSpec::Warn
        }
        Some(crate::raw::RawBuildrootOverrideCheck::Off) => {
            gaia_spec::BuildrootOverrideCheckSpec::Off
        }
    }
}
