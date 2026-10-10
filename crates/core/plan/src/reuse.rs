use crate::{
    ExecutionPlan, OperationId, OperationKind, OperationOptionality, OperationReuse,
    PlannedOperation, ReuseState,
};
use gaia_spec::{
    CheckpointAnchorRef, ImageDefinition, ResolvedBuildSpec, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

// Generous because a timeout changes the fingerprint and forces a rebuild; a
// JVM or rustup proxy on a busy machine can take several seconds to start.
pub(crate) const COMMAND_SIGNATURE_TIMEOUT_SECONDS: u64 = 10;

pub fn spec_fingerprint(spec: &ResolvedBuildSpec) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{spec:?}").hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn apply_reuse_state(
    mut plan: ExecutionPlan,
    spec: &ResolvedBuildSpec,
    reuse_state: Option<&ReuseState>,
) -> ExecutionPlan {
    let Some(reuse_state) = reuse_state else {
        return plan;
    };
    let mut decisions = HashMap::<String, bool>::new();
    // Dependency kinds, for computing input signatures while `plan` is mutated.
    let snapshot = plan.clone();

    for operation in &mut plan.operations {
        let operation_id = operation.id.as_str().to_string();
        let should_execute = match &operation.kind {
            OperationKind::ResolveBuild => true,
            OperationKind::EmitReport => true,
            _ => {
                let fingerprint_mismatch = reuse_state
                    .operation_fingerprints
                    .get(&operation_id)
                    .copied()
                    != Some(operation.fingerprint);
                let source_refresh_reason = source_refresh_rebuild_reason(spec, &operation.kind);
                let outputs_missing = !operation_outputs_present(spec, &operation.kind);
                let output_signature_mismatch = reuse_state
                    .operation_output_signatures
                    .get(&operation_id)
                    .map(String::as_str)
                    != operation_output_signature(spec, &operation.kind).as_deref();
                let dependency_rebuilding = operation.depends_on.iter().any(|dependency| {
                    if dependency.as_str() == OperationId::resolve().as_str() {
                        return false;
                    }
                    !decisions.get(dependency.as_str()).copied().unwrap_or(false)
                });
                if !reuse_state.completed_operation_ids.contains(&operation_id)
                    || source_refresh_reason.is_some()
                    || fingerprint_mismatch
                    || outputs_missing
                    || output_signature_mismatch
                    || dependency_rebuilding
                {
                    true
                } else {
                    // Dependencies are all reused, but they may have changed
                    // since this operation last consumed them (for example
                    // after a partial `--only` run).
                    reuse_state
                        .operation_input_signatures
                        .get(&operation_id)
                        .is_some_and(|recorded| {
                            *recorded != operation_input_signature(spec, &snapshot, operation)
                        })
                }
            }
        };

        if should_execute {
            if !reuse_state.completed_operation_ids.contains(&operation_id) {
                operation.reuse = OperationReuse::execute(
                    "not_in_reuse_state",
                    format!(
                        "operation '{}' is not present in the persisted reuse state",
                        operation.id.as_str()
                    ),
                );
            } else if let Some((code, message)) =
                source_refresh_rebuild_reason(spec, &operation.kind)
            {
                operation.reuse = OperationReuse::execute(code, message);
            } else if reuse_state
                .operation_fingerprints
                .get(&operation_id)
                .copied()
                != Some(operation.fingerprint)
            {
                operation.reuse = OperationReuse::execute(
                    "operation_fingerprint_mismatch",
                    format!(
                        "operation '{}' will execute because its persisted fingerprint does not match current inputs",
                        operation.id.as_str()
                    ),
                );
            } else if !operation_outputs_present(spec, &operation.kind) {
                operation.reuse = OperationReuse::execute(
                    "materialized_output_missing",
                    format!(
                        "operation '{}' will execute because its expected materialized outputs are missing",
                        operation.id.as_str()
                    ),
                );
            } else if reuse_state
                .operation_output_signatures
                .get(&operation_id)
                .map(String::as_str)
                != operation_output_signature(spec, &operation.kind).as_deref()
            {
                operation.reuse = OperationReuse::execute(
                    "operation_output_changed",
                    format!(
                        "operation '{}' will execute because its persisted materialized outputs do not match current state",
                        operation.id.as_str()
                    ),
                );
            } else if !matches!(
                &operation.kind,
                OperationKind::ResolveBuild | OperationKind::EmitReport
            ) {
                let recorded_input = reuse_state
                    .operation_input_signatures
                    .get(&operation_id)
                    .copied();
                let dependency_rebuilding = operation.depends_on.iter().any(|dependency| {
                    dependency.as_str() != OperationId::resolve().as_str()
                        && !decisions.get(dependency.as_str()).copied().unwrap_or(false)
                });
                if dependency_rebuilding {
                    operation.reuse = OperationReuse::execute(
                        "dependency_rebuilt",
                        format!(
                            "operation '{}' will execute because one or more dependencies are rebuilding",
                            operation.id.as_str()
                        ),
                    );
                    operation.cutoff_input_signature = recorded_input;
                } else {
                    operation.reuse = OperationReuse::execute(
                        "inputs_changed",
                        format!(
                            "operation '{}' will execute because its inputs changed since it last ran",
                            operation.id.as_str()
                        ),
                    );
                }
            }
            decisions.insert(operation_id, false);
        } else {
            operation.reuse = OperationReuse::Reuse {
                source: "state-file".into(),
            };
            decisions.insert(operation_id, true);
        }
    }

    plan
}

fn source_refresh_rebuild_reason(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<(&'static str, String)> {
    let OperationKind::MaterializeSource { source_id } = kind else {
        return None;
    };
    let source = spec.sources.iter().find(|source| source.id == *source_id)?;
    let (refresh_policy, pin_policy, remote_git) = match &source.definition {
        // A lockfile entry pins the commit, so the source is not floating
        // even when its configured ref is.
        SourceDefinition::Git(git) => (
            git.refresh_policy,
            git.pin_policy,
            local_repo_path(&git.repo).is_none() && git.locked_commit.is_none(),
        ),
        SourceDefinition::Path(path) => (path.refresh_policy, path.pin_policy, false),
        SourceDefinition::Archive(archive) => (archive.refresh_policy, archive.pin_policy, false),
        SourceDefinition::Download(download) => {
            (download.refresh_policy, download.pin_policy, false)
        }
    };

    if refresh_policy == SourceRefreshPolicySpec::Always {
        return Some((
            "source_refresh_always",
            format!(
                "source '{}' will materialize because its refresh policy is always",
                source.id.as_str()
            ),
        ));
    }
    if refresh_policy == SourceRefreshPolicySpec::Auto
        && remote_git
        && pin_policy == SourcePinPolicySpec::Floating
    {
        return Some((
            "remote_floating_source",
            format!(
                "source '{}' will materialize because it tracks a floating remote git ref",
                source.id.as_str()
            ),
        ));
    }
    None
}

pub(crate) fn artifact_rebuild_message(artifact: &gaia_spec::ArtifactSpec) -> String {
    if !artifact.dependencies.is_empty() {
        return format!(
            "artifact '{}' will build because dependency artifacts are part of this plan",
            artifact.id.as_str()
        );
    }
    if let Some(source) = &artifact.source {
        return format!(
            "artifact '{}' will build from source '{}'",
            artifact.id.as_str(),
            source.id.as_str()
        );
    }
    format!(
        "artifact '{}' will build because no reuse state exists yet",
        artifact.id.as_str()
    )
}

pub fn operation_fingerprint(spec: &ResolvedBuildSpec, kind: &OperationKind) -> u64 {
    let mut hasher = DefaultHasher::new();
    match kind {
        OperationKind::ResolveBuild => {
            spec.identity.build_name.hash(&mut hasher);
            spec.identity.display_name.hash(&mut hasher);
            spec.identity.version.hash(&mut hasher);
        }
        OperationKind::MaterializeSource { source_id } => {
            if let Some(source) = spec.sources.iter().find(|source| source.id == *source_id) {
                format!("{source:?}").hash(&mut hasher);
                source_backend_signature(spec, source).hash(&mut hasher);
            }
        }
        OperationKind::BuildArtifact { artifact_id } => {
            if let Some(artifact) = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id == *artifact_id)
            {
                format!("{artifact:?}").hash(&mut hasher);
                crate::reuse_toolchain::artifact_backend_signature(spec, artifact)
                    .hash(&mut hasher);
                // Only hashed when present so image-only artifacts keep
                // their existing fingerprints.
                if let Some(image) = artifact_docker_build_signature(spec, artifact) {
                    image.hash(&mut hasher);
                }
            }
        }
        OperationKind::InstallArtifact { install_id, .. } => {
            spec.install
                .entries
                .iter()
                .find(|install| install.id == *install_id)
                .map(|install| format!("{install:?}"))
                .hash(&mut hasher);
        }
        OperationKind::RenderStageFile { item_id } => {
            if let Some(item) = spec.stage.files.iter().find(|item| item.id == *item_id) {
                format!("{item:?}").hash(&mut hasher);
                path_state_signature(&resolve_workspace_path(spec, &item.src)).hash(&mut hasher);
            }
        }
        OperationKind::RenderStageEnvSet { item_id } => {
            spec.stage
                .env_sets
                .iter()
                .find(|item| item.id == *item_id)
                .map(|item| format!("{item:?}"))
                .hash(&mut hasher);
        }
        OperationKind::RenderStageService { item_id } => {
            if let Some(item) = spec.stage.services.iter().find(|item| item.id == *item_id) {
                format!("{item:?}").hash(&mut hasher);
                path_state_signature(&resolve_workspace_path(spec, &item.unit_path))
                    .hash(&mut hasher);
            }
        }
        OperationKind::PrepareImage | OperationKind::BuildImage => {
            // Disk assembly is fingerprinted by its own operation; a partition
            // layout change must not re-run the Buildroot build.
            let mut image = spec.image.clone();
            image.assembly = None;
            format!("{image:?}").hash(&mut hasher);
            image_backend_signature(spec, &spec.image).hash(&mut hasher);
            // Only hashed when enabled so existing fingerprints stay valid.
            let buildroot_policy = &spec.policy.providers.buildroot;
            if buildroot_policy.shared_output {
                (
                    "buildroot-shared-output",
                    &buildroot_policy.shared_output_dir,
                )
                    .hash(&mut hasher);
            }
        }
        OperationKind::AssembleImage => {
            format!("{:?}", spec.image.assembly).hash(&mut hasher);
            operation_output_signature(spec, &OperationKind::BuildImage).hash(&mut hasher);
            crate::reuse_assembly::assembly_input_signature(spec).hash(&mut hasher);
        }
        OperationKind::CaptureCheckpoint { checkpoint_id } => {
            spec.checkpoints
                .points
                .iter()
                .find(|checkpoint| checkpoint.id == *checkpoint_id)
                .map(|checkpoint| format!("{checkpoint:?}"))
                .hash(&mut hasher);
        }
        OperationKind::EmitReport => {
            format!("{:?}", spec.reporting).hash(&mut hasher);
        }
    }
    if let Some(imports) = crate::reuse_imports::import_source_signature(spec, kind) {
        imports.hash(&mut hasher);
    }
    hasher.finish()
}

fn source_backend_signature(spec: &ResolvedBuildSpec, source: &gaia_spec::SourceSpec) -> String {
    match &source.definition {
        SourceDefinition::Git(git) => format!(
            "{}|{}",
            command_signature("git", ["--version"]),
            git_source_state_signature(git)
        ),
        SourceDefinition::Archive(archive) => format!(
            "{}|{}",
            command_signature("tar", ["--version"]),
            path_state_signature(&resolve_workspace_path(spec, &archive.path))
        ),
        SourceDefinition::Download(_) => command_signature("curl", ["--version"]),
        SourceDefinition::Path(path) => format!(
            "path-source|{}",
            path_state_signature_with_ignores(
                &resolve_workspace_path(spec, &path.path),
                &workspace_path_ignores(spec),
            )
        ),
    }
}

/// Content hash of a Dockerfile-built execution image, so editing the
/// Dockerfile or its context rebuilds the artifact.
fn artifact_docker_build_signature(
    spec: &ResolvedBuildSpec,
    artifact: &gaia_spec::ArtifactSpec,
) -> Option<String> {
    let Some(gaia_spec::ArtifactExecutionSpec::Docker(docker)) = &artifact.execution else {
        return None;
    };
    let dockerfile = resolve_workspace_path(spec, docker.dockerfile.as_deref()?);
    let context = docker
        .context
        .as_deref()
        .map(|context| resolve_workspace_path(spec, context))
        .unwrap_or_else(|| {
            dockerfile
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        });
    Some(
        gaia_process::docker_build_context_hash(&dockerfile, &context)
            .map(|hash| format!("docker-build:{hash}"))
            .unwrap_or_else(|error| format!("docker-build-error:{error}")),
    )
}

fn image_backend_signature(spec: &ResolvedBuildSpec, image: &gaia_spec::ImageSpec) -> String {
    match &image.definition {
        ImageDefinition::Buildroot(_buildroot) => {
            let buildroot_dir = env::var("GAIA_BUILDROOT_DIR")
                .ok()
                .or_else(|| env::var("BUILDROOT_DIR").ok())
                .unwrap_or_default();
            format!(
                "{}|{}|{}|{}",
                command_signature("make", ["--version"]),
                command_signature("tar", ["--version"]),
                buildroot_dir.clone().if_empty_then("no-buildroot-dir"),
                if buildroot_dir.is_empty() {
                    "no-buildroot-state".to_string()
                } else {
                    path_state_signature(Path::new(&buildroot_dir))
                }
            )
        }
        ImageDefinition::StartingPoint(starting_point) => {
            let source_signature = if let Some(source_id) = &starting_point.source {
                let source_dir = Path::new(&spec.workspace.build_dir)
                    .join("sources")
                    .join(source_id.as_str());
                let resolved = starting_point
                    .source_path
                    .as_ref()
                    .map(|path| source_dir.join(path))
                    .unwrap_or(source_dir);
                path_state_signature(&resolved)
            } else {
                path_state_signature(Path::new(&starting_point.rootfs_path))
            };
            format!(
                "{}|{}",
                command_signature("tar", ["--version"]),
                source_signature
            )
        }
    }
}

/// Tool version signature, probed once per process: planning fingerprints
/// every artifact, and each probe would otherwise spawn the tool again.
pub(crate) fn command_signature<const N: usize>(program: &str, args: [&str; N]) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let key = std::iter::once(program)
        .chain(args)
        .collect::<Vec<_>>()
        .join("\u{1f}");
    let cache = CACHE.get_or_init(Default::default);
    if let Some(signature) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return signature;
    }
    let signature = probe_command_signature(program, args);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, signature.clone());
    }
    signature
}

fn probe_command_signature<const N: usize>(program: &str, args: [&str; N]) -> String {
    // Checked first so a missing tool does not log a process start failure.
    if !crate::reuse_toolchain::program_on_path(program) {
        return format!("{program}:unavailable");
    }
    let mut command = Command::new(program);
    command.args(args);
    let retention = gaia_process::ProcessOutputRetention {
        stdout_bytes: 4096,
        stderr_bytes: 4096,
        stdout_lines: 8,
        stderr_lines: 8,
    };
    match gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        Duration::from_secs(COMMAND_SIGNATURE_TIMEOUT_SECONDS),
        "reuse command signature",
        retention,
        None,
        None,
    ) {
        Ok(result) if result.output.status.success() => {
            let stdout = String::from_utf8_lossy(&result.output.stdout)
                .trim()
                .to_string();
            let stderr = String::from_utf8_lossy(&result.output.stderr)
                .trim()
                .to_string();
            if !stdout.is_empty() {
                format!("{program}:{stdout}")
            } else if !stderr.is_empty() {
                format!("{program}:{stderr}")
            } else {
                format!("{program}:ok")
            }
        }
        Ok(result) => format!("{program}:exit-{}", result.output.status),
        Err(error) => match error.kind {
            gaia_process::ProcessRunErrorKind::Timeout => {
                format!("{program}:timeout-{COMMAND_SIGNATURE_TIMEOUT_SECONDS}s")
            }
            gaia_process::ProcessRunErrorKind::Cancelled => format!("{program}:cancelled"),
            gaia_process::ProcessRunErrorKind::ToolStart => format!("{program}:unavailable"),
            gaia_process::ProcessRunErrorKind::RuntimeState => format!("{program}:runtime-error"),
        },
    }
}

trait EmptyFallback {
    fn if_empty_then(self, fallback: &str) -> String;
}

impl EmptyFallback for String {
    fn if_empty_then(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn resolve_workspace_path(spec: &ResolvedBuildSpec, value: &str) -> PathBuf {
    gaia_spec::resolve_workspace_path(&spec.workspace, value).unwrap_or_else(|_| {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(path)
        }
    })
}

fn git_source_state_signature(git: &gaia_spec::GitSourceSpec) -> String {
    if let Some(locked_commit) = &git.locked_commit {
        // The checkout is fully determined by the locked commit; new commits
        // in a local repository do not change it.
        return format!("locked-git:{locked_commit}");
    }
    if let Some(local_repo) = local_repo_path(&git.repo) {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&local_repo)
            .arg("rev-parse")
            .arg("HEAD");
        return match command.output() {
            Ok(output) if output.status.success() => format!(
                "local-git:{}",
                String::from_utf8_lossy(&output.stdout).trim()
            ),
            Ok(output) => format!("local-git:exit-{}", output.status),
            Err(error) => format!("local-git:unavailable:{error}"),
        };
    }
    "remote-git".into()
}

fn local_repo_path(repo: &str) -> Option<PathBuf> {
    let file_repo = repo.strip_prefix("file://").map(PathBuf::from);
    let direct = PathBuf::from(repo);
    file_repo.or_else(|| direct.exists().then_some(direct))
}

pub(crate) fn path_state_signature(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    hash_path_state(path, &mut hasher, &[]);
    format!("{:016x}", hasher.finish())
}

fn path_state_signature_with_ignores(path: &Path, ignored_names: &[String]) -> String {
    let mut hasher = DefaultHasher::new();
    hash_path_state(path, &mut hasher, ignored_names);
    format!("{:016x}", hasher.finish())
}

fn hash_path_state(path: &Path, hasher: &mut DefaultHasher, ignored_names: &[String]) {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| ignored_names.iter().any(|ignored| ignored == name))
    {
        return;
    }
    path.display().to_string().hash(hasher);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            "missing".hash(hasher);
            return;
        }
    };
    metadata.len().hash(hasher);
    metadata.is_dir().hash(hasher);
    metadata.is_file().hash(hasher);
    metadata.file_type().is_symlink().hash(hasher);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode().hash(hasher);
    }
    if let Ok(modified) = metadata.modified()
        && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
    {
        duration.as_secs().hash(hasher);
        duration.subsec_nanos().hash(hasher);
    }
    if metadata.is_dir() {
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort();
        for entry in entries {
            hash_path_state(&entry, hasher, ignored_names);
        }
    }
}

fn workspace_path_ignores(spec: &ResolvedBuildSpec) -> Vec<String> {
    let mut ignored = [
        "target",
        ".git",
        ".gaia",
        "build",
        "out",
        "node_modules",
        "__pycache__",
        ".gaia-pack",
        ".gaia-wheelhouse",
    ]
    .map(str::to_string)
    .to_vec();
    for path in [&spec.workspace.build_dir, &spec.workspace.out_dir] {
        let candidate = Path::new(path);
        if let Some(name) = candidate.file_name().and_then(|name| name.to_str())
            && !ignored.iter().any(|ignored_name| ignored_name == name)
        {
            ignored.push(name.to_string());
        }
    }
    ignored
}

fn operation_outputs_present(spec: &ResolvedBuildSpec, kind: &OperationKind) -> bool {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => true,
        OperationKind::MaterializeSource { source_id } => {
            resolve_workspace_path(spec, &spec.workspace.build_dir)
                .join("sources")
                .join(source_id.as_str())
                .join("source.txt")
                .is_file()
        }
        OperationKind::BuildArtifact { artifact_id } => spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == *artifact_id)
            .is_some_and(|artifact| artifact_output_path(spec, &artifact.output.path).exists()),
        OperationKind::InstallArtifact {
            install_id,
            artifact,
        } => {
            install_state_path(spec, install_id).is_file()
                && spec
                    .artifacts
                    .iter()
                    .find(|candidate| candidate.id == artifact.id)
                    .is_some_and(|artifact| {
                        artifact_output_path(spec, &artifact.output.path).exists()
                    })
        }
        OperationKind::RenderStageFile { item_id } => {
            stage_state_path(spec, "file", item_id).is_file()
        }
        OperationKind::RenderStageEnvSet { item_id } => {
            stage_state_path(spec, "env", item_id).is_file()
        }
        OperationKind::RenderStageService { item_id } => {
            stage_state_path(spec, "service", item_id).is_file()
        }
        OperationKind::CaptureCheckpoint { checkpoint_id } => {
            checkpoint_state_path(spec, checkpoint_id).is_file()
        }
        OperationKind::PrepareImage => buildroot_output_dir(spec).join("target").is_dir(),
        OperationKind::BuildImage => {
            let collect_exists = spec.image.output.collect_dir.as_deref().is_some_and(|dir| {
                resolve_workspace_path(spec, dir)
                    .join("image-provider.txt")
                    .is_file()
            });
            let archive_exists = match (
                spec.image.output.collect_dir.as_deref(),
                spec.image.output.archive_name.as_deref(),
            ) {
                (Some(dir), Some(name)) => resolve_workspace_path(spec, dir).join(name).is_file(),
                _ => false,
            };
            collect_exists || archive_exists
        }
        OperationKind::AssembleImage => assembly_state_path(spec).is_file(),
    }
}

/// A signature of what an operation produced, based on the provider state
/// files that record output content hashes rather than file timestamps, so
/// rebuilding identical output yields the same signature.
pub fn operation_content_signature(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<String> {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => None,
        OperationKind::BuildArtifact { artifact_id } => spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == *artifact_id)
            .map(|artifact| {
                provider_state_signature(&artifact_state_path(&artifact_output_path(
                    spec,
                    &artifact.output.path,
                )))
            }),
        OperationKind::InstallArtifact { install_id, .. } => Some(provider_state_signature(
            &install_state_path(spec, install_id),
        )),
        // Image outputs have no content-hashed state yet; use the
        // conservative output signature.
        OperationKind::PrepareImage | OperationKind::BuildImage => {
            operation_output_signature(spec, kind)
        }
        OperationKind::MaterializeSource { .. }
        | OperationKind::RenderStageFile { .. }
        | OperationKind::RenderStageEnvSet { .. }
        | OperationKind::RenderStageService { .. }
        | OperationKind::AssembleImage
        | OperationKind::CaptureCheckpoint { .. } => operation_output_signature(spec, kind),
    }
}

/// Signature of the content an operation consumes: the content signatures of
/// its direct dependencies (build resolution excluded).
pub fn operation_input_signature(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    operation: &PlannedOperation,
) -> u64 {
    let mut dependencies = operation
        .depends_on
        .iter()
        .filter(|dependency| dependency.as_str() != OperationId::resolve().as_str())
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    dependencies.dedup();
    let mut hasher = DefaultHasher::new();
    for dependency in dependencies {
        dependency.as_str().hash(&mut hasher);
        plan.operations
            .iter()
            .find(|candidate| candidate.id == *dependency)
            .and_then(|candidate| operation_content_signature(spec, &candidate.kind))
            .hash(&mut hasher);
    }
    hasher.finish()
}

pub fn operation_output_signature(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<String> {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => None,
        OperationKind::MaterializeSource { source_id } => {
            let materialized_dir = resolve_workspace_path(spec, &spec.workspace.build_dir)
                .join("sources")
                .join(source_id.as_str());
            let state = materialized_dir.join(".gaia-source-state.txt");
            Some(provider_state_signature(&state))
        }
        OperationKind::BuildArtifact { artifact_id } => spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == *artifact_id)
            .map(|artifact| {
                let output_path = artifact_output_path(spec, &artifact.output.path);
                format!(
                    "{}|{}",
                    provider_state_signature(&artifact_state_path(&output_path)),
                    path_state_signature(&output_path),
                )
            }),
        OperationKind::InstallArtifact {
            install_id,
            artifact,
        } => Some(format!(
            "{}|{}",
            provider_state_signature(&install_state_path(spec, install_id)),
            spec.artifacts
                .iter()
                .find(|candidate| candidate.id == artifact.id)
                .map(|artifact| path_state_signature(&artifact_output_path(
                    spec,
                    &artifact.output.path
                )))
                .unwrap_or_else(|| "artifact-missing".into())
        )),
        OperationKind::RenderStageFile { item_id } => Some(provider_state_signature(
            &stage_state_path(spec, "file", item_id),
        )),
        OperationKind::RenderStageEnvSet { item_id } => Some(provider_state_signature(
            &stage_state_path(spec, "env", item_id),
        )),
        OperationKind::RenderStageService { item_id } => Some(provider_state_signature(
            &stage_state_path(spec, "service", item_id),
        )),
        OperationKind::PrepareImage => {
            spec.image.output.collect_dir.as_deref().map(|collect_dir| {
                let collect_dir = resolve_workspace_path(spec, collect_dir);
                format!(
                    "{}|{}",
                    provider_state_signature(&collect_dir.join(".gaia-image-state.txt")),
                    content_state_signature(&buildroot_output_dir(spec).join(".config")),
                )
            })
        }
        OperationKind::BuildImage => {
            let mut parts = Vec::new();
            if let Some(collect_dir) = spec.image.output.collect_dir.as_deref() {
                let collect_dir = resolve_workspace_path(spec, collect_dir);
                parts.push(provider_state_signature(
                    &collect_dir.join(".gaia-image-state.txt"),
                ));
                parts.push(content_state_signature(
                    &collect_dir.join("image-provider.txt"),
                ));
            }
            if let (Some(collect_dir), Some(archive_name)) = (
                spec.image.output.collect_dir.as_deref(),
                spec.image.output.archive_name.as_deref(),
            ) {
                parts.push(content_state_signature(
                    &resolve_workspace_path(spec, collect_dir).join(archive_name),
                ));
            }
            (!parts.is_empty()).then(|| parts.join("|"))
        }
        OperationKind::AssembleImage => Some(provider_state_signature(&assembly_state_path(spec))),
        OperationKind::CaptureCheckpoint { checkpoint_id } => Some(provider_state_signature(
            &checkpoint_state_path(spec, checkpoint_id),
        )),
    }
}

fn artifact_output_path(spec: &ResolvedBuildSpec, output_path: &str) -> PathBuf {
    resolve_workspace_path(spec, output_path)
}

fn artifact_state_path(output_path: &Path) -> PathBuf {
    if output_path.is_dir() {
        output_path.join(".gaia").join("artifact.gaia-state.txt")
    } else {
        output_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".gaia")
            .join(format!(
                "{}.gaia-state.txt",
                output_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("artifact")
            ))
    }
}

fn buildroot_output_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    resolve_workspace_path(spec, &spec.workspace.build_dir).join("image/buildroot-output")
}

/// SHA-256 of a file's contents, never its mtime or absolute path.
///
/// Used for outputs that are rewritten with identical content on every run
/// (for example Buildroot's `.config` after defconfig/olddefconfig) and for
/// outputs that may be reached through a symlink into a RAM tree, where the
/// path and timestamps differ between otherwise identical builds. Only the
/// file name is recorded for missing or unreadable files.
pub(crate) fn content_state_signature(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let Ok(mut file) = fs::File::open(path) else {
        return format!("missing:{name}");
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return format!("unreadable:{name}"),
        }
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

fn provider_state_signature(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|contents| {
            format!(
                "state:{}",
                gaia_spec::KeyValueState::parse(&contents).render()
            )
        })
        .unwrap_or_else(|_| format!("state-missing:{}", path.display()))
}

fn runtime_state_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    resolve_workspace_path(spec, &spec.workspace.out_dir).join(gaia_spec::RUNTIME_STATE_DIR_NAME)
}

fn install_state_path(spec: &ResolvedBuildSpec, install_id: &gaia_spec::InstallId) -> PathBuf {
    runtime_state_dir(spec).join(format!("install-{}.state", install_id.as_str()))
}

fn stage_state_path(
    spec: &ResolvedBuildSpec,
    kind: &str,
    item_id: &gaia_spec::StageItemId,
) -> PathBuf {
    runtime_state_dir(spec).join(format!("stage-{kind}-{}.state", item_id.as_str()))
}

fn checkpoint_state_path(
    spec: &ResolvedBuildSpec,
    checkpoint_id: &gaia_spec::CheckpointId,
) -> PathBuf {
    runtime_state_dir(spec).join(format!("checkpoint-{}.state", checkpoint_id.as_str()))
}

fn assembly_state_path(spec: &ResolvedBuildSpec) -> PathBuf {
    runtime_state_dir(spec).join(gaia_spec::IMAGE_ASSEMBLY_STATE_FILE_NAME)
}

pub(crate) fn checkpoint_anchor_dependency(anchor: &CheckpointAnchorRef) -> OperationId {
    match anchor {
        CheckpointAnchorRef::Image => OperationId::image(),
        CheckpointAnchorRef::Install(id) => OperationId::install(id),
        CheckpointAnchorRef::StageFile(id) => OperationId::stage_file(id),
        CheckpointAnchorRef::StageEnvSet(id) => OperationId::stage_env_set(id),
        CheckpointAnchorRef::StageService(id) => OperationId::stage_service(id),
        CheckpointAnchorRef::Unknown(_) => OperationId::image(),
    }
}

pub(crate) fn checkpoint_optionality(
    checkpoint: &gaia_spec::CheckpointPointSpec,
) -> OperationOptionality {
    match (checkpoint.use_policy, checkpoint.upload_policy) {
        (gaia_spec::CheckpointPolicy::Always, _) | (_, gaia_spec::CheckpointPolicy::Always) => {
            OperationOptionality::Required
        }
        (gaia_spec::CheckpointPolicy::Auto, _) | (_, gaia_spec::CheckpointPolicy::Auto) => {
            OperationOptionality::Conditional
        }
        (gaia_spec::CheckpointPolicy::Off, gaia_spec::CheckpointPolicy::Off) => {
            OperationOptionality::BestEffort
        }
    }
}

#[cfg(test)]
#[path = "reuse_tests.rs"]
mod tests;
