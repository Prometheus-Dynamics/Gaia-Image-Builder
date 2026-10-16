//! The image outputs the image assembly spec names, resolved to paths. The
//! spec knows each output's kind (an archive, a disk, a filesystem, a
//! transform), so the size report takes the kind from here rather than from
//! the file name. Resolving the spec's own path templates means the list also
//! covers outputs a reused assembly operation left on disk.

use gaia_spec::{AssemblyPathTemplate, AssemblyRoots, ResolvedBuildSpec};
use std::path::PathBuf;

/// An output the spec names, with the kind the spec gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownOutput {
    pub(crate) path: PathBuf,
    pub(crate) kind: &'static str,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct AssemblyOutputs {
    /// Archives, disks, filesystems and transform outputs, in that order.
    pub(crate) known: Vec<KnownOutput>,
    /// The assembly's output and work dirs, which output names are relative to.
    pub(crate) dirs: Vec<PathBuf>,
}

/// The spec's assembly outputs, resolved against its assembly roots. Empty
/// when the build has no assembly or its roots cannot be resolved.
pub(crate) fn assembly_outputs(spec: &ResolvedBuildSpec) -> AssemblyOutputs {
    let Some(assembly) = spec.image.assembly.as_ref() else {
        return AssemblyOutputs::default();
    };
    let Ok(roots) = AssemblyRoots::new(spec, assembly) else {
        return AssemblyOutputs::default();
    };
    let resolve =
        |template: &AssemblyPathTemplate| roots.resolve_path(spec, template.as_str()).ok();
    let mut known = Vec::new();
    for archive in &assembly.archives {
        push(&mut known, resolve(&archive.output), "archive");
    }
    for disk in &assembly.disks {
        push(&mut known, resolve(&disk.output), "disk");
    }
    for filesystem in &assembly.filesystems {
        push(&mut known, resolve(&filesystem.output), "filesystem");
    }
    for transform in &assembly.transforms {
        push(&mut known, resolve(&transform.dest), "transform");
        if let Some(hash_dest) = &transform.hash_dest {
            push(&mut known, resolve(hash_dest), "transform");
        }
    }
    AssemblyOutputs {
        known,
        dirs: vec![roots.assembly_out.clone(), roots.assembly_work.clone()],
    }
}

fn push(known: &mut Vec<KnownOutput>, path: Option<PathBuf>, kind: &'static str) {
    if let Some(path) = path {
        known.push(KnownOutput { path, kind });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_skips_unresolved_outputs() {
        let mut known = Vec::new();
        push(&mut known, None, "disk");
        push(&mut known, Some(PathBuf::from("/out/a.img")), "disk");
        assert_eq!(
            known,
            vec![KnownOutput {
                path: PathBuf::from("/out/a.img"),
                kind: "disk",
            }]
        );
    }
}
