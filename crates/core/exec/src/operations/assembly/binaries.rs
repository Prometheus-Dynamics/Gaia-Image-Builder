//! `[[image.assembly.binaries]]`: a binary copied into a tree with the
//! runtime closure it needs (its interpreter and libraries, from the sysroot
//! it was built for). A static binary is only copied.

use super::runtime_closure::{RuntimeLinkage, copy_runtime_closure};
use super::*;

pub(super) struct BinarySummary {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) runtime_linkage: RuntimeLinkage,
    pub(super) runtime_libraries: Vec<PathBuf>,
}

/// The mode a listed binary gets when the config does not set one.
const DEFAULT_BINARY_MODE: &str = "0755";

pub(super) fn execute_binary(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    binary: &gaia_spec::AssemblyBinarySpec,
) -> Result<BinarySummary, AssemblyError> {
    let tree = roots.tree_path(&binary.tree)?;
    let src = roots.resolve_path(spec, &binary.src)?;
    if !src.is_file() {
        return Err(format!(
            "assembly binary '{}' does not exist or is not a file",
            src.display()
        )
        .into());
    }
    let dest = assembly_tree_dest(tree, &binary.dest)?;
    if dest == tree {
        return Err(format!(
            "assembly binary dest '{}' names the tree itself",
            binary.dest
        )
        .into());
    }
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create binary dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    std_fs::copy(&src, &dest).map_err(|error| {
        format!(
            "failed to copy binary '{}' to '{}': {error}",
            src.display(),
            dest.display()
        )
    })?;
    let mode = match binary.parsed_mode().map_err(|error| error.to_string())? {
        Some(mode) => mode,
        None => DEFAULT_BINARY_MODE
            .parse()
            .map_err(|error: gaia_spec::FileModeParseError| error.to_string())?,
    };
    apply_mode(&dest, Some(mode))?;

    let sysroot = binary
        .sysroot
        .as_ref()
        .map(|template| roots.resolve_path(spec, template))
        .transpose()?;
    let closure = gaia_plan::resolve_runtime_closure(&src, sysroot.as_deref())?;
    let (runtime_linkage, runtime_libraries) = if closure.dynamic {
        let copied = copy_runtime_closure(tree, &closure)?;
        (RuntimeLinkage::Dynamic, copied)
    } else {
        (RuntimeLinkage::Static, Vec::new())
    };
    Ok(BinarySummary {
        bytes: file_len(&dest)?,
        sha256: file_sha256(&dest)?,
        src,
        dest,
        runtime_linkage,
        runtime_libraries,
    })
}
