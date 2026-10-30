use crate::{IdError, InstallId, StageItemId};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CheckpointSpec {
    pub points: Vec<CheckpointPointSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckpointPointSpec {
    pub id: CheckpointId,
    pub backend: Option<CheckpointBackendRef>,
    pub use_policy: CheckpointPolicy,
    pub upload_policy: CheckpointPolicy,
    pub anchor: CheckpointAnchorRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct CheckpointId(String);

impl CheckpointId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn try_new(value: impl Into<String>) -> Result<Self, IdError> {
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckpointBackendRef {
    pub backend: String,
    /// The registry settings of `backend = "oci"`.
    pub oci: Option<OciCheckpointSpec>,
}

/// An OCI registry as a checkpoint backend: a cache of build products under
/// `<registry>/<repository>/<kind>:<key>`. Keys are resolved by the caller
/// (`env:VAR` names an environment variable that holds the key).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OciCheckpointSpec {
    pub registry: String,
    pub repository: String,
    pub verify_key: Option<String>,
    pub sign_key: Option<String>,
    pub sign_kind: Option<String>,
    /// `trust`: `"unsigned"` accepts pulls without a signature check (unsafe);
    /// validation rejects any other value.
    pub trust: Option<String>,
    /// `token`: `secret:NAME` or `env:VAR`, the registry token. It comes before
    /// `GAIA_REGISTRY_TOKEN`, the docker config and `gh`.
    pub token: Option<String>,
    /// `username`: the user name paired with `token`. `gaia` when not set.
    pub username: Option<String>,
}

impl OciCheckpointSpec {
    /// Whether pulls are accepted without a signature check.
    pub fn trust_unsigned(&self) -> bool {
        self.trust.as_deref() == Some("unsigned")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum CheckpointPolicy {
    #[default]
    Off,
    Auto,
    Always,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
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
