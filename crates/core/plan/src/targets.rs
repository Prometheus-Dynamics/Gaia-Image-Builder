use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::{ExecutionPlan, OperationKind};

/// Part of the build graph to execute instead of the whole plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanTarget {
    /// Every operation in a build domain.
    Domain(PlanDomain),
    /// One operation by id, such as `artifact:helios-engine`.
    Operation(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanDomain {
    Sources,
    Artifacts,
    Install,
    Stage,
    Image,
    Checkpoints,
}

impl PlanDomain {
    pub const ALL: [Self; 6] = [
        Self::Sources,
        Self::Artifacts,
        Self::Install,
        Self::Stage,
        Self::Image,
        Self::Checkpoints,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sources => "sources",
            Self::Artifacts => "artifacts",
            Self::Install => "install",
            Self::Stage => "stage",
            Self::Image => "image",
            Self::Checkpoints => "checkpoints",
        }
    }

    pub fn contains(self, kind: &OperationKind) -> bool {
        match self {
            Self::Sources => matches!(kind, OperationKind::MaterializeSource { .. }),
            Self::Artifacts => matches!(kind, OperationKind::BuildArtifact { .. }),
            Self::Install => matches!(kind, OperationKind::InstallArtifact { .. }),
            Self::Stage => matches!(
                kind,
                OperationKind::RenderStageFile { .. }
                    | OperationKind::RenderStageEnvSet { .. }
                    | OperationKind::RenderStageService { .. }
            ),
            Self::Image => matches!(
                kind,
                OperationKind::PrepareImage
                    | OperationKind::BuildImage
                    | OperationKind::AssembleImage
            ),
            Self::Checkpoints => matches!(kind, OperationKind::CaptureCheckpoint { .. }),
        }
    }
}

impl FromStr for PlanTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let domain = match value {
            "source" | "sources" => Some(PlanDomain::Sources),
            "artifact" | "artifacts" => Some(PlanDomain::Artifacts),
            "install" | "installs" => Some(PlanDomain::Install),
            "stage" => Some(PlanDomain::Stage),
            "image" => Some(PlanDomain::Image),
            "checkpoint" | "checkpoints" => Some(PlanDomain::Checkpoints),
            _ => None,
        };
        if let Some(domain) = domain {
            return Ok(Self::Domain(domain));
        }
        if value.contains(':') {
            return Ok(Self::Operation(value.to_string()));
        }
        let domains = PlanDomain::ALL.map(PlanDomain::as_str).join(", ");
        Err(format!(
            "unknown target '{value}': expected a domain ({domains}) or an operation id such as 'artifact:<id>'"
        ))
    }
}

impl ExecutionPlan {
    /// Returns the operations matching `targets` plus everything they depend
    /// on, in plan order. Report emission is left out because it depends on
    /// every operation in the build.
    pub fn restrict_to(&self, targets: &[PlanTarget]) -> Result<ExecutionPlan, String> {
        for target in targets {
            if let PlanTarget::Operation(id) = target
                && !self
                    .operations
                    .iter()
                    .any(|operation| operation.id.as_str() == id)
            {
                return Err(format!("the plan has no operation '{id}'"));
            }
        }

        let dependencies: HashMap<&str, Vec<&str>> = self
            .operations
            .iter()
            .map(|operation| {
                (
                    operation.id.as_str(),
                    operation.depends_on.iter().map(|id| id.as_str()).collect(),
                )
            })
            .collect();
        let mut keep = HashSet::new();
        let mut pending: Vec<&str> = self
            .operations
            .iter()
            .filter(|operation| {
                targets.iter().any(|target| match target {
                    PlanTarget::Domain(domain) => domain.contains(&operation.kind),
                    PlanTarget::Operation(id) => operation.id.as_str() == id,
                })
            })
            .map(|operation| operation.id.as_str())
            .collect();
        if pending.is_empty() {
            let requested = targets
                .iter()
                .map(|target| match target {
                    PlanTarget::Domain(domain) => domain.as_str().to_string(),
                    PlanTarget::Operation(id) => id.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!("the plan has no operations in: {requested}"));
        }
        while let Some(id) = pending.pop() {
            if keep.insert(id) {
                pending.extend(dependencies.get(id).into_iter().flatten().copied());
            }
        }

        Ok(ExecutionPlan {
            build_id: self.build_id.clone(),
            operations: self
                .operations
                .iter()
                .filter(|operation| keep.contains(operation.id.as_str()))
                .cloned()
                .collect(),
        })
    }
}
