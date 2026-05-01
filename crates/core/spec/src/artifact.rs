use crate::{ArtifactId, SourceRef};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSpec {
    pub id: ArtifactId,
    pub definition: ArtifactDefinition,
    pub source: Option<SourceRef>,
    pub execution: Option<ArtifactExecutionSpec>,
    pub target: Option<String>,
    pub build_mode: Option<BuildModeSpec>,
    pub dependencies: Vec<ArtifactRef>,
    pub output: ArtifactOutputSpec,
    pub install_identity: Option<ArtifactInstallIdentitySpec>,
}

impl ArtifactSpec {
    pub fn new(
        id: impl Into<ArtifactId>,
        definition: ArtifactDefinition,
        source: Option<SourceRef>,
        output: ArtifactOutputSpec,
    ) -> Self {
        Self {
            id: id.into(),
            definition,
            source,
            execution: None,
            target: None,
            build_mode: None,
            dependencies: Vec::new(),
            output,
            install_identity: None,
        }
    }

    pub fn provider_kind(&self) -> ArtifactProviderKind {
        self.definition.provider_kind()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactExecutionSpec {
    Host,
    Docker(DockerArtifactExecutionSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerArtifactExecutionSpec {
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactDefinition {
    Rust(RustArtifactSpec),
    Java(JavaArtifactSpec),
    Node(NodeArtifactSpec),
    Python(PythonArtifactSpec),
    Go(GoArtifactSpec),
}

impl ArtifactDefinition {
    pub fn provider_kind(&self) -> ArtifactProviderKind {
        match self {
            Self::Rust(_) => ArtifactProviderKind::Rust,
            Self::Java(_) => ArtifactProviderKind::Java,
            Self::Node(_) => ArtifactProviderKind::Node,
            Self::Python(_) => ArtifactProviderKind::Python,
            Self::Go(_) => ArtifactProviderKind::Go,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustArtifactSpec {
    pub package: String,
    pub target_name: Option<String>,
    pub variant: ArtifactVariantSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaArtifactSpec {
    pub build_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeArtifactSpec {
    pub package_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonArtifactSpec {
    pub package_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoArtifactSpec {
    pub package: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactVariantSpec {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildModeSpec {
    Debug,
    Release,
    Custom(String),
}

impl BuildModeSpec {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
            Self::Custom(mode) => mode.as_str(),
        }
    }
}

impl std::fmt::Display for BuildModeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for BuildModeSpec {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "debug" => Self::Debug,
            "release" => Self::Release,
            custom => Self::Custom(custom.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOutputSpec {
    pub path: String,
}

impl ArtifactOutputSpec {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.path)
    }
}

impl AsRef<Path> for ArtifactOutputSpec {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInstallIdentitySpec {
    pub install_name: String,
    pub install_class: ArtifactInstallClassSpec,
    pub destination_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactInstallClassSpec {
    Binary,
    Library,
    Archive,
    Config,
    Service,
    Data,
}

impl ArtifactInstallClassSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Library => "library",
            Self::Archive => "archive",
            Self::Config => "config",
            Self::Service => "service",
            Self::Data => "data",
        }
    }
}

impl std::fmt::Display for ArtifactInstallClassSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactProviderKind {
    Rust,
    Java,
    Node,
    Python,
    Go,
}

impl ArtifactProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Java => "java",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
        }
    }
}

impl std::fmt::Display for ArtifactProviderKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ArtifactProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "rust" => Ok(Self::Rust),
            "java" => Ok(Self::Java),
            "node" => Ok(Self::Node),
            "python" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            other => Err(format!("unknown artifact provider kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactRef {
    pub id: ArtifactId,
}

impl ArtifactRef {
    pub fn new(id: impl Into<ArtifactId>) -> Self {
        Self { id: id.into() }
    }
}

impl From<ArtifactId> for ArtifactRef {
    fn from(id: ArtifactId) -> Self {
        Self::new(id)
    }
}

impl From<&str> for ArtifactRef {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for ArtifactRef {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_mode_and_provider_kind_parse_from_strings() {
        assert_eq!("debug".parse::<BuildModeSpec>(), Ok(BuildModeSpec::Debug));
        assert_eq!(
            "release".parse::<BuildModeSpec>(),
            Ok(BuildModeSpec::Release)
        );
        assert_eq!(
            "profiled".parse::<BuildModeSpec>(),
            Ok(BuildModeSpec::Custom("profiled".into()))
        );

        assert_eq!(
            "python".parse::<ArtifactProviderKind>(),
            Ok(ArtifactProviderKind::Python)
        );
        assert!("unknown".parse::<ArtifactProviderKind>().is_err());
    }
}
