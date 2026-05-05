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
