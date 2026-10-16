use crate::{ArtifactId, SourceId, SourceRef};
use std::path::Path;

#[derive(Clone, PartialEq, Eq)]
pub struct ArtifactSpec {
    pub id: ArtifactId,
    pub definition: ArtifactDefinition,
    pub source: Option<SourceRef>,
    /// Sources other than `source` the artifact reads: named by `sources`
    /// in its config, or by a `${source.<id>.*}` token in its build fields.
    /// Sorted and without duplicates; the artifact depends on each source.
    pub extra_sources: Vec<SourceId>,
    pub execution: Option<ArtifactExecutionSpec>,
    pub target: Option<String>,
    pub build_mode: Option<BuildModeSpec>,
    pub dependencies: Vec<ArtifactRef>,
    pub after_image_prepare: bool,
    pub output: ArtifactOutputSpec,
    pub install_identity: Option<ArtifactInstallIdentitySpec>,
}

// Operation fingerprints hash the Debug output of specs. `extra_sources` is
// only printed when set, so artifacts without extra sources keep the
// fingerprints they had before the field existed.
impl std::fmt::Debug for ArtifactSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("ArtifactSpec");
        debug
            .field("id", &self.id)
            .field("definition", &self.definition)
            .field("source", &self.source)
            .field("execution", &self.execution)
            .field("target", &self.target)
            .field("build_mode", &self.build_mode)
            .field("dependencies", &self.dependencies)
            .field("after_image_prepare", &self.after_image_prepare)
            .field("output", &self.output)
            .field("install_identity", &self.install_identity);
        if !self.extra_sources.is_empty() {
            debug.field("extra_sources", &self.extra_sources);
        }
        debug.finish()
    }
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
            extra_sources: Vec::new(),
            execution: None,
            target: None,
            build_mode: None,
            dependencies: Vec::new(),
            after_image_prepare: false,
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

#[derive(Clone, Default, PartialEq, Eq)]
pub struct DockerArtifactExecutionSpec {
    pub image: Option<String>,
    /// Dockerfile Gaia builds the execution image from (workspace path).
    pub dockerfile: Option<String>,
    /// Build context for `dockerfile`; defaults to the Dockerfile's directory.
    pub context: Option<String>,
}

// Operation fingerprints hash the Debug output of specs. The Dockerfile
// fields are only printed when set, so image-only artifacts keep the
// fingerprints they had before Dockerfile support existed.
impl std::fmt::Debug for DockerArtifactExecutionSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("DockerArtifactExecutionSpec");
        debug.field("image", &self.image);
        if let Some(dockerfile) = &self.dockerfile {
            debug.field("dockerfile", dockerfile);
        }
        if let Some(context) = &self.context {
            debug.field("context", context);
        }
        debug.finish()
    }
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

#[derive(Clone, PartialEq, Eq)]
pub struct RustArtifactSpec {
    pub package: String,
    pub target_name: Option<String>,
    pub variant: ArtifactVariantSpec,
    /// Passed to cargo as `--features a,b`. For a build group member this is
    /// the sorted union of every member's features.
    pub features: Vec<String>,
    /// Passed to cargo as `--no-default-features`.
    pub no_default_features: bool,
    /// Passed to cargo as `--all-features`; exclusive with the two above.
    pub all_features: bool,
    /// Rust artifacts sharing a build group are always built by one cargo
    /// invocation, so cargo resolves features once for all of them.
    pub build_group: Option<String>,
    /// Sorted packages of every member of `build_group` (this one
    /// included); each is passed to cargo as `-p`. Empty outside a group.
    pub group_packages: Vec<String>,
    /// Cargo git dependencies served from declared git sources, sorted by
    /// `url`. Empty unless the artifact sets `cargo_git_sources`.
    pub cargo_git_sources: Vec<CargoGitSourceSpec>,
}

/// One cargo git dependency the build takes from a declared git source
/// instead of the network: the repository URL as Cargo.lock records it, and
/// the git source whose mirror holds the commits the lock pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoGitSourceSpec {
    /// The repository URL as written in the workspace's Cargo.lock (or in
    /// the artifact's `cargo_git_sources` key; URLs match with and without
    /// `git+`, a trailing `.git` or `/`).
    pub url: String,
    /// The declared `kind = "git"` source that supplies the commits.
    pub source: SourceId,
    /// That source's repository URL, which keys its mirror.
    pub repo: String,
}

/// The form cargo git URLs are compared in: without a `git+` prefix, a
/// `?query` or `#fragment`, a trailing `/` and a trailing `.git`. Cargo.lock
/// writes `git+https://host/repo?branch=main#sha`, and the key of
/// `cargo_git_sources` may be written with or without `.git`.
pub fn cargo_git_url_key(url: &str) -> String {
    let url = url.trim();
    let url = url.strip_prefix("git+").unwrap_or(url);
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    url.trim_end_matches('/').to_string()
}

impl RustArtifactSpec {
    /// Packages one cargo invocation for this artifact selects: the whole
    /// build group, or just the artifact's own package.
    pub fn cargo_packages(&self) -> Vec<String> {
        if self.group_packages.is_empty() {
            vec![self.package.clone()]
        } else {
            self.group_packages.clone()
        }
    }
}

// Operation fingerprints hash the Debug output of specs. The build group
// fields are only printed when set, so artifacts outside a group keep the
// fingerprints they had before build groups existed.
impl std::fmt::Debug for RustArtifactSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("RustArtifactSpec");
        debug
            .field("package", &self.package)
            .field("target_name", &self.target_name)
            .field("variant", &self.variant)
            .field("features", &self.features)
            .field("no_default_features", &self.no_default_features)
            .field("all_features", &self.all_features);
        if let Some(build_group) = &self.build_group {
            debug.field("build_group", build_group);
        }
        if !self.group_packages.is_empty() {
            debug.field("group_packages", &self.group_packages);
        }
        if !self.cargo_git_sources.is_empty() {
            debug.field("cargo_git_sources", &self.cargo_git_sources);
        }
        debug.finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaArtifactSpec {
    pub build_target: String,
    pub build_args: Vec<String>,
    pub build_command: Vec<String>,
    pub build_env: Vec<(String, String)>,
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

    #[test]
    fn artifact_spec_debug_prints_extra_sources_only_when_set() {
        let mut spec = ArtifactSpec::new(
            "app",
            ArtifactDefinition::Go(GoArtifactSpec {
                package: "./cmd/app".into(),
            }),
            None,
            ArtifactOutputSpec {
                path: "out/app".into(),
            },
        );
        // What `#[derive(Debug)]` printed before `extra_sources` existed.
        let derived = format!(
            "ArtifactSpec {{ id: {:?}, definition: {:?}, source: {:?}, execution: {:?}, target: {:?}, build_mode: {:?}, dependencies: {:?}, after_image_prepare: {:?}, output: {:?}, install_identity: {:?} }}",
            spec.id,
            spec.definition,
            spec.source,
            spec.execution,
            spec.target,
            spec.build_mode,
            spec.dependencies,
            spec.after_image_prepare,
            spec.output,
            spec.install_identity
        );
        assert_eq!(format!("{spec:?}"), derived);

        spec.extra_sources = vec![SourceId::new("eidos")];
        let debug = format!("{spec:?}");
        assert!(debug.ends_with("install_identity: None, extra_sources: [SourceId(\"eidos\")] }"));
    }

    fn rust_spec() -> RustArtifactSpec {
        RustArtifactSpec {
            package: "engine".into(),
            target_name: Some("libengine.so".into()),
            variant: ArtifactVariantSpec::File,
            features: vec!["tls".into()],
            no_default_features: true,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
            cargo_git_sources: Vec::new(),
        }
    }

    #[test]
    fn rust_spec_debug_matches_the_derived_format_outside_a_group() {
        let spec = rust_spec();
        // What `#[derive(Debug)]` printed before build groups existed.
        let derived = format!(
            "RustArtifactSpec {{ package: {:?}, target_name: {:?}, variant: {:?}, features: {:?}, no_default_features: {:?}, all_features: {:?} }}",
            spec.package,
            spec.target_name,
            spec.variant,
            spec.features,
            spec.no_default_features,
            spec.all_features
        );
        assert_eq!(format!("{spec:?}"), derived);
        let pretty = format!("{spec:#?}");
        assert!(!pretty.contains("build_group") && !pretty.contains("group_packages"));
    }

    #[test]
    fn rust_spec_debug_includes_group_fields_when_set() {
        let mut spec = rust_spec();
        spec.build_group = Some("engine".into());
        spec.group_packages = vec!["engine".into(), "plugin".into()];
        let debug = format!("{spec:?}");
        assert!(debug.ends_with(
            "all_features: false, build_group: \"engine\", group_packages: [\"engine\", \"plugin\"] }"
        ));
        assert_eq!(spec.cargo_packages(), spec.group_packages);
        assert_eq!(rust_spec().cargo_packages(), vec!["engine".to_string()]);
    }

    #[test]
    fn cargo_git_url_key_ignores_prefix_query_slash_and_dot_git() {
        let key = "https://github.com/Prometheus-Dynamics/Eidos";
        for url in [
            "https://github.com/Prometheus-Dynamics/Eidos",
            "https://github.com/Prometheus-Dynamics/Eidos.git",
            "https://github.com/Prometheus-Dynamics/Eidos/",
            "git+https://github.com/Prometheus-Dynamics/Eidos.git?branch=main#0123",
            "git+https://github.com/Prometheus-Dynamics/Eidos?rev=abc",
        ] {
            assert_eq!(cargo_git_url_key(url), key, "{url}");
        }
        assert_ne!(
            cargo_git_url_key("https://github.com/Prometheus-Dynamics/Eidos-core.git"),
            key
        );
    }

    #[test]
    fn rust_spec_debug_includes_cargo_git_sources_only_when_set() {
        let mut spec = rust_spec();
        let plain = format!("{spec:?}");
        assert!(!plain.contains("cargo_git_sources"));
        spec.cargo_git_sources = vec![CargoGitSourceSpec {
            url: "https://example.com/dep.git".into(),
            source: SourceId::new("dep"),
            repo: "https://example.com/dep.git".into(),
        }];
        let debug = format!("{spec:?}");
        assert!(debug.contains(
            "cargo_git_sources: [CargoGitSourceSpec { url: \"https://example.com/dep.git\""
        ));
        assert!(debug.ends_with("}] }"));
    }
}
