#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsMutation {
    Install { target: String },
    Stage { target: String },
}

impl FsMutation {
    pub fn install(target: impl Into<String>) -> Self {
        Self::Install {
            target: target.into(),
        }
    }

    pub fn stage(target: impl Into<String>) -> Self {
        Self::Stage {
            target: target.into(),
        }
    }
}
