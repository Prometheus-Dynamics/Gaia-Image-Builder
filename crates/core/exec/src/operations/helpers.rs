use super::*;
use gaia_image_providers::ImageExecutionPolicy;
use gaia_spec::{KeyValueState, StageItemId};
use std::collections::VecDeque;
use std::fs as std_fs;
use std::sync::{Arc, Mutex};

/// Bounded tail of an operation's streamed output.
///
/// Every streamed line goes to the live event sink as it arrives; only the
/// newest `failure_tail_lines` are kept here, for the failure output tail. A
/// long build (Buildroot emits hundreds of thousands of lines) therefore
/// costs constant memory and is never re-emitted after it finishes.
#[derive(Clone)]
pub(crate) struct LogTail {
    inner: Arc<Mutex<VecDeque<String>>>,
    capacity: usize,
}

impl LogTail {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity.min(1024)))),
            capacity,
        }
    }

    pub(crate) fn for_spec(spec: &ResolvedBuildSpec) -> Self {
        Self::new(spec.policy.execution.output_retention.failure_tail_lines)
    }

    pub(crate) fn push(&self, line: String) {
        if self.capacity == 0 {
            return;
        }
        let mut lines = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// The retained streamed tail followed by the failure message, bounded
    /// to the configured tail length.
    pub(crate) fn failure_tail(&self, message: &str, spec: &ResolvedBuildSpec) -> Vec<String> {
        let mut lines = self.lines();
        lines.extend(message.lines().map(str::to_string));
        output_tail(&lines, spec)
    }

    /// A sink that streams each line live to the event sender and keeps the
    /// bounded tail. `None` when nobody is observing the run.
    pub(crate) fn sink(
        &self,
        operation_id: OperationId,
        event_sender: Option<mpsc::Sender<ExecutionEvent>>,
    ) -> Option<gaia_process::ProcessLogSink> {
        let direct_sink = process_log_sink(operation_id, event_sender)?;
        let tail = self.clone();
        Some(Arc::new(move |line: gaia_process::ProcessLogLine| {
            tail.push(line.line.clone());
            direct_sink(line);
        }) as gaia_process::ProcessLogSink)
    }
}

pub(crate) fn output_tail(lines: &[String], spec: &ResolvedBuildSpec) -> Vec<String> {
    let tail_lines = spec.policy.execution.output_retention.failure_tail_lines;
    let start = lines.len().saturating_sub(tail_lines);
    lines[start..].to_vec()
}

/// The policy an image operation of `spec` runs with (also used by previews).
pub fn image_execution_policy(spec: &ResolvedBuildSpec) -> ImageExecutionPolicy {
    let policy = spec
        .policy
        .providers
        .image_command_policy(spec.image.provider_kind());
    ImageExecutionPolicy {
        retry_attempts: policy.retry_attempts,
        retry_backoff_ms: policy.retry_backoff_ms,
        retry_backoff_strategy: policy.retry_backoff_strategy,
        timeout_seconds: policy.timeout_seconds,
        jobs: spec.policy.execution.jobs,
        local_jobs: policy.local_jobs,
        download_dir: policy.download_dir,
        ccache_enabled: policy.ccache.enabled,
        ccache_dir: policy.ccache.dir,
        ccache_max_size: policy.ccache.max_size,
        parallel_packages: spec.image.provider_kind() == gaia_spec::ImageProviderKind::Buildroot
            && spec.policy.providers.buildroot.parallel_packages,
        package_cache: spec.policy.providers.buildroot.package_cache.clone(),
        work_dir: spec.policy.providers.buildroot.work_dir.clone(),
        host_tools: spec.policy.providers.buildroot.host_tools.clone(),
        shared_output: spec.image.provider_kind() == gaia_spec::ImageProviderKind::Buildroot
            && spec.policy.providers.buildroot.shared_output,
        shared_output_dir: spec.policy.providers.buildroot.shared_output_dir.clone(),
        override_check: spec.policy.providers.buildroot.override_check,
        kernel_modules_check: spec.policy.providers.buildroot.kernel_modules_check,
        output_retention: process_output_retention(spec),
        rebuild_packages: Vec::new(),
    }
}

pub(crate) fn process_output_retention(
    spec: &ResolvedBuildSpec,
) -> gaia_process::ProcessOutputRetention {
    let retention = spec.policy.execution.output_retention;
    gaia_process::ProcessOutputRetention {
        stdout_bytes: retention.stdout_bytes,
        stderr_bytes: retention.stderr_bytes,
        stdout_lines: retention.stdout_lines,
        stderr_lines: retention.stderr_lines,
    }
}

pub(crate) fn resolve_artifact_source_dir(
    spec: &ResolvedBuildSpec,
    artifact: &gaia_spec::ArtifactSpec,
) -> Option<String> {
    let source_ref = artifact.source.as_ref()?;
    let source = spec
        .sources
        .iter()
        .find(|source| source.id == source_ref.id)?;
    let resolved = gaia_spec::source_materialized_dir(&spec.workspace, source);
    Some(
        std_fs::canonicalize(&resolved)
            .unwrap_or(resolved)
            .display()
            .to_string(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StageRuntimeKind {
    File,
    Env,
    Service,
}

impl StageRuntimeKind {
    fn as_state_kind(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Env => "env",
            Self::Service => "service",
        }
    }
}

pub(crate) fn stage_result(
    spec: &ResolvedBuildSpec,
    operation_id: OperationId,
    runtime_kind: StageRuntimeKind,
    prefix: &str,
    item_id: &StageItemId,
) -> OperationExecutionResult {
    let _ = FsMutation::stage(item_id.as_str().to_string());
    let kind = runtime_kind.as_state_kind();
    let mut state = KeyValueState::new()
        .with("kind", format!("stage-{kind}"))
        .with("item_id", item_id.as_str());
    match runtime_kind {
        StageRuntimeKind::File => {
            let file = spec.stage.files.iter().find(|file| file.id == *item_id);
            state = state
                .with(
                    "src",
                    file.map(|file| file.src.as_str()).unwrap_or_default(),
                )
                .with(
                    "dest",
                    file.map(|file| file.dest.as_str()).unwrap_or_default(),
                )
                .with(
                    "origin",
                    file.map(|file| file.origin.as_str())
                        .unwrap_or("static-asset"),
                );
        }
        StageRuntimeKind::Env => {
            let env_set = spec
                .stage
                .env_sets
                .iter()
                .find(|env_set| env_set.id == *item_id);
            state = state
                .with(
                    "name",
                    env_set
                        .map(|env_set| env_set.name.as_str())
                        .unwrap_or_default(),
                )
                .with(
                    "entry_count",
                    env_set
                        .map(|env_set| env_set.entries.len())
                        .unwrap_or_default(),
                );
        }
        StageRuntimeKind::Service => {
            let service = spec
                .stage
                .services
                .iter()
                .find(|service| service.id == *item_id);
            state = state
                .with(
                    "name",
                    service
                        .map(|service| service.name.as_str())
                        .unwrap_or_default(),
                )
                .with(
                    "unit_path",
                    service
                        .map(|service| service.unit_path.as_str())
                        .unwrap_or_default(),
                );
        }
    };
    let state_path = stage_state_path(spec, kind, item_id);
    if let Err(message) = write_runtime_state(state_path.clone(), &state) {
        return failure_with_cleanup(
            operation_id,
            "stage_runtime_state_failed",
            ExecutionErrorKind::RuntimeState,
            message,
            RollbackDomain::Stage,
            vec![state_path],
        );
    }
    OperationExecutionResult::success(operation_id, format!("{prefix} '{}'", item_id.as_str()))
        .with_cleanup_domain(RollbackDomain::Stage)
        .with_cleanup_paths(vec![state_path])
}

pub(crate) fn runtime_state_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    PathBuf::from(&spec.workspace.out_dir).join(gaia_spec::RUNTIME_STATE_DIR_NAME)
}

pub(crate) fn install_state_path(
    spec: &ResolvedBuildSpec,
    install_id: &gaia_spec::InstallId,
) -> PathBuf {
    runtime_state_dir(spec).join(format!("install-{}.state", install_id.as_str()))
}

fn stage_state_path(spec: &ResolvedBuildSpec, kind: &str, item_id: &StageItemId) -> PathBuf {
    runtime_state_dir(spec).join(format!("stage-{kind}-{}.state", item_id.as_str()))
}

pub(crate) fn checkpoint_state_path(
    spec: &ResolvedBuildSpec,
    checkpoint_id: &gaia_spec::CheckpointId,
) -> PathBuf {
    runtime_state_dir(spec).join(format!("checkpoint-{}.state", checkpoint_id.as_str()))
}

pub(crate) fn write_runtime_state(path: PathBuf, state: &KeyValueState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create runtime state dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    std_fs::write(&path, state.render()).map_err(|error| {
        format!(
            "failed to write runtime state '{}': {error}",
            path.display()
        )
    })
}

pub(crate) fn success_from_messages(
    operation_id: OperationId,
    mut messages: Vec<String>,
    fallback: String,
    cleanup_domain: RollbackDomain,
    cleanup_paths: Vec<PathBuf>,
) -> OperationExecutionResult {
    if messages.is_empty() {
        return OperationExecutionResult::success(operation_id, fallback)
            .with_cleanup_domain(cleanup_domain)
            .with_cleanup_paths(cleanup_paths);
    }

    let mut events = Vec::new();
    for message in messages.drain(..) {
        events.push(ExecutionEvent::Log {
            operation_id: operation_id.clone(),
            message,
        });
    }
    events.push(ExecutionEvent::Succeeded {
        operation_id: operation_id.clone(),
    });

    OperationExecutionResult {
        operation_id,
        events,
        error: None,
        cancelled: false,
        reused_source: None,
        image_results: Vec::new(),
        cleanup_domain: Some(cleanup_domain),
        cleanup_paths,
    }
}

pub(crate) fn cancelled_with_cleanup(
    operation_id: OperationId,
    message: String,
    cleanup_domain: RollbackDomain,
    cleanup_paths: Vec<PathBuf>,
) -> OperationExecutionResult {
    OperationExecutionResult {
        events: vec![ExecutionEvent::Log {
            operation_id: operation_id.clone(),
            message,
        }],
        operation_id,
        error: None,
        cancelled: true,
        reused_source: None,
        image_results: Vec::new(),
        cleanup_domain: Some(cleanup_domain),
        cleanup_paths,
    }
}

pub(crate) fn failure_with_kind(
    operation_id: OperationId,
    code: &'static str,
    kind: ExecutionErrorKind,
    message: String,
) -> OperationExecutionResult {
    failure_with_kind_and_tail(operation_id, code, kind, message, Vec::new())
}

pub(crate) fn failure_with_kind_and_tail(
    operation_id: OperationId,
    code: &'static str,
    kind: ExecutionErrorKind,
    message: String,
    output_tail: Vec<String>,
) -> OperationExecutionResult {
    OperationExecutionResult {
        events: vec![ExecutionEvent::Failed {
            operation_id: operation_id.clone(),
            message: message.clone(),
        }],
        operation_id: operation_id.clone(),
        error: Some(ExecutionError {
            code,
            kind,
            operation_id,
            message,
            output_tail,
            cleanup_domain: None,
            cleanup_paths: Vec::new(),
            cleanup_status: crate::ExecutionCleanupStatus::NotRequired,
            cleanup_failures: Vec::new(),
        }),
        cancelled: false,
        reused_source: None,
        image_results: Vec::new(),
        cleanup_domain: None,
        cleanup_paths: Vec::new(),
    }
}

pub(crate) fn failure_with_cleanup(
    operation_id: OperationId,
    code: &'static str,
    kind: ExecutionErrorKind,
    message: String,
    cleanup_domain: RollbackDomain,
    cleanup_paths: Vec<PathBuf>,
) -> OperationExecutionResult {
    failure_with_cleanup_and_tail(
        operation_id,
        code,
        kind,
        message,
        Vec::new(),
        cleanup_domain,
        cleanup_paths,
    )
}

pub(crate) fn failure_with_cleanup_and_tail(
    operation_id: OperationId,
    code: &'static str,
    kind: ExecutionErrorKind,
    message: String,
    output_tail: Vec<String>,
    cleanup_domain: RollbackDomain,
    cleanup_paths: Vec<PathBuf>,
) -> OperationExecutionResult {
    let mut result = failure_with_kind_and_tail(operation_id, code, kind, message, output_tail);
    result.cleanup_domain = Some(cleanup_domain);
    result.cleanup_paths = cleanup_paths;
    if let Some(error) = &mut result.error {
        error.cleanup_domain = Some(cleanup_domain);
        error.cleanup_paths = result.cleanup_paths.clone();
    }
    result
}

pub(crate) fn execution_error_kind_from_source(
    kind: &gaia_source_providers::SourceProviderErrorKind,
) -> ExecutionErrorKind {
    match kind {
        gaia_source_providers::SourceProviderErrorKind::ToolStart => ExecutionErrorKind::ToolStart,
        gaia_source_providers::SourceProviderErrorKind::Timeout => ExecutionErrorKind::Timeout,
        gaia_source_providers::SourceProviderErrorKind::Cancelled => ExecutionErrorKind::Cancelled,
        gaia_source_providers::SourceProviderErrorKind::OutputMissing => {
            ExecutionErrorKind::OutputMissing
        }
        gaia_source_providers::SourceProviderErrorKind::BackendCommand => {
            ExecutionErrorKind::BackendCommand
        }
        gaia_source_providers::SourceProviderErrorKind::PolicyBlocked => {
            ExecutionErrorKind::PolicyBlocked
        }
        gaia_source_providers::SourceProviderErrorKind::RuntimeState => {
            ExecutionErrorKind::RuntimeState
        }
        gaia_source_providers::SourceProviderErrorKind::Unknown => ExecutionErrorKind::Unknown,
    }
}

pub(crate) fn execution_error_kind_from_artifact(
    kind: &gaia_artifact_providers::ArtifactProviderErrorKind,
) -> ExecutionErrorKind {
    match kind {
        gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart => {
            ExecutionErrorKind::ToolStart
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::Timeout => ExecutionErrorKind::Timeout,
        gaia_artifact_providers::ArtifactProviderErrorKind::Cancelled => {
            ExecutionErrorKind::Cancelled
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::OutputMissing => {
            ExecutionErrorKind::OutputMissing
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::BackendCommand => {
            ExecutionErrorKind::BackendCommand
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::PolicyBlocked => {
            ExecutionErrorKind::PolicyBlocked
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::RuntimeState => {
            ExecutionErrorKind::RuntimeState
        }
        gaia_artifact_providers::ArtifactProviderErrorKind::Unknown => ExecutionErrorKind::Unknown,
    }
}

pub(crate) fn execution_error_kind_from_image(
    kind: &gaia_image_providers::ImageProviderErrorKind,
) -> ExecutionErrorKind {
    match kind {
        gaia_image_providers::ImageProviderErrorKind::ToolStart => ExecutionErrorKind::ToolStart,
        gaia_image_providers::ImageProviderErrorKind::Timeout => ExecutionErrorKind::Timeout,
        gaia_image_providers::ImageProviderErrorKind::Cancelled => ExecutionErrorKind::Cancelled,
        gaia_image_providers::ImageProviderErrorKind::OutputMissing => {
            ExecutionErrorKind::OutputMissing
        }
        gaia_image_providers::ImageProviderErrorKind::BackendCommand => {
            ExecutionErrorKind::BackendCommand
        }
        gaia_image_providers::ImageProviderErrorKind::PolicyBlocked => {
            ExecutionErrorKind::PolicyBlocked
        }
        gaia_image_providers::ImageProviderErrorKind::RuntimeState => {
            ExecutionErrorKind::RuntimeState
        }
        gaia_image_providers::ImageProviderErrorKind::Unknown => ExecutionErrorKind::Unknown,
    }
}

impl OperationExecutionResult {
    pub(crate) fn with_cleanup_paths(mut self, cleanup_paths: Vec<PathBuf>) -> Self {
        self.cleanup_paths = cleanup_paths;
        self
    }

    pub(crate) fn with_image_result(
        mut self,
        image_result: gaia_image_providers::ImageExecutionResult,
    ) -> Self {
        self.image_results.push(image_result);
        self
    }
}

pub(crate) fn source_cleanup_paths(
    spec: &ResolvedBuildSpec,
    source: &gaia_spec::SourceSpec,
) -> Vec<PathBuf> {
    vec![
        PathBuf::from(&spec.workspace.build_dir)
            .join("sources")
            .join(source.id.as_str()),
    ]
}

pub(crate) fn artifact_cleanup_paths(contract: &ArtifactExecutionContract) -> Vec<PathBuf> {
    let output_path = PathBuf::from(&contract.output.path);
    vec![
        output_path,
        gaia_artifact_providers::artifact_sidecar_path(contract, "gaia-build.txt"),
        gaia_artifact_providers::artifact_sidecar_path(contract, "gaia-state.txt"),
    ]
}

pub(crate) fn image_cleanup_paths(
    result: &gaia_image_providers::ImageExecutionResult,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(collect_dir) = &result.collect_dir {
        paths.push(collect_dir.clone());
    }
    if let Some(archive_path) = &result.archive_path
        && !paths.iter().any(|existing| existing == archive_path)
    {
        paths.push(archive_path.clone());
    }
    paths
}

pub(crate) fn image_definition_cleanup_paths(spec: &ResolvedBuildSpec) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(collect_dir) = &spec.image.output.collect_dir {
        paths.push(PathBuf::from(collect_dir));
    }
    if let (Some(collect_dir), Some(archive_name)) = (
        &spec.image.output.collect_dir,
        &spec.image.output.archive_name,
    ) {
        let archive_path = PathBuf::from(collect_dir).join(archive_name);
        if !paths.iter().any(|existing| existing == &archive_path) {
            paths.push(archive_path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_tail_uses_execution_retention_policy() {
        let mut spec = ResolvedBuildSpec::new("tail-policy");
        spec.policy.execution.output_retention.failure_tail_lines = 2;
        let lines = vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ];

        assert_eq!(
            output_tail(&lines, &spec),
            vec!["second".to_string(), "third".to_string()]
        );
    }
}
