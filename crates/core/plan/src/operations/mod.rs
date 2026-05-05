use gaia_spec::{ArtifactId, ArtifactRef, CheckpointId, InstallId, SourceId, StageItemId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn resolve() -> Self {
        Self::new("resolve-build")
    }

    pub fn source(source_id: &SourceId) -> Self {
        Self::new(format!("source:{}", source_id.as_str()))
    }

    pub fn artifact(artifact_id: &ArtifactId) -> Self {
        Self::new(format!("artifact:{}", artifact_id.as_str()))
    }

    pub fn install(install_id: &InstallId) -> Self {
        Self::new(format!("install:{}", install_id.as_str()))
    }

    pub fn stage() -> Self {
        Self::new("stage:render")
    }

    pub fn stage_file(stage_item_id: &StageItemId) -> Self {
        Self::new(format!("stage:file:{}", stage_item_id.as_str()))
    }

    pub fn stage_env_set(stage_item_id: &StageItemId) -> Self {
        Self::new(format!("stage:env:{}", stage_item_id.as_str()))
    }

    pub fn stage_service(stage_item_id: &StageItemId) -> Self {
        Self::new(format!("stage:service:{}", stage_item_id.as_str()))
    }

    pub fn image() -> Self {
        Self::new("image:build")
    }

    pub fn image_prepare() -> Self {
        Self::new("image:prepare")
    }

    pub fn image_assembly() -> Self {
        Self::new("image:assembly")
    }

    pub fn checkpoint(checkpoint_id: &CheckpointId) -> Self {
        Self::new(format!("checkpoint:{}", checkpoint_id.as_str()))
    }

    pub fn report() -> Self {
        Self::new("report:emit")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for OperationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for OperationId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for OperationId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for OperationId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::str::FromStr for OperationId {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOperation {
    pub id: OperationId,
    pub kind: OperationKind,
    pub depends_on: Vec<OperationId>,
    pub parallelism: OperationParallelism,
    pub optionality: OperationOptionality,
    pub fingerprint: u64,
    pub reuse: OperationReuse,
}

impl PlannedOperation {
    pub fn new(id: OperationId, kind: OperationKind) -> Self {
        Self {
            id,
            kind,
            depends_on: Vec::new(),
            parallelism: OperationParallelism::default(),
            optionality: OperationOptionality::Required,
            fingerprint: 0,
            reuse: OperationReuse::execute("initial_plan", "operation will execute in this plan"),
        }
    }

    pub fn with_dependency(mut self, dependency: OperationId) -> Self {
        self.depends_on.push(dependency);
        self
    }

    pub fn with_reuse(mut self, reuse: OperationReuse) -> Self {
        self.reuse = reuse;
        self
    }

    pub fn with_fingerprint(mut self, fingerprint: u64) -> Self {
        self.fingerprint = fingerprint;
        self
    }

    pub fn with_parallelism(mut self, parallelism: OperationParallelism) -> Self {
        self.parallelism = parallelism;
        self
    }

    pub fn with_optionality(mut self, optionality: OperationOptionality) -> Self {
        self.optionality = optionality;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationParallelism {
    pub domain: OperationParallelismDomain,
    pub mode: OperationParallelismMode,
}

impl OperationParallelism {
    pub fn parallelizable(domain: OperationParallelismDomain) -> Self {
        Self {
            domain,
            mode: OperationParallelismMode::Parallelizable,
        }
    }

    pub fn exclusive(domain: OperationParallelismDomain) -> Self {
        Self {
            domain,
            mode: OperationParallelismMode::Exclusive,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OperationParallelismMode {
    #[default]
    Exclusive,
    Parallelizable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OperationParallelismDomain {
    #[default]
    Global,
    Sources,
    Artifacts,
    Runtime,
    Images,
    Checkpoints,
    Reporting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationOptionality {
    #[default]
    Required,
    Conditional,
    BestEffort,
}

impl OperationOptionality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Conditional => "conditional",
            Self::BestEffort => "best-effort",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationReuse {
    Execute(RebuildReason),
    Reuse { source: String },
}

impl OperationReuse {
    pub fn execute(code: &'static str, message: impl Into<String>) -> Self {
        Self::Execute(RebuildReason {
            code,
            message: message.into(),
        })
    }

    pub fn should_execute(&self) -> bool {
        matches!(self, Self::Execute(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReason {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationKind {
    ResolveBuild,
    MaterializeSource {
        source_id: SourceId,
    },
    BuildArtifact {
        artifact_id: ArtifactId,
    },
    InstallArtifact {
        install_id: InstallId,
        artifact: ArtifactRef,
    },
    RenderStageFile {
        item_id: StageItemId,
    },
    RenderStageEnvSet {
        item_id: StageItemId,
    },
    RenderStageService {
        item_id: StageItemId,
    },
    PrepareImage,
    BuildImage,
    AssembleImage,
    CaptureCheckpoint {
        checkpoint_id: CheckpointId,
    },
    EmitReport,
}
