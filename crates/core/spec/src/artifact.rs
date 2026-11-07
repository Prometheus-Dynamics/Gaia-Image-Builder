use crate::{ArtifactId, SourceId, SourceRef};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactSpec {
    pub id: ArtifactId,
    pub definition: ArtifactDefinition,
    pub source: Option<SourceRef>,
    /// Sources other than `source` the artifact reads: named by `sources`
    /// in its config, or by a `${source.<id>.*}` token in its build fields.
    /// Sorted and without duplicates; the artifact depends on each source.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_sources: Vec<SourceId>,
    pub execution: Option<ArtifactExecutionSpec>,
    pub target: Option<String>,
    /// The `[[toolchains]]` id a rust artifact builds with (see
    /// `ResolvedToolchain`). Not serialized when unset, so the fingerprints of
    /// artifacts without one keep.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    pub build_mode: Option<BuildModeSpec>,
    pub dependencies: Vec<ArtifactRef>,
    pub after_image_prepare: bool,
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
            extra_sources: Vec::new(),
            execution: None,
            target: None,
            toolchain: None,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ArtifactExecutionSpec {
    Host,
    Docker(DockerArtifactExecutionSpec),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DockerArtifactExecutionSpec {
    pub image: Option<String>,
    /// Dockerfile Gaia builds the execution image from (workspace path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    /// Build context for `dockerfile`; defaults to the Dockerfile's directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ArtifactDefinition {
    Rust(RustArtifactSpec),
    Java(JavaArtifactSpec),
    Node(NodeArtifactSpec),
    Python(PythonArtifactSpec),
    Go(GoArtifactSpec),
    Command(CommandArtifactSpec),
}

impl ArtifactDefinition {
    pub fn provider_kind(&self) -> ArtifactProviderKind {
        match self {
            Self::Rust(_) => ArtifactProviderKind::Rust,
            Self::Java(_) => ArtifactProviderKind::Java,
            Self::Node(_) => ArtifactProviderKind::Node,
            Self::Python(_) => ArtifactProviderKind::Python,
            Self::Go(_) => ArtifactProviderKind::Go,
            Self::Command(_) => ArtifactProviderKind::Command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_group: Option<String>,
    /// Sorted packages of every member of `build_group` (this one
    /// included); each is passed to cargo as `-p`. Empty outside a group.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub group_packages: Vec<String>,
    /// Cargo git dependencies served from declared git sources, sorted by
    /// `url`. Empty unless the artifact sets `cargo_git_sources`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cargo_git_sources: Vec<CargoGitSourceSpec>,
    /// Variables set for cargo, in order; `${secret:NAME}` in a value is read
    /// when cargo starts. Gaia's own variables (toolchain, sccache, offline)
    /// win over these. Every member of a build group sets the same ones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub build_env: Vec<(String, String)>,
    /// The flags of the build's `[cpu]` profile, when the artifact builds
    /// for the profile's architecture. Not serialized when unset, so the
    /// fingerprints of builds without a profile keep.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<crate::RustCpuSpec>,
    /// How the artifact links Rust crates. Not serialized when static, so the
    /// fingerprints of builds that never set it keep.
    #[serde(skip_serializing_if = "RustLinkSpec::is_static")]
    pub link: RustLinkSpec,
    /// The C ABI a `cdylib` declares it exports; checked after the build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<Box<RustSurfaceSpec>>,
    /// The library links of the artifact's build group that Gaia records
    /// and, for the library that trims, relinks. Every member of the group
    /// carries it, so the group's one cargo build is the same for each.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim: Option<Box<RustTrimSpec>>,
}

/// A shared library relinked to export only what its users import. Cargo
/// links it as usual through a wrapper that records the link; Gaia then
/// replays that link with rustc's version script cut down to the names the
/// users import, and `--gc-sections`, so the code only the dropped exports
/// reached goes too.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RustTrimSpec {
    /// The artifacts of the build group whose imports the library keeps;
    /// empty on a member that is not itself trimmed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<String>,
    /// The file each user's cargo build writes (`helios-api`,
    /// `libplugin.so`), in the order of `users`: Gaia reads their imports
    /// there, whether or not they are rebuilt with the library.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user_files: Vec<String>,
    /// The file names (`libfoo.so`) of the group's trimmed libraries, whose
    /// links the wrapper records. Sorted.
    pub recorded: Vec<String>,
}

/// A shared library's declared surface: the symbols it exports and nothing
/// else. The library's own build keeps everything else local (a version
/// script); Gaia only checks the built file against the list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RustSurfaceSpec {
    /// The list, relative to the artifact's source: one symbol per line
    /// (`#` starts a comment), or a linker version script's `global:` names.
    pub exports: String,
    /// The crates holding the surface's process-wide state (its handle
    /// tables). A program that links one of them statically must not also
    /// load the surface library: handles from one copy are unknown to the
    /// other. Empty means the artifact's own package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub state_crates: Vec<String>,
    /// The feature with which the surface package's Rust wrapper calls the
    /// installed library instead of linking the surface in (Lemnos's
    /// `shared`): a program with it on holds no copy of the state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_feature: Option<String>,
    /// Directories in the image (`/usr/lib/helios/plugins`) of plugins the
    /// surface opens at run time (`dlopen`): each ELF file under them is a
    /// consumer of the surface, and runs in the process of each program
    /// that loads it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugin_dirs: Vec<String>,
    /// The symbols each plugin there must export: the plugin ABI's entry
    /// points (Daedalus's `daedalus_plugin_descriptor`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugin_exports: Vec<String>,
    /// The ABI version each plugin there exports as a `u32` constant, and
    /// the range the surface opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<SurfacePluginVersionSpec>,
}

/// A plugin's ABI version: the exported `u32` constant that holds it
/// (Daedalus's `daedalus_plugin_stable_version`) and the accepted range,
/// both ends included. Each end is a fixed number, or the name of a `u32`
/// constant the surface library itself exports
/// (`daedalus_surface_stable_abi_min`/`_max`), so the range follows the
/// surface the image ships; a fixed end wins. A plugin without the symbol
/// is not checked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SurfacePluginVersionSpec {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_min: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_max: Option<String>,
}

impl RustSurfaceSpec {
    pub fn new(exports: impl Into<String>) -> Self {
        Self {
            exports: exports.into(),
            state_crates: Vec::new(),
            shared_feature: None,
            plugin_dirs: Vec::new(),
            plugin_exports: Vec::new(),
            plugin_version: None,
        }
    }
}

/// Where a Rust artifact's resolved crate graph is recorded, next to its
/// output (`<dir>/.gaia/<file name>.crates.json`): written when the build
/// declares a surface, read by the image checks.
pub fn rust_crates_record_path(output_path: &std::path::Path) -> std::path::PathBuf {
    output_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(".gaia")
        .join(format!(
            "{}.crates.json",
            output_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact")
        ))
}

/// How a Rust artifact links the Rust crates it uses, the standard library
/// included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RustLinkSpec {
    /// Every crate is linked into the artifact (rustc's default).
    #[default]
    Static,
    /// Crates a Rust `dylib` of the build provides, and the standard library,
    /// are linked dynamically (`-C prefer-dynamic`).
    Shared,
}

impl RustLinkSpec {
    pub fn is_static(&self) -> bool {
        *self == Self::Static
    }
}

/// The `package` that names the toolchain's standard library rather than a
/// cargo package (cargo rejects `std` as a package name): the artifact is a
/// directory holding the target's `libstd-<hash>.so`.
pub const RUST_STD_PACKAGE: &str = "std";

/// One cargo git dependency the build takes from a declared git source
/// instead of the network: the repository URL as Cargo.lock records it, and
/// the git source whose mirror holds the commits the lock pins.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    /// Whether the artifact is the toolchain's standard library
    /// ([`RUST_STD_PACKAGE`]), which no cargo invocation builds.
    pub fn is_std_library(&self) -> bool {
        self.package == RUST_STD_PACKAGE
    }

    /// Packages one cargo invocation for this artifact selects: the whole
    /// build group, or just the artifact's own package.
    pub fn cargo_packages(&self) -> Vec<String> {
        if self.is_std_library() {
            Vec::new()
        } else if self.group_packages.is_empty() {
            vec![self.package.clone()]
        } else {
            self.group_packages.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct JavaArtifactSpec {
    pub build_target: String,
    pub build_args: Vec<String>,
    pub build_command: Vec<String>,
    pub build_env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NodeArtifactSpec {
    pub package_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PythonArtifactSpec {
    pub package_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GoArtifactSpec {
    pub package: String,
}

/// A host command run in its source's directory (`kind = "command"`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommandArtifactSpec {
    /// The program and its arguments.
    pub command: Vec<String>,
    /// Environment set for the command, in order.
    pub build_env: Vec<(String, String)>,
    /// A path relative to the source directory copied to the output after
    /// the command; without it the command writes `GAIA_ARTIFACT_OUTPUT`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collect: Option<String>,
    /// Paths relative to `collect` left out of the copy.
    pub collect_exclude: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ArtifactVariantSpec {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactInstallIdentitySpec {
    pub install_name: String,
    pub install_class: ArtifactInstallClassSpec,
    pub destination_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ArtifactProviderKind {
    Rust,
    Java,
    Node,
    Python,
    Go,
    Command,
}

impl ArtifactProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Java => "java",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Command => "command",
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
            "command" => Ok(Self::Command),
            other => Err(format!("unknown artifact provider kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
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

    /// Fingerprints encode `extra_sources` only when set.
    #[test]
    fn artifact_spec_serializes_extra_sources_only_when_set() {
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
        assert!(!crate::canonical_json(&spec).contains("extra_sources"));
        spec.extra_sources = vec![SourceId::new("corelib")];
        assert!(crate::canonical_json(&spec).contains(r#""extra_sources":["corelib"]"#));
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
            build_env: Vec::new(),
            cpu: None,
            link: RustLinkSpec::Static,
            surface: None,
            trim: None,
        }
    }

    /// Fingerprints encode the build group fields only when set.
    #[test]
    fn rust_spec_serializes_group_fields_only_when_set() {
        let mut spec = rust_spec();
        let plain = crate::canonical_json(&spec);
        assert!(!plain.contains("build_group") && !plain.contains("group_packages"));
        spec.build_group = Some("engine".into());
        spec.group_packages = vec!["engine".into(), "plugin".into()];
        let grouped = crate::canonical_json(&spec);
        assert!(
            grouped.contains(r#""build_group":"engine""#)
                && grouped.contains(r#""group_packages":["engine","plugin"]"#),
            "{grouped}"
        );
        assert_eq!(spec.cargo_packages(), spec.group_packages);
        assert_eq!(rust_spec().cargo_packages(), vec!["engine".to_string()]);
    }

    #[test]
    fn cargo_git_url_key_ignores_prefix_query_slash_and_dot_git() {
        let key = "https://example.com/org/Corelib";
        for url in [
            "https://example.com/org/Corelib",
            "https://example.com/org/Corelib.git",
            "https://example.com/org/Corelib/",
            "git+https://example.com/org/Corelib.git?branch=main#0123",
            "git+https://example.com/org/Corelib?rev=abc",
        ] {
            assert_eq!(cargo_git_url_key(url), key, "{url}");
        }
        assert_ne!(
            cargo_git_url_key("https://example.com/org/Corelib-core.git"),
            key
        );
    }

    #[test]
    fn rust_spec_serializes_cargo_git_sources_only_when_set() {
        let mut spec = rust_spec();
        assert!(!crate::canonical_json(&spec).contains("cargo_git_sources"));
        spec.cargo_git_sources = vec![CargoGitSourceSpec {
            url: "https://example.com/dep.git".into(),
            source: SourceId::new("dep"),
            repo: "https://example.com/dep.git".into(),
        }];
        assert!(
            crate::canonical_json(&spec)
                .contains(r#""cargo_git_sources":[{"repo":"https://example.com/dep.git""#)
        );
    }
}
