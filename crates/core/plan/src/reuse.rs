use crate::reuse::content::{
    config_path_fingerprint, content_path_signature, rendered_stage_content,
};
use crate::reuse_explain::{
    describe_rebuilding, fingerprint_change_message, rebuilding_dependencies,
    source_tree_content_signature,
};
use crate::reuse_floating::FloatingCheck;
use crate::{
    ExecutionPlan, OperationId, OperationKind, OperationOptionality, OperationReuse,
    PlannedOperation, REBUILD_REQUESTED, RebuildRequest, ReuseState,
};
use gaia_spec::{
    CheckpointAnchorRef, ImageDefinition, ResolvedBuildSpec, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec,
};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

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
    rebuild: &RebuildRequest,
) -> ExecutionPlan {
    let Some(reuse_state) = reuse_state else {
        rebuild.mark(&mut plan);
        return plan;
    };
    let mut decisions = HashMap::<String, bool>::new();
    // Reason code of each operation that executes, for naming a cascade's cause.
    let mut executing_codes = HashMap::<String, &'static str>::new();
    // Dependency kinds, for computing input signatures while `plan` is mutated.
    let snapshot = plan.clone();
    // Output and input signatures of this plan's operations, each computed once.
    let signatures = PlanSignatures::new(spec, &snapshot);
    // Decisions are read per dependency, so every dependency must be decided
    // before its dependents. Operations are pushed in planning order, which
    // does not always put a dependency first (an artifact after an image
    // prepare step is pushed before it).
    for index in evaluation_order(&plan.operations) {
        let operation = &mut plan.operations[index];
        let operation_id = operation.id.as_str().to_string();
        let requested = rebuild.reason(&operation_id, &operation.kind);
        let should_execute = requested.is_some()
            || match &operation.kind {
                OperationKind::ResolveBuild => true,
                OperationKind::EmitReport => true,
                _ => {
                    let fingerprint_mismatch = reuse_state
                        .operation_fingerprints
                        .get(&operation_id)
                        .copied()
                        != Some(operation.fingerprint);
                    let source_refresh_reason =
                        source_refresh_rebuild_reason(spec, &operation.kind);
                    let outputs_missing = !operation_outputs_present(spec, &operation.kind);
                    let output_signature_mismatch = reuse_state
                        .operation_output_signatures
                        .get(&operation_id)
                        .map(String::as_str)
                        != signatures.output_signature(index).as_deref();
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
                            .is_some_and(|recorded| *recorded != signatures.input_signature(index))
                    }
                }
            };

        if should_execute {
            if let Some(message) = requested {
                operation.reuse = OperationReuse::execute(REBUILD_REQUESTED, message);
            } else if !reuse_state.completed_operation_ids.contains(&operation_id) {
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
                    fingerprint_change_message(spec, &operation.id, &operation.kind),
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
                != signatures.output_signature(index).as_deref()
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
                    let rebuilding = describe_rebuilding(
                        &rebuilding_dependencies(operation, &decisions),
                        &executing_codes,
                    );
                    let cutoff_note = if recorded_input.is_some() {
                        "; reused instead if the rebuilt inputs turn out unchanged"
                    } else {
                        ""
                    };
                    operation.reuse = OperationReuse::execute(
                        "dependency_rebuilt",
                        format!(
                            "operation '{}' will execute because {rebuilding}{cutoff_note}",
                            operation.id.as_str(),
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
            if let OperationReuse::Execute(reason) = &operation.reuse {
                executing_codes.insert(operation_id.clone(), reason.code);
            }
            decisions.insert(operation_id, false);
        } else {
            let source = match &operation.kind {
                OperationKind::MaterializeSource { source_id } => {
                    floating_reuse_note(spec, source_id.as_str())
                }
                _ => None,
            };
            operation.reuse = OperationReuse::Reuse {
                source: source.unwrap_or_else(|| "state-file".into()),
            };
            decisions.insert(operation_id, true);
        }
    }

    plan
}

/// Indices of `operations` with every dependency before its dependents. Ties
/// keep planning order, so an already ordered plan is evaluated as pushed. A
/// dependency missing from the plan is ignored here (`validate` reports it),
/// and operations on a cycle (also reported by `validate`) keep planning order
/// at the end.
fn evaluation_order(operations: &[PlannedOperation]) -> Vec<usize> {
    let index_of: HashMap<&str, usize> = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation.id.as_str(), index))
        .collect();
    let mut pending = vec![0usize; operations.len()];
    let mut dependents = vec![Vec::new(); operations.len()];
    for (index, operation) in operations.iter().enumerate() {
        for dependency in &operation.depends_on {
            if let Some(&dependency_index) = index_of.get(dependency.as_str()) {
                pending[index] += 1;
                dependents[dependency_index].push(index);
            }
        }
    }
    let mut ready = std::collections::BinaryHeap::new();
    for (index, count) in pending.iter().enumerate() {
        if *count == 0 {
            ready.push(std::cmp::Reverse(index));
        }
    }
    let mut order = Vec::with_capacity(operations.len());
    let mut placed = vec![false; operations.len()];
    while let Some(std::cmp::Reverse(index)) = ready.pop() {
        placed[index] = true;
        order.push(index);
        for &dependent in &dependents[index] {
            pending[dependent] -= 1;
            if pending[dependent] == 0 {
                ready.push(std::cmp::Reverse(dependent));
            }
        }
    }
    order.extend((0..operations.len()).filter(|index| !placed[*index]));
    order
}

/// Why a git source reused by a floating ref that did not move is reused.
fn floating_reuse_note(spec: &ResolvedBuildSpec, source_id: &str) -> Option<String> {
    let source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == source_id)?;
    let SourceDefinition::Git(git) = &source.definition else {
        return None;
    };
    crate::reuse_floating::reuse_note(spec, source_id, git)
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
    // A floating git ref is compared with the commit last materialized. An
    // unchanged ref keeps the checkout; a moved or unresolvable one
    // materializes, and an unresolvable one says why.
    if let SourceDefinition::Git(git) = &source.definition
        && refresh_policy == SourceRefreshPolicySpec::Auto
        && pin_policy == SourcePinPolicySpec::Floating
    {
        match crate::reuse_floating::check(spec, source.id.as_str(), git) {
            FloatingCheck::NotApplicable => {}
            FloatingCheck::Unchanged { .. } => return None,
            FloatingCheck::Moved { label, from, to } => {
                let message = match from {
                    Some(from) => format!(
                        "source '{}' will materialize because {label} moved from {} to {}",
                        source.id.as_str(),
                        crate::reuse_floating::short(&from),
                        crate::reuse_floating::short(&to),
                    ),
                    None => format!(
                        "source '{}' will materialize because {label} is at {} and no commit is recorded for it",
                        source.id.as_str(),
                        crate::reuse_floating::short(&to),
                    ),
                };
                return Some(("remote_floating_source", message));
            }
            FloatingCheck::Unresolved { label, error } => {
                return Some((
                    "remote_floating_source",
                    format!(
                        "source '{}' will materialize because {label} could not be resolved ({error})",
                        source.id.as_str(),
                    ),
                ));
            }
        }
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
                // The contents of build command scripts (see reuse_build_inputs).
                let scripts = crate::reuse_build_inputs::build_command_components(spec, artifact);
                if !scripts.is_empty() {
                    format!("{scripts:?}").hash(&mut hasher);
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
                config_path_fingerprint(spec, &item.src).hash(&mut hasher);
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
                config_path_fingerprint(spec, &item.unit_path).hash(&mut hasher);
            }
        }
        OperationKind::PrepareImage | OperationKind::BuildImage => {
            // Disk assembly is fingerprinted by its own operation; a partition
            // layout change must not re-run the Buildroot build.
            let mut image = spec.image.clone();
            image.assembly = None;
            // Where and under what name images are collected and archived is
            // image:build's business: the prepare step neither collects nor
            // archives, so a renamed image must not re-run it.
            if matches!(kind, OperationKind::PrepareImage) {
                image.output = Default::default();
                // The image feed (installs, stage files, services) is applied
                // by image:build's post-build step; prepare runs no feed.
                image.feed = Default::default();
                // Leaving packages out of the rootfs is applied to the image's
                // target by image:build only; prepare finalizes without it.
                if let ImageDefinition::Buildroot(buildroot) = &mut image.definition {
                    buildroot.rootfs_exclude_packages.clear();
                }
            }
            format!("{image:?}").hash(&mut hasher);
            image_backend_signature(spec, &spec.image).hash(&mut hasher);
            // Post scripts and BR2_EXTERNAL files by content (see
            // reuse_build_inputs); only hashed when there are some.
            let content = crate::reuse_build_inputs::buildroot_image_components(
                spec,
                matches!(kind, OperationKind::BuildImage),
            );
            if !content.is_empty() {
                format!("{content:?}").hash(&mut hasher);
            }
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
            // The disk the archive is built from is an input of this step.
            spec.image.output.disk.hash(&mut hasher);
            // Hashes the collected image, which the plan's image:build operation
            // also reads: computed once per plan.
            // Stored encoded (the memo holds strings), then hashed as the
            // `Option<String>` it decodes to, so the fingerprint is unchanged.
            let encoded = plan_memo(
                "build-image-output".into(),
                || match operation_output_signature(spec, &OperationKind::BuildImage) {
                    Some(signature) => format!("some\u{1f}{signature}"),
                    None => "none".into(),
                },
            );
            encoded
                .strip_prefix("some\u{1f}")
                .map(str::to_string)
                .hash(&mut hasher);
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

pub(crate) fn source_backend_signature(
    spec: &ResolvedBuildSpec,
    source: &gaia_spec::SourceSpec,
) -> String {
    match &source.definition {
        SourceDefinition::Git(git) => format!(
            "{}|{}",
            command_signature("git", ["--version"]),
            git_source_state_signature(spec, git)
        ),
        SourceDefinition::Archive(archive) => format!(
            "{}|{}",
            command_signature("tar", ["--version"]),
            path_state_signature(&resolve_workspace_path(spec, &archive.path))
        ),
        SourceDefinition::Download(_) => command_signature("curl", ["--version"]),
        SourceDefinition::Path(path) => format!(
            "path-source|{}",
            gaia_source_providers::path_source_fingerprint(
                &spec.workspace,
                &resolve_workspace_path(spec, &path.path),
                &path.identity_ignore,
            )
        ),
    }
}

/// Content hash of a Dockerfile-built execution image, so editing the
/// Dockerfile or its context rebuilds the artifact.
pub(crate) fn artifact_docker_build_signature(
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

pub(crate) fn image_backend_signature(
    spec: &ResolvedBuildSpec,
    image: &gaia_spec::ImageSpec,
) -> String {
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

/// How long a probed tool or image signature is reused within a process. Long
/// enough that planning probes each tool once (a planning pass, or the
/// variants of one run), short enough that a long-running TUI sees an upgraded
/// tool or a rebuilt image within a minute.
pub(crate) const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

/// Tool version signature, probed at most once per [`PROBE_CACHE_TTL`]:
/// planning fingerprints every artifact, and each probe would otherwise spawn
/// the tool again.
pub(crate) fn command_signature<const N: usize>(program: &str, args: [&str; N]) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();
    let key = std::iter::once(program)
        .chain(args)
        .collect::<Vec<_>>()
        .join("\u{1f}");
    let cache = CACHE.get_or_init(Default::default);
    if let Some(signature) = cache.lock().ok().and_then(|cache| fresh(cache.get(&key))) {
        return signature;
    }
    let signature = probe_command_signature(program, args);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, (Instant::now(), signature.clone()));
    }
    signature
}

/// The cached value when it was stored within [`PROBE_CACHE_TTL`].
pub(crate) fn fresh(entry: Option<&(Instant, String)>) -> Option<String> {
    entry
        .filter(|(stored, _)| stored.elapsed() < PROBE_CACHE_TTL)
        .map(|(_, value)| value.clone())
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

pub(crate) fn resolve_workspace_path(spec: &ResolvedBuildSpec, value: &str) -> PathBuf {
    gaia_spec::resolve_workspace_path(&spec.workspace, value).unwrap_or_else(|_| {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(path)
        }
    })
}

fn git_source_state_signature(spec: &ResolvedBuildSpec, git: &gaia_spec::GitSourceSpec) -> String {
    if let Some(locked_commit) = &git.locked_commit {
        // The checkout is fully determined by the locked commit; new commits
        // in a local repository do not change it.
        return format!("locked-git:{locked_commit}");
    }
    // A floating ref is identified by the commit it resolves to now, so a
    // moved branch changes the fingerprint and rebuilds what consumed it.
    if crate::reuse_floating::applies(spec, git)
        && let Ok(commit) = crate::reuse_floating::resolved_commit(git)
    {
        return format!("floating-git:{commit}");
    }
    if let Some(local_repo) = local_repo_path(&git.repo) {
        // Once per plan: several sources can share a repository.
        return plan_memo(format!("git-head\u{1f}{}", local_repo.display()), || {
            let mut command = Command::new("git");
            command
                .arg("-C")
                .arg(&local_repo)
                .arg("rev-parse")
                .arg("HEAD");
            match command.output() {
                Ok(output) if output.status.success() => format!(
                    "local-git:{}",
                    String::from_utf8_lossy(&output.stdout).trim()
                ),
                Ok(output) => format!("local-git:exit-{}", output.status),
                Err(error) => format!("local-git:unavailable:{error}"),
            }
        });
    }
    "remote-git".into()
}

pub(crate) fn local_repo_path(repo: &str) -> Option<PathBuf> {
    let file_repo = repo.strip_prefix("file://").map(PathBuf::from);
    let direct = PathBuf::from(repo);
    file_repo.or_else(|| direct.exists().then_some(direct))
}

thread_local! {
    /// Lookups made for the plan being built on this thread, or `None` outside
    /// a plan (see [`PlanMemoScope`]).
    static PLAN_MEMO: RefCell<Option<HashMap<String, String>>> = const { RefCell::new(None) };
}

/// Memoizes expensive lookups for the duration of one plan. Planning asks for
/// the same tree state (the Buildroot tree, a repository's HEAD) once per
/// operation that reads it, and the answer cannot change while the plan is
/// built. Outside a plan nothing is memoized, so a later plan always reads the
/// tree afresh. A scope begun inside another one changes nothing.
pub(crate) struct PlanMemoScope {
    owner: bool,
}

impl PlanMemoScope {
    pub(crate) fn begin() -> Self {
        PLAN_MEMO.with(|memo| {
            let mut memo = memo.borrow_mut();
            if memo.is_some() {
                return Self { owner: false };
            }
            *memo = Some(HashMap::new());
            Self { owner: true }
        })
    }
}

impl Drop for PlanMemoScope {
    fn drop(&mut self) {
        if self.owner {
            PLAN_MEMO.with(|memo| *memo.borrow_mut() = None);
        }
    }
}

/// `compute`, memoized under `key` within the current plan. Without a plan
/// in progress it is just `compute()`.
pub(crate) fn plan_memo(key: String, compute: impl FnOnce() -> String) -> String {
    let hit = PLAN_MEMO.with(|memo| {
        memo.borrow()
            .as_ref()
            .and_then(|memo| memo.get(&key).cloned())
    });
    if let Some(value) = hit {
        return value;
    }
    let value = compute();
    PLAN_MEMO.with(|memo| {
        if let Some(memo) = memo.borrow_mut().as_mut() {
            memo.insert(key, value.clone());
        }
    });
    value
}

pub(crate) fn path_state_signature(path: &Path) -> String {
    plan_memo(format!("path-state\u{1f}{}", path.display()), || {
        let mut hasher = DefaultHasher::new();
        hash_path_state(path, &mut hasher, &[]);
        format!("{:016x}", hasher.finish())
    })
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

fn operation_outputs_present(spec: &ResolvedBuildSpec, kind: &OperationKind) -> bool {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => true,
        OperationKind::MaterializeSource { source_id } => {
            gaia_spec::source_build_dir_by_id(spec, source_id.as_str())
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
        // What an install delivers is the artifact's bytes, not the install
        // record: a rebuilt artifact with new bytes must change this, or the
        // image built from the feed would be cut off as unchanged.
        OperationKind::InstallArtifact {
            install_id,
            artifact,
        } => Some(format!(
            "{}|{}",
            provider_state_signature(&install_state_path(spec, install_id)),
            spec.artifacts
                .iter()
                .find(|candidate| candidate.id == artifact.id)
                .map(|candidate| content_path_signature(&artifact_output_path(
                    spec,
                    &candidate.output.path
                )))
                .unwrap_or_else(|| "artifact-missing".into())
        )),
        // Prepare's signature is its Buildroot .config; build's is the
        // collected image content. Both are already content-based.
        OperationKind::PrepareImage | OperationKind::BuildImage => {
            operation_output_signature(spec, kind)
        }
        // Dependents consume a git source's checked-out tree, not its commit,
        // so a rev bump with identical files does not invalidate them.
        OperationKind::MaterializeSource { source_id } => {
            source_tree_content_signature(spec, source_id.as_str())
                .or_else(|| operation_output_signature(spec, kind))
        }
        // A rendered stage item delivers its entry and its source's bytes.
        OperationKind::RenderStageFile { item_id } => {
            let item = spec.stage.files.iter().find(|item| item.id == *item_id);
            Some(rendered_stage_content(
                spec,
                &stage_state_path(spec, "file", item_id),
                item.map(|item| format!("{item:?}")).unwrap_or_default(),
                item.map(|item| item.src.as_str()),
            ))
        }
        OperationKind::RenderStageEnvSet { item_id } => {
            let item = spec.stage.env_sets.iter().find(|item| item.id == *item_id);
            Some(rendered_stage_content(
                spec,
                &stage_state_path(spec, "env", item_id),
                item.map(|item| format!("{item:?}")).unwrap_or_default(),
                None,
            ))
        }
        OperationKind::RenderStageService { item_id } => {
            let item = spec.stage.services.iter().find(|item| item.id == *item_id);
            Some(rendered_stage_content(
                spec,
                &stage_state_path(spec, "service", item_id),
                item.map(|item| format!("{item:?}")).unwrap_or_default(),
                item.map(|item| item.unit_path.as_str()),
            ))
        }
        OperationKind::AssembleImage | OperationKind::CaptureCheckpoint { .. } => {
            operation_output_signature(spec, kind)
        }
    }
}

/// Signature of the content an operation consumes: the content signatures of
/// its direct dependencies (build resolution excluded).
pub fn operation_input_signature(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    operation: &PlannedOperation,
) -> u64 {
    input_signature_from(operation, |dependency| {
        plan.operations
            .iter()
            .find(|candidate| candidate.id == *dependency)
            .and_then(|candidate| operation_content_signature(spec, &candidate.kind))
    })
}

/// The input signature of `operation`, given the content signature of each
/// dependency. Shared by [`operation_input_signature`] and the per-plan cache,
/// so both produce the same value.
fn input_signature_from(
    operation: &PlannedOperation,
    mut content_of: impl FnMut(&OperationId) -> Option<String>,
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
        content_of(dependency).hash(&mut hasher);
    }
    hasher.finish()
}

/// Output and input signatures of one plan's operations, each computed at
/// most once. A stage item depends on every install, and an install's content
/// is its artifact's bytes, so without this one artifact is hashed once per
/// consumer.
pub(crate) struct PlanSignatures<'a> {
    spec: &'a ResolvedBuildSpec,
    plan: &'a ExecutionPlan,
    /// Operation id to its position in `plan.operations` (the first one wins).
    index: HashMap<&'a str, usize>,
    contents: RefCell<HashMap<usize, Option<String>>>,
    outputs: RefCell<HashMap<usize, Option<String>>>,
}

impl<'a> PlanSignatures<'a> {
    pub(crate) fn new(spec: &'a ResolvedBuildSpec, plan: &'a ExecutionPlan) -> Self {
        let mut index = HashMap::new();
        for (position, operation) in plan.operations.iter().enumerate() {
            index.entry(operation.id.as_str()).or_insert(position);
        }
        Self {
            spec,
            plan,
            index,
            contents: RefCell::default(),
            outputs: RefCell::default(),
        }
    }

    /// [`operation_output_signature`] of the operation at `position`.
    pub(crate) fn output_signature(&self, position: usize) -> Option<String> {
        cached(&self.outputs, position, || {
            operation_output_signature(self.spec, &self.plan.operations[position].kind)
        })
    }

    /// [`operation_content_signature`] of the operation at `position`.
    fn content_signature(&self, position: usize) -> Option<String> {
        cached(&self.contents, position, || {
            operation_content_signature(self.spec, &self.plan.operations[position].kind)
        })
    }

    /// [`operation_input_signature`] of the operation at `position`.
    pub(crate) fn input_signature(&self, position: usize) -> u64 {
        input_signature_from(&self.plan.operations[position], |dependency| {
            self.index
                .get(dependency.as_str())
                .and_then(|&found| self.content_signature(found))
        })
    }
}

/// `compute`, memoized in `cache` under `key`.
fn cached<K: Eq + std::hash::Hash>(
    cache: &RefCell<HashMap<K, Option<String>>>,
    key: K,
    compute: impl FnOnce() -> Option<String>,
) -> Option<String> {
    // The lookup borrow ends before `compute` runs, so `compute` may use the cache.
    let hit = cache.borrow().get(&key).cloned();
    if let Some(value) = hit {
        return value;
    }
    let value = compute();
    cache.borrow_mut().insert(key, value.clone());
    value
}

pub fn operation_output_signature(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<String> {
    match kind {
        OperationKind::ResolveBuild | OperationKind::EmitReport => None,
        OperationKind::MaterializeSource { source_id } => {
            let materialized_dir = gaia_spec::source_build_dir_by_id(spec, source_id.as_str());
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
        // Prepare's output is the Buildroot tree, summarized by its .config.
        // The collect dir state file is not part of it: the image build
        // rewrites that shared file on every run (archive hashes, mtime
        // digests), which would otherwise invalidate prepare's dependents.
        OperationKind::PrepareImage => spec
            .image
            .output
            .collect_dir
            .as_deref()
            .map(|_| content_state_signature(&buildroot_output_dir(spec).join(".config"))),
        // Content of the collected images and archive, never the provider's
        // rewritten state file (see reuse_content).
        OperationKind::BuildImage => content::image_build_content_signature(spec),
        OperationKind::AssembleImage => Some(content::image_assembly_content_signature(spec)),
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

pub(crate) fn provider_state_signature(path: &Path) -> String {
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
    resolve_workspace_path(spec, &spec.workspace.out_dir).join(gaia_spec::runtime_state_dir_name(
        spec.selection.variant.as_deref(),
    ))
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

#[path = "reuse_content.rs"]
pub(crate) mod content;

#[cfg(test)]
thread_local! {
    /// Content hashes of files and trees made on this thread (see
    /// `content_path_signature`), counted by the signature tests.
    pub(crate) static CONTENT_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "reuse_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "reuse_signature_tests.rs"]
mod signature_tests;
