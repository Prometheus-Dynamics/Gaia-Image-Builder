//! `[[toolchains]]`: a cross toolchain an external OS SDK provides (the
//! Buildroot SDK of a device OS), and the paths Gaia builds against.
//!
//! Gaia does not install anything: the SDK is a source (a downloaded and
//! verified archive, or any other source) or a local directory, and the
//! compiler, the sysroot and the Rust target are read from it.

use crate::source_dirs::source_materialized_dir;
use crate::{ResolvedBuildSpec, SourceId, resolve_workspace_path};
use std::path::{Path, PathBuf};

/// A declared `[[toolchains]]` entry, as the build compiles it. Which of the
/// either/or fields are set is checked by the validator; a field left empty
/// here is reported there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolchainSpec {
    pub id: String,
    pub kind: ToolchainKindSpec,
    /// The SDK root is the materialized tree of this source ...
    pub root_source: Option<SourceId>,
    /// ... or this directory (a workspace path; absolute paths are allowed).
    pub root_path: Option<String>,
    /// Run `<root>/relocate-sdk.sh` once per extracted location, when the
    /// SDK has that script (see the docs).
    pub relocate: bool,
    /// The compiler prefix in `<root>/bin`, such as `aarch64-linux-`. `None`
    /// takes the triple followed by `-`.
    pub prefix: Option<String>,
    /// The GNU triple, such as `aarch64-buildroot-linux-gnu`.
    pub triple: String,
    /// The sysroot is this path under the root (or an absolute path) ...
    pub sysroot_path: Option<String>,
    /// ... or the materialized tree of this source.
    pub sysroot_source: Option<SourceId>,
    /// The Rust target the SDK's compiler serves, such as
    /// `aarch64-unknown-linux-gnu`. Rust artifacts build for it unless they
    /// name a target of their own.
    pub rust_target: Option<String>,
    /// The device's page size in bytes, a power of two of at least 4096. Set,
    /// Rust artifacts link with `max-page-size` of it, and a package built
    /// with this toolchain checks its segments against it. Not serialized when
    /// unset, so toolchains without one keep their identities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ToolchainKindSpec {
    /// A Buildroot SDK: `<root>/bin/<prefix>gcc`, `<root>/<triple>/sysroot`.
    BuildrootSdk,
}

impl ToolchainKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuildrootSdk => "buildroot-sdk",
        }
    }
}

/// A toolchain with its paths resolved on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToolchain {
    pub id: String,
    pub root: PathBuf,
    pub sysroot: PathBuf,
    pub relocate: bool,
    pub prefix: String,
    pub triple: String,
    pub rust_target: Option<String>,
    pub page_size: Option<u64>,
}

impl ResolvedToolchain {
    /// The compiler or tool `name` in the SDK's `bin` dir, with the prefix:
    /// `tool("gcc")` is `<root>/bin/<prefix>gcc`.
    pub fn tool(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(format!("{}{name}", self.prefix))
    }
}

/// The default compiler prefix of a triple: the triple and a `-`.
pub fn default_toolchain_prefix(triple: &str) -> String {
    format!("{triple}-")
}

/// The SDK root a toolchain names, resolved against the workspace.
pub fn toolchain_root(
    spec: &ResolvedBuildSpec,
    toolchain: &ToolchainSpec,
) -> Result<PathBuf, String> {
    if let Some(source) = &toolchain.root_source {
        return source_dir(spec, source, &toolchain.id);
    }
    match &toolchain.root_path {
        Some(path) => resolve_workspace_path(&spec.workspace, path)
            .map_err(|error| format!("toolchain '{}': {error}", toolchain.id)),
        None => Err(format!("toolchain '{}' has no root", toolchain.id)),
    }
}

/// Resolves the toolchain `id` of `spec` to its paths on this machine. The
/// paths are not checked to exist: the build reports what is missing.
pub fn resolve_toolchain(spec: &ResolvedBuildSpec, id: &str) -> Result<ResolvedToolchain, String> {
    let toolchain = spec
        .toolchains
        .iter()
        .find(|toolchain| toolchain.id == id)
        .ok_or_else(|| format!("unknown toolchain '{id}'"))?;
    let root = toolchain_root(spec, toolchain)?;
    let sysroot = match (&toolchain.sysroot_source, &toolchain.sysroot_path) {
        (Some(source), _) => source_dir(spec, source, &toolchain.id)?,
        (None, Some(path)) if Path::new(path).is_absolute() => PathBuf::from(path),
        (None, Some(path)) => root.join(path),
        (None, None) => return Err(format!("toolchain '{id}' has no sysroot")),
    };
    let prefix = toolchain
        .prefix
        .clone()
        .unwrap_or_else(|| default_toolchain_prefix(&toolchain.triple));
    Ok(ResolvedToolchain {
        id: toolchain.id.clone(),
        root,
        sysroot,
        relocate: toolchain.relocate,
        prefix,
        triple: toolchain.triple.clone(),
        rust_target: toolchain.rust_target.clone(),
        page_size: toolchain.page_size,
    })
}

/// Where a source's tree is materialized, for a toolchain that names it.
fn source_dir(spec: &ResolvedBuildSpec, id: &SourceId, toolchain: &str) -> Result<PathBuf, String> {
    let source = spec
        .sources
        .iter()
        .find(|source| source.id == *id)
        .ok_or_else(|| format!("toolchain '{toolchain}' names undefined source '{id}'"))?;
    Ok(source_materialized_dir(&spec.workspace, source))
}
