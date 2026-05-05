use super::*;

pub(crate) fn assembly_state_path(spec: &ResolvedBuildSpec) -> PathBuf {
    runtime_state_dir(spec).join(gaia_spec::IMAGE_ASSEMBLY_STATE_FILE_NAME)
}

pub(crate) fn image_assembly_cleanup_paths(spec: &ResolvedBuildSpec) -> Vec<PathBuf> {
    let mut paths = vec![assembly_state_path(spec)];
    let Some(assembly) = &spec.image.assembly else {
        return paths;
    };
    if let Ok(roots) = AssemblyRoots::new(spec, assembly) {
        paths.extend(image_assembly_output_cleanup_paths(spec, assembly, &roots));
    }
    dedupe_paths(paths)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AssemblyExecutionContext {
    cleanup_paths: Vec<PathBuf>,
}

impl AssemblyExecutionContext {
    pub(super) fn new(
        spec: &ResolvedBuildSpec,
        assembly: &gaia_spec::ImageAssemblySpec,
        roots: &AssemblyRoots,
    ) -> Self {
        Self {
            cleanup_paths: image_assembly_output_cleanup_paths(spec, assembly, roots),
        }
    }

    pub(super) fn cleanup_paths(self) -> Vec<PathBuf> {
        dedupe_paths(self.cleanup_paths)
    }
}

fn image_assembly_output_cleanup_paths(
    spec: &ResolvedBuildSpec,
    assembly: &gaia_spec::ImageAssemblySpec,
    roots: &AssemblyRoots,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for tree in &assembly.trees {
        if let Ok(path) = roots.tree_path(&tree.id) {
            paths.push(path.to_path_buf());
        }
    }
    for transform in &assembly.transforms {
        if let Ok(path) = roots.resolve_path(spec, &transform.dest) {
            paths.push(temporary_assembly_output_path(&path));
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
    }
    for filesystem in &assembly.filesystems {
        if let Ok(path) = roots.resolve_path(spec, &filesystem.output) {
            let temp = temporary_assembly_output_path(&path);
            if filesystem.kind == gaia_spec::AssemblyFilesystemKindSpec::CpioGzip {
                paths.push(temp.with_extension("cpio.tmp"));
            }
            paths.push(temp);
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
    }
    for disk in &assembly.disks {
        if let Ok(path) = roots.resolve_path(spec, &disk.output) {
            paths.push(temporary_assembly_output_path(&path));
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
    }
    dedupe_paths(paths)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}
