use crate::operations::OperationId;
use gaia_spec::{AssemblyOciLayerSource, ImageAssemblySpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};

/// The artifacts whose outputs an image assembly reads directly: a file's
/// `src`, `src_dir` or `src_glob`, a transform's `src`, or an OCI `tar`
/// layer inside an artifact's `output_path`. The assembly waits for them,
/// so an app layer built from artifact outputs skips the image feed: an
/// app-only change rebuilds the artifact and reruns the assembly, while the
/// image build (which waits only for installs) is reused.
pub(crate) fn assembly_artifact_dependencies(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
) -> Vec<OperationId> {
    let Ok(roots) = gaia_spec::AssemblyRoots::new(spec, assembly) else {
        return Vec::new();
    };
    let resolve = |raw: &str| roots.resolve_path(spec, raw).ok();
    let mut read = Vec::<PathBuf>::new();
    for file in &assembly.files {
        read.extend(file.src.as_deref().and_then(resolve));
        read.extend(file.src_dir.as_deref().and_then(resolve));
        if let Some(glob) = &file.src_glob {
            // The literal directory in front of the first wildcard.
            let literal = glob.split(['*', '?', '[']).next().unwrap_or_default();
            let dir = literal.rsplit_once('/').map_or("", |(dir, _)| dir);
            if !dir.is_empty() {
                read.extend(resolve(dir));
            }
        }
    }
    for transform in &assembly.transforms {
        read.extend(transform.src.as_deref().and_then(resolve));
    }
    for oci in &assembly.ocis {
        for layer in &oci.layers {
            if let Some(AssemblyOciLayerSource::Tar(tar)) = layer.source() {
                read.extend(resolve(tar.as_str()));
            }
        }
    }
    spec.artifacts
        .iter()
        .filter(|artifact| {
            let output = crate::reuse::artifact_output_path(spec, &artifact.output.path);
            read.iter().any(|path| overlaps(path, &output))
        })
        .map(|artifact| OperationId::artifact(&artifact.id))
        .collect()
}

/// One path is the other or lies inside it.
fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}
