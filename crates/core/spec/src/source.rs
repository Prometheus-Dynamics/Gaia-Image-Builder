use crate::SourceId;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub id: SourceId,
    pub definition: SourceDefinition,
}

impl SourceSpec {
    pub fn new(id: impl Into<SourceId>, definition: SourceDefinition) -> Self {
        Self {
            id: id.into(),
            definition,
        }
    }

    pub fn provider_kind(&self) -> SourceProviderKind {
        self.definition.provider_kind()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDefinition {
    Git(GitSourceSpec),
    Path(PathSourceSpec),
    Archive(ArchiveSourceSpec),
    Download(DownloadSourceSpec),
}

impl SourceDefinition {
    pub fn provider_kind(&self) -> SourceProviderKind {
        match self {
            Self::Git(_) => SourceProviderKind::Git,
            Self::Path(_) => SourceProviderKind::Path,
            Self::Archive(_) => SourceProviderKind::Archive,
            Self::Download(_) => SourceProviderKind::Download,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSourceSpec {
    pub repo: String,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    pub subdir: Option<String>,
    pub update: bool,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSourceSpec {
    pub path: String,
    pub identity_ignore: Vec<String>,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

impl PathSourceSpec {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.path)
    }
}

impl AsRef<Path> for PathSourceSpec {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSourceSpec {
    pub path: String,
    pub strip_components: u32,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

impl ArchiveSourceSpec {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.path)
    }
}

impl AsRef<Path> for ArchiveSourceSpec {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadSourceSpec {
    pub url: String,
    pub sha256: Option<String>,
    pub output_path: String,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRefreshPolicySpec {
    Auto,
    Always,
    Never,
}

impl SourceRefreshPolicySpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

impl std::fmt::Display for SourceRefreshPolicySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePinPolicySpec {
    Floating,
    Locked,
}

impl SourcePinPolicySpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Floating => "floating",
            Self::Locked => "locked",
        }
    }
}

impl std::fmt::Display for SourcePinPolicySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceProviderKind {
    Git,
    Path,
    Archive,
    Download,
}

impl SourceProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Path => "path",
            Self::Archive => "archive",
            Self::Download => "download",
        }
    }
}

impl std::fmt::Display for SourceProviderKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for SourceProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "git" => Ok(Self::Git),
            "path" => Ok(Self::Path),
            "archive" => Ok(Self::Archive),
            "download" => Ok(Self::Download),
            other => Err(format!("unknown source provider kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub id: SourceId,
}

impl SourceRef {
    pub fn new(id: impl Into<SourceId>) -> Self {
        Self { id: id.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_provider_kind_parses_from_strings() {
        assert_eq!(
            "git".parse::<SourceProviderKind>(),
            Ok(SourceProviderKind::Git)
        );
        assert_eq!(
            "download".parse::<SourceProviderKind>(),
            Ok(SourceProviderKind::Download)
        );
        assert!("unknown".parse::<SourceProviderKind>().is_err());
    }
}
