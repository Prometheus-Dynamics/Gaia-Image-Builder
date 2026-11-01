//! Ordering the assembly steps by the outputs they read and write, and
//! clearing the outputs of an earlier run that the steps are about to make.

use super::*;
use gaia_spec::{AssemblyStep, ImageAssemblySpec};

/// The steps in the order they must run, with every step's resolved paths.
pub(super) fn ordered_assembly_steps(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    roots: &AssemblyRoots,
) -> Result<(Vec<AssemblyStep>, Vec<gaia_spec::AssemblyStepPaths>), AssemblyError> {
    let paths = gaia_spec::assembly_step_paths(
        assembly,
        &|template| roots.resolve_path(spec, template.as_str()).ok(),
        &|tree| roots.tree_path(tree).ok().map(Path::to_path_buf),
    );
    let order = gaia_spec::order_assembly_steps(&paths)
        .map_err(|cycle| AssemblyError::runtime(cycle.describe(assembly)))?;
    Ok((order, paths))
}

/// Removes what the steps that create their outputs from scratch produced
/// in an earlier run, so a step reading one of those paths before it is
/// produced again fails instead of using a stale file. A step that writes
/// the file it reads (a `copy` from `${provider.images}` to
/// `${assembly.out}`, which is the same dir by default) is an error before
/// anything is removed: removing its output would delete its input.
pub(super) fn remove_stale_step_outputs(
    assembly: &gaia_spec::ImageAssemblySpec,
    paths: &[gaia_spec::AssemblyStepPaths],
) -> Result<(), AssemblyError> {
    let replacing = || paths.iter().filter(|step| step.step.replaces_outputs());
    for step in replacing() {
        if let Some(path) = step.writes.iter().find(|write| step.reads.contains(write)) {
            return Err(AssemblyError::runtime(format!(
                "assembly {} reads and writes the same file '{}'; write it to another path (an expected image is already in the image output dir)",
                step.step.describe(assembly),
                path.display()
            )));
        }
    }
    for step in replacing() {
        for output in &step.writes {
            match std_fs::symlink_metadata(output) {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                    std_fs::remove_file(output).map_err(|error| {
                        format!(
                            "failed to remove stale assembly output '{}': {error}",
                            output.display()
                        )
                    })?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
