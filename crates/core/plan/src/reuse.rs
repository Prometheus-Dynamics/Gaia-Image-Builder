use crate::{
    ExecutionPlan, OperationId, OperationKind, OperationOptionality, OperationReuse, ReuseState,
};
use gaia_spec::{
    ArtifactDefinition, CheckpointAnchorRef, ImageDefinition, ResolvedBuildSpec, SourceDefinition,
    SourcePinPolicySpec, SourceRefreshPolicySpec,
};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

const COMMAND_SIGNATURE_TIMEOUT_SECONDS: u64 = 2;

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
                if !reuse_state.completed_operation_ids.contains(&operation_id)
                    || source_refresh_reason.is_some()
                    || fingerprint_mismatch
                    || outputs_missing
                    || output_signature_mismatch
                {
                    true
                } else {
                    operation.depends_on.iter().any(|dependency| {
                        if dependency.as_str() == OperationId::resolve().as_str() {
                            return false;
                        }
                        !decisions.get(dependency.as_str()).copied().unwrap_or(false)
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
                operation.reuse = OperationReuse::execute(
                    "dependency_rebuilt",
                    format!(
                        "operation '{}' will execute because one or more dependencies are rebuilding",
                        operation.id.as_str()
                    ),
                );
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
        SourceDefinition::Git(git) => (
            git.refresh_policy,
            git.pin_policy,
            local_repo_path(&git.repo).is_none(),
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

pub(crate) fn operation_fingerprint(spec: &ResolvedBuildSpec, kind: &OperationKind) -> u64 {
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
                artifact_backend_signature(artifact).hash(&mut hasher);
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
            format!("{:?}", spec.image).hash(&mut hasher);
            image_backend_signature(spec, &spec.image).hash(&mut hasher);
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

fn artifact_backend_signature(artifact: &gaia_spec::ArtifactSpec) -> String {
    match &artifact.definition {
        ArtifactDefinition::Rust(_) => format!(
            "{}|{}",
            command_signature("cargo", ["--version"]),
            command_signature("rustc", ["--version"])
        ),
        ArtifactDefinition::Go(_) => command_signature("go", ["version"]),
        ArtifactDefinition::Python(_) => command_signature("python3", ["--version"]),
        ArtifactDefinition::Node(_) => format!(
            "{}|{}",
            command_signature("npm", ["--version"]),
            command_signature("node", ["--version"])
        ),
        ArtifactDefinition::Java(_) => format!(
            "{}|{}",
            command_signature("mvn", ["-version"]),
            command_signature("gradle", ["--version"])
        ),
    }
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

pub(crate) fn command_signature<const N: usize>(program: &str, args: [&str; N]) -> String {
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
    let mut ignored = vec![
        "target".to_string(),
        ".git".to_string(),
        ".gaia".to_string(),
        "build".to_string(),
        "out".to_string(),
    ];
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
        OperationKind::MaterializeSource { source_id } => PathBuf::from(&spec.workspace.build_dir)
            .join("sources")
            .join(source_id.as_str())
            .join("source.txt")
            .is_file(),
        OperationKind::BuildArtifact { artifact_id } => spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == *artifact_id)
            .is_some_and(|artifact| Path::new(&artifact.output.path).exists()),
        OperationKind::InstallArtifact {
            install_id,
            artifact,
        } => {
            install_state_path(spec, install_id).is_file()
                && spec
                    .artifacts
                    .iter()
                    .find(|candidate| candidate.id == artifact.id)
                    .is_some_and(|artifact| Path::new(&artifact.output.path).exists())
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
            let collect_exists = spec
                .image
                .output
                .collect_dir
                .as_deref()
                .is_some_and(|dir| Path::new(dir).join("image-provider.txt").is_file());
            let archive_exists = match (
                spec.image.output.collect_dir.as_deref(),
                spec.image.output.archive_name.as_deref(),
            ) {
                (Some(dir), Some(name)) => Path::new(dir).join(name).is_file(),
                _ => false,
            };
            collect_exists || archive_exists
        }
        OperationKind::AssembleImage => assembly_state_path(spec).is_file(),
    }
}

pub fn operation_output_signature(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<String> {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => None,
        OperationKind::MaterializeSource { source_id } => {
            let materialized_dir = PathBuf::from(&spec.workspace.build_dir)
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
                let output_path = Path::new(&artifact.output.path);
                format!(
                    "{}|{}",
                    provider_state_signature(&artifact_state_path(output_path)),
                    path_state_signature(output_path),
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
                .map(|artifact| path_state_signature(Path::new(&artifact.output.path)))
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
                format!(
                    "{}|{}",
                    provider_state_signature(&Path::new(collect_dir).join(".gaia-image-state.txt")),
                    path_state_signature(&buildroot_output_dir(spec).join(".config")),
                )
            })
        }
        OperationKind::BuildImage => {
            let mut parts = Vec::new();
            if let Some(collect_dir) = spec.image.output.collect_dir.as_deref() {
                parts.push(provider_state_signature(
                    &Path::new(collect_dir).join(".gaia-image-state.txt"),
                ));
                parts.push(path_state_signature(
                    &Path::new(collect_dir).join("image-provider.txt"),
                ));
            }
            if let (Some(collect_dir), Some(archive_name)) = (
                spec.image.output.collect_dir.as_deref(),
                spec.image.output.archive_name.as_deref(),
            ) {
                parts.push(path_state_signature(
                    &Path::new(collect_dir).join(archive_name),
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
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let resolved_build_dir = if build_dir.is_absolute() {
        build_dir
    } else {
        PathBuf::from(&spec.workspace.root_dir).join(build_dir)
    };
    resolved_build_dir.join("image/buildroot-output")
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
    PathBuf::from(&spec.workspace.out_dir).join(gaia_spec::RUNTIME_STATE_DIR_NAME)
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
