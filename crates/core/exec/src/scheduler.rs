use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use gaia_plan::{
    ExecutionPlan, OperationId, OperationParallelismDomain, OperationParallelismMode,
    PlannedOperation,
};
use gaia_process::ProcessCancelCheck;
use gaia_spec::{ArtifactDefinition, ResolvedBuildSpec};

use crate::ExecutionProviders;
use crate::operations::{ExecutionEvent, OperationExecutionResult, dispatch_operation};
use crate::runtime::ExecutionRuntime;

pub(crate) struct ScheduleReadyContext<'env> {
    pub(crate) spec: &'env ResolvedBuildSpec,
    pub(crate) plan: &'env ExecutionPlan,
    pub(crate) providers: &'env ExecutionProviders<'env>,
    pub(crate) build_name: &'env str,
    pub(crate) event_sender: Option<Sender<ExecutionEvent>>,
    pub(crate) cancel_check: ProcessCancelCheck,
    pub(crate) max_parallel_jobs: usize,
}

pub(crate) struct ScheduleReadyState<'a> {
    pub(crate) remaining_dependencies: &'a [usize],
    pub(crate) completed: &'a [bool],
    pub(crate) running: &'a mut [bool],
    pub(crate) running_count: &'a mut usize,
}

pub(crate) fn schedule_ready_operations<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    result_tx: &std::sync::mpsc::Sender<(usize, OperationExecutionResult)>,
    runtime: &mut ExecutionRuntime,
    context: &ScheduleReadyContext<'env>,
    state: ScheduleReadyState<'_>,
) -> bool {
    let mut scheduled_any = false;
    let spec = context.spec;
    let plan = context.plan;
    let providers = context.providers;
    let build_name = context.build_name;
    let event_sender = &context.event_sender;
    let cancel_check = &context.cancel_check;
    let max_parallel_jobs = context.max_parallel_jobs;
    let ScheduleReadyState {
        remaining_dependencies,
        completed,
        running,
        running_count,
    } = state;
    loop {
        if *running_count >= max_parallel_jobs {
            break;
        }
        let Some(index) =
            next_schedulable_operation(spec, plan, remaining_dependencies, completed, running)
        else {
            break;
        };
        let operation = &plan.operations[index];
        let tx = result_tx.clone();
        let operation_event_sender = event_sender.clone();
        let operation_cancel_check = cancel_check.clone();
        runtime.emit_event(ExecutionEvent::Started {
            operation_id: operation.id.clone(),
        });
        tracing::info!(
            operation_id = %operation.id.as_str(),
            operation_kind = ?operation.kind,
            parallelism_mode = ?operation.parallelism.mode,
            parallelism_domain = ?operation.parallelism.domain,
            running_operations = *running_count,
            max_parallel_jobs,
            "operation started"
        );
        running[index] = true;
        *running_count += 1;
        scope.spawn(move || {
            let result = dispatch_operation(
                operation,
                spec,
                providers,
                build_name,
                operation_event_sender,
                Some(operation_cancel_check),
            );
            let _ = tx.send((index, result));
        });
        scheduled_any = true;
        if !supports_parallel_runtime(
            operation.parallelism.mode.clone(),
            &operation.parallelism.domain,
        ) {
            break;
        }
    }
    scheduled_any
}

pub(crate) fn resolve_parallel_jobs(spec: &ResolvedBuildSpec) -> usize {
    if spec.policy.execution.jobs == 0 {
        thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .max(1)
    } else {
        usize::try_from(spec.policy.execution.jobs)
            .unwrap_or(1)
            .max(1)
    }
}

fn next_schedulable_operation(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    remaining_dependencies: &[usize],
    completed: &[bool],
    running: &[bool],
) -> Option<usize> {
    let mut ready_parallel = Vec::new();
    let mut ready_exclusive = Vec::new();
    let mut any_running = false;
    let mut exclusive_running = false;
    let running_parallel_resources = running
        .iter()
        .enumerate()
        .filter(|(_, is_running)| **is_running)
        .flat_map(|(index, _)| operation_parallel_resource_keys(spec, &plan.operations[index]))
        .collect::<HashSet<_>>();

    for (index, operation) in plan.operations.iter().enumerate() {
        if running[index] {
            any_running = true;
            if operation.parallelism.mode == OperationParallelismMode::Exclusive {
                exclusive_running = true;
            }
            continue;
        }
        if completed[index] || remaining_dependencies[index] != 0 {
            continue;
        }
        match operation.parallelism.mode {
            OperationParallelismMode::Parallelizable
                if supports_parallel_runtime(
                    operation.parallelism.mode.clone(),
                    &operation.parallelism.domain,
                ) =>
            {
                let resource_keys = operation_parallel_resource_keys(spec, operation);
                if resource_keys
                    .iter()
                    .all(|resource| !running_parallel_resources.contains(resource))
                {
                    ready_parallel.push(index)
                } else {
                    tracing::debug!(
                        operation_id = %operation.id.as_str(),
                        operation_kind = ?operation.kind,
                        resources = ?resource_keys
                            .iter()
                            .filter(|resource| running_parallel_resources.contains(resource))
                            .collect::<Vec<_>>(),
                        "operation waiting for parallel resource"
                    );
                    ready_exclusive.push(index)
                }
            }
            OperationParallelismMode::Exclusive => ready_exclusive.push(index),
            OperationParallelismMode::Parallelizable => ready_exclusive.push(index),
        }
    }

    if exclusive_running {
        return None;
    }
    if any_running {
        return ready_parallel.into_iter().next();
    }
    if let Some(index) = ready_exclusive.into_iter().next() {
        return Some(index);
    }
    ready_parallel.into_iter().next()
}

fn supports_parallel_runtime(
    mode: OperationParallelismMode,
    domain: &OperationParallelismDomain,
) -> bool {
    mode == OperationParallelismMode::Parallelizable
        && matches!(
            domain,
            OperationParallelismDomain::Sources
                | OperationParallelismDomain::Artifacts
                | OperationParallelismDomain::Runtime
                | OperationParallelismDomain::Images
                | OperationParallelismDomain::Checkpoints
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ParallelResourceKey {
    SourceMaterialization {
        source_id: String,
    },
    ArtifactBuildInput {
        provider: &'static str,
        package_root: ResourcePath,
    },
    ArtifactOutput {
        path: ResourcePath,
    },
    RuntimeDestination {
        kind: RuntimeDestinationKind,
        path: ResourcePath,
    },
    RuntimeName {
        kind: RuntimeNameKind,
        name: String,
    },
    ImageWorkspace,
    ImageOutput {
        path: ResourcePath,
    },
    Checkpoint {
        checkpoint_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResourcePath(PathBuf);

impl ResourcePath {
    fn host(root_dir: &str, path: impl AsRef<Path>) -> Self {
        let candidate = path.as_ref();
        let resolved = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            PathBuf::from(root_dir).join(candidate)
        };
        Self(std::fs::canonicalize(&resolved).unwrap_or(resolved))
    }

    fn virtual_path(path: impl AsRef<Path>) -> Self {
        Self(path.as_ref().to_path_buf())
    }

    fn package_root(source_dir: &ResourcePath, package_dir: Option<&str>) -> Self {
        let Some(package_dir) = package_dir.filter(|value| !value.trim().is_empty()) else {
            return source_dir.clone();
        };
        let package_dir = PathBuf::from(package_dir);
        if package_dir.is_absolute() {
            Self(package_dir)
        } else {
            Self(source_dir.0.join(package_dir))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RuntimeDestinationKind {
    Install,
    StageFile,
    StageService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RuntimeNameKind {
    StageEnv,
}

fn operation_parallel_resource_keys(
    spec: &ResolvedBuildSpec,
    operation: &PlannedOperation,
) -> Vec<ParallelResourceKey> {
    match &operation.kind {
        gaia_plan::OperationKind::MaterializeSource { source_id } => {
            vec![ParallelResourceKey::SourceMaterialization {
                source_id: source_id.as_str().to_string(),
            }]
        }
        gaia_plan::OperationKind::BuildArtifact { artifact_id } => {
            let Some(artifact) = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id == *artifact_id)
            else {
                return Vec::new();
            };
            let mut keys = vec![ParallelResourceKey::ArtifactOutput {
                path: ResourcePath::host(&spec.workspace.root_dir, &artifact.output.path),
            }];
            let Some(source_dir) = resolve_artifact_parallel_source_dir(spec, artifact) else {
                return keys;
            };
            match &artifact.definition {
                ArtifactDefinition::Rust(_)
                | ArtifactDefinition::Go(_)
                | ArtifactDefinition::Java(_) => {
                    keys.push(ParallelResourceKey::ArtifactBuildInput {
                        provider: artifact.provider_kind().as_str(),
                        package_root: ResourcePath::package_root(&source_dir, None),
                    });
                }
                ArtifactDefinition::Node(node) => {
                    keys.push(ParallelResourceKey::ArtifactBuildInput {
                        provider: artifact.provider_kind().as_str(),
                        package_root: ResourcePath::package_root(
                            &source_dir,
                            Some(&node.package_dir),
                        ),
                    });
                }
                ArtifactDefinition::Python(python) => {
                    keys.push(ParallelResourceKey::ArtifactBuildInput {
                        provider: artifact.provider_kind().as_str(),
                        package_root: ResourcePath::package_root(
                            &source_dir,
                            Some(&python.package_dir),
                        ),
                    });
                }
            }
            keys
        }
        gaia_plan::OperationKind::InstallArtifact { install_id, .. } => {
            let Some(install) = spec
                .install
                .entries
                .iter()
                .find(|install| install.id == *install_id)
            else {
                return Vec::new();
            };
            vec![ParallelResourceKey::RuntimeDestination {
                kind: RuntimeDestinationKind::Install,
                path: ResourcePath::virtual_path(&install.dest),
            }]
        }
        gaia_plan::OperationKind::RenderStageFile { item_id } => {
            let Some(file) = spec.stage.files.iter().find(|file| file.id == *item_id) else {
                return Vec::new();
            };
            vec![ParallelResourceKey::RuntimeDestination {
                kind: RuntimeDestinationKind::StageFile,
                path: ResourcePath::virtual_path(&file.dest),
            }]
        }
        gaia_plan::OperationKind::RenderStageEnvSet { item_id } => {
            let Some(env_set) = spec
                .stage
                .env_sets
                .iter()
                .find(|env_set| env_set.id == *item_id)
            else {
                return Vec::new();
            };
            vec![ParallelResourceKey::RuntimeName {
                kind: RuntimeNameKind::StageEnv,
                name: env_set.name.clone(),
            }]
        }
        gaia_plan::OperationKind::RenderStageService { item_id } => {
            let Some(service) = spec
                .stage
                .services
                .iter()
                .find(|service| service.id == *item_id)
            else {
                return Vec::new();
            };
            vec![ParallelResourceKey::RuntimeDestination {
                kind: RuntimeDestinationKind::StageService,
                path: ResourcePath::virtual_path(&service.unit_path),
            }]
        }
        gaia_plan::OperationKind::PrepareImage => vec![ParallelResourceKey::ImageWorkspace],
        gaia_plan::OperationKind::BuildImage => {
            let mut keys = vec![ParallelResourceKey::ImageWorkspace];
            if let Some(collect_dir) = &spec.image.output.collect_dir {
                keys.push(ParallelResourceKey::ImageOutput {
                    path: ResourcePath::host(&spec.workspace.root_dir, collect_dir),
                });
            }
            if let Some(archive_name) = &spec.image.output.archive_name
                && let Some(collect_dir) = &spec.image.output.collect_dir
            {
                let archive_path = std::path::PathBuf::from(collect_dir).join(archive_name);
                keys.push(ParallelResourceKey::ImageOutput {
                    path: ResourcePath::host(&spec.workspace.root_dir, archive_path),
                });
            }
            keys
        }
        gaia_plan::OperationKind::AssembleImage => vec![ParallelResourceKey::ImageWorkspace],
        gaia_plan::OperationKind::CaptureCheckpoint { checkpoint_id } => {
            vec![ParallelResourceKey::Checkpoint {
                checkpoint_id: checkpoint_id.as_str().to_string(),
            }]
        }
        _ => Vec::new(),
    }
}

fn resolve_artifact_parallel_source_dir(
    spec: &ResolvedBuildSpec,
    artifact: &gaia_spec::ArtifactSpec,
) -> Option<ResourcePath> {
    let source_ref = artifact.source.as_ref()?;
    let source = spec
        .sources
        .iter()
        .find(|source| source.id == source_ref.id)?;
    match &source.definition {
        gaia_spec::SourceDefinition::Path(path) => {
            let candidate = std::path::PathBuf::from(&path.path);
            let resolved = if candidate.is_absolute() {
                candidate
            } else {
                std::path::PathBuf::from(&spec.workspace.root_dir).join(candidate)
            };
            Some(ResourcePath::host(&spec.workspace.root_dir, resolved))
        }
        gaia_spec::SourceDefinition::Git(_)
        | gaia_spec::SourceDefinition::Archive(_)
        | gaia_spec::SourceDefinition::Download(_) => Some(ResourcePath::host(
            &spec.workspace.root_dir,
            PathBuf::from(&spec.workspace.build_dir)
                .join("sources")
                .join(source.id.as_str()),
        )),
    }
}

pub(crate) fn next_pending_operation_id(
    plan: &ExecutionPlan,
    completed: &[bool],
    running: &[bool],
) -> Option<OperationId> {
    plan.operations
        .iter()
        .enumerate()
        .find(|(index, _)| !completed[*index] && !running[*index])
        .map(|(_, operation)| operation.id.clone())
}

#[cfg(test)]
mod tests;
