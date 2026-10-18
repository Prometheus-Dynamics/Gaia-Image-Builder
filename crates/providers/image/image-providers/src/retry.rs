//! The retry loop of an image provider command, shared by the Buildroot and
//! starting-point providers: attempts, backoff between them, cancellation
//! during the backoff, and the logs of each step.

use crate::{ImageProviderError, ImageProviderErrorKind, ProcessCancelCheck};
use gaia_process::{
    ProcessRetryBackoffStrategy, retry_backoff_duration as process_retry_backoff_duration,
    sleep_with_cancel,
};
use gaia_spec::RetryBackoffStrategySpec;
use std::time::Duration;

/// What one command's retries are, for its logs and errors.
pub struct RetryRun<'a> {
    /// The `provider_domain` of the logs, such as `image.buildroot`.
    pub domain: &'a str,
    /// The command's name in logs and errors.
    pub label: &'a str,
    /// `host` or `docker`.
    pub backend: &'a str,
    pub docker_image: Option<&'a str>,
    pub timeout: Duration,
    pub attempts: u32,
    pub backoff_strategy: RetryBackoffStrategySpec,
    pub backoff_ms: u64,
    pub cancel_check: Option<&'a ProcessCancelCheck>,
}

/// A failed attempt: the message it leaves as the last error, and whether
/// another attempt may help.
pub struct AttemptFailure {
    pub message: String,
    pub retryable: bool,
}

/// Runs an attempt (given its 1-based number) until one succeeds, one fails
/// non-retryably, or the attempts run out. An error the attempt returns stops
/// the loop at once.
pub fn run_with_retries(
    run: &RetryRun<'_>,
    mut attempt: impl FnMut(u32) -> Result<Option<AttemptFailure>, ImageProviderError>,
) -> Result<(), ImageProviderError> {
    let attempts = run.attempts.max(1);
    let mut last_error = String::new();
    for number in 1..=attempts {
        tracing::debug!(
            command_label = run.label,
            provider_domain = run.domain,
            backend = run.backend,
            docker_image = run.docker_image,
            attempt = number,
            attempts,
            timeout_seconds = run.timeout.as_secs(),
            "running image provider command"
        );
        let Some(failure) = attempt(number)? else {
            tracing::debug!(
                command_label = run.label,
                provider_domain = run.domain,
                backend = run.backend,
                attempt = number,
                attempts,
                "image provider command succeeded"
            );
            return Ok(());
        };
        last_error = failure.message;
        if !failure.retryable {
            break;
        }
        if number < attempts {
            let backoff = retry_backoff_duration(run.backoff_strategy, run.backoff_ms, number);
            tracing::warn!(
                command_label = run.label,
                provider_domain = run.domain,
                backend = run.backend,
                attempt = number,
                attempts,
                backoff_ms = backoff.as_millis(),
                "image provider command failed; retrying"
            );
            if !sleep_with_cancel(backoff, run.cancel_check) {
                tracing::warn!(
                    command_label = run.label,
                    provider_domain = run.domain,
                    backend = run.backend,
                    attempt = number,
                    attempts,
                    "image provider retry backoff cancelled"
                );
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::Cancelled,
                    format!("{} cancelled during retry backoff", run.label),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = run.label,
        provider_domain = run.domain,
        backend = run.backend,
        attempts,
        "image provider command exhausted retries"
    );
    Err(ImageProviderError::backend_command(last_error))
}

/// The delay before retry number `attempt` (the first retry is attempt 1).
pub fn retry_backoff_duration(
    strategy: RetryBackoffStrategySpec,
    base_backoff_ms: u64,
    attempt: u32,
) -> Duration {
    process_retry_backoff_duration(
        match strategy {
            RetryBackoffStrategySpec::Fixed => ProcessRetryBackoffStrategy::Fixed,
            RetryBackoffStrategySpec::Exponential => ProcessRetryBackoffStrategy::Exponential,
        },
        base_backoff_ms,
        attempt,
    )
}
