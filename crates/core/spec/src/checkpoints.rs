use crate::{IdError, InstallId, StageItemId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckpointSpec {
    pub points: Vec<CheckpointPointSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointPointSpec {
    pub id: CheckpointId,
    pub backend: Option<CheckpointBackendRef>,
    pub use_policy: CheckpointPolicy,
    pub upload_policy: CheckpointPolicy,
    pub anchor: CheckpointAnchorRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckpointId(String);

impl CheckpointId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn try_new(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(IdError::empty())
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(&self) -> bool {
        !self.as_str().trim().is_empty()
    }
}

impl AsRef<str> for CheckpointId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for CheckpointId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for CheckpointId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for CheckpointId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::str::FromStr for CheckpointId {
    type Err = IdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointBackendRef {
    pub backend: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckpointPolicy {
    #[default]
    Off,
    Auto,
    Always,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CheckpointAnchorRef {
    #[default]
    Image,
    Install(InstallId),
    StageFile(StageItemId),
    StageEnvSet(StageItemId),
    StageService(StageItemId),
    Unknown(String),
}

impl CheckpointAnchorRef {
    pub fn as_str(&self) -> String {
        match self {
            Self::Image => "image".to_string(),
            Self::Install(id) => format!("install:{}", id.as_str()),
            Self::StageFile(id) => format!("stage-file:{}", id.as_str()),
            Self::StageEnvSet(id) => format!("stage-env:{}", id.as_str()),
            Self::StageService(id) => format!("stage-service:{}", id.as_str()),
            Self::Unknown(value) => value.clone(),
        }
    }
}

impl std::fmt::Display for CheckpointAnchorRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_id_rejects_empty_fallible_construction() {
        assert_eq!(CheckpointId::try_new(""), Err(IdError::empty()));
        assert_eq!(CheckpointId::try_new(" \t\n"), Err(IdError::empty()));
        assert!("".parse::<CheckpointId>().is_err());
        assert!(!CheckpointId::new("").is_valid());
    }
}
