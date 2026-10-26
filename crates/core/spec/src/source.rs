use crate::SourceId;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceSpec {
    pub id: SourceId,
    pub definition: SourceDefinition,
    /// The variant this source is checked out for, when the variants of a
    /// build check it out differently (a changed rev, for example). Its
    /// files then live under `sources/variants/<variant>/<id>`; `None` keeps
    /// the shared `sources/<id>`.
    pub variant_scope: Option<String>,
}

impl SourceSpec {
    pub fn new(id: impl Into<SourceId>, definition: SourceDefinition) -> Self {
        Self {
            id: id.into(),
            definition,
            variant_scope: None,
        }
    }

    pub fn provider_kind(&self) -> SourceProviderKind {
        self.definition.provider_kind()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GitSourceSpec {
    pub repo: String,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    pub subdir: Option<String>,
    pub update: bool,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
    /// Commit recorded for this source in the build's lockfile. When set,
    /// materialization checks out exactly this commit instead of the
    /// configured branch/tag tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_commit: Option<String>,
    /// Where the git commands for this source run. Not serialized, so not
    /// part of operation fingerprints: where git runs does not change the
    /// checkout it produces.
    #[serde(skip)]
    pub git_backend: GitBackendSpec,
}

/// Where a git source's git commands (clone, fetch, ls-remote, checkout) run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum GitBackendSpec {
    /// On the host, with the user's environment: git config, credential
    /// helpers, ssh agent and `GIT_*` variables.
    #[default]
    Host,
    /// Through the build's execution backend (docker when configured).
    Execution,
}

impl GitBackendSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Execution => "execution",
        }
    }
}

impl GitSourceSpec {
    /// The configured selector as `<kind>:<value>` (`branch:main`,
    /// `tag:v1`, `rev:abc`, or `head:HEAD`), used to key lockfile entries.
    pub fn ref_selector(&self) -> String {
        if let Some(branch) = &self.branch {
            format!("branch:{branch}")
        } else if let Some(tag) = &self.tag {
            format!("tag:{tag}")
        } else if let Some(rev) = &self.rev {
            format!("rev:{rev}")
        } else {
            "head:HEAD".into()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArchiveSourceSpec {
    /// The archive file, as a workspace path. Empty when the archive is
    /// downloaded from `url` instead.
    pub path: String,
    /// Where the archive is downloaded from (`url`), instead of `path`. The
    /// download goes to the shared download cache, keyed by `sha256`.
    /// Not serialized when unset, so the fingerprints of `path` archives keep.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The archive's SHA-256, lowercase hex. Required with `url` and checked
    /// before the archive is extracted; not allowed with `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub strip_components: u32,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

impl ArchiveSourceSpec {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.path)
    }

    /// Whether the archive is downloaded from `url` rather than read from `path`.
    pub fn is_url(&self) -> bool {
        self.url.is_some()
    }
}

impl AsRef<Path> for ArchiveSourceSpec {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DownloadSourceSpec {
    pub url: String,
    pub sha256: Option<String>,
    pub output_path: String,
    pub refresh_policy: SourceRefreshPolicySpec,
    pub pin_policy: SourcePinPolicySpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
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
