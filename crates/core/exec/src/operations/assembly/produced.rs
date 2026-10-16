//! Recording the assembly outputs a step wrote, with their sizes as the step
//! wrote them, for the run summary (see [`ProducedOutput`]).
//!
//! Each output is named by the spec's own path template and resolved against
//! the spec's own roots, not the run's: a RAM run's steps see its outputs moved
//! into RAM, and the record must still be the path the spec publishes to.

use super::verity::VerityOutput;
use super::*;

/// The assembly outputs a run's steps wrote, with their sizes as written.
pub(super) struct ProducedRecorder<'a> {
    spec: &'a ResolvedBuildSpec,
    /// The spec's own assembly and roots, which outputs are recorded under.
    assembly: &'a ImageAssemblySpec,
    roots: &'a AssemblyRoots,
    pub(super) outputs: Vec<ProducedOutput>,
}

impl<'a> ProducedRecorder<'a> {
    pub(super) fn new(
        spec: &'a ResolvedBuildSpec,
        assembly: &'a ImageAssemblySpec,
        roots: &'a AssemblyRoots,
    ) -> Self {
        Self {
            spec,
            assembly,
            roots,
            outputs: Vec::new(),
        }
    }
}

impl ProducedRecorder<'_> {
    /// Records the transform at `index` as it was written, `bytes` long.
    pub(super) fn record_transform(&mut self, index: usize, bytes: u64) {
        let assembly = self.assembly;
        self.record_produced(
            ProducedOutputKind::Transform,
            &assembly.transforms[index].dest,
            bytes,
        );
    }

    /// Records the verity transform at `index`: its data image, and the hash
    /// tree when that is a file of its own.
    pub(super) fn record_verity(&mut self, index: usize, output: &VerityOutput) {
        let assembly = self.assembly;
        let transform = &assembly.transforms[index];
        self.record_produced(ProducedOutputKind::Transform, &transform.dest, output.bytes);
        if let Some(hash_dest) = &transform.hash_dest
            && let Ok(bytes) = file_len(&output.hash)
        {
            self.record_produced(ProducedOutputKind::Transform, hash_dest, bytes);
        }
    }

    /// Records the filesystem at `index`, and its published copy when it has
    /// one. Both are `bytes` long.
    pub(super) fn record_filesystem(&mut self, index: usize, bytes: u64) {
        let assembly = self.assembly;
        let filesystem = &assembly.filesystems[index];
        self.record_produced(ProducedOutputKind::Filesystem, &filesystem.output, bytes);
        if let Some(template) = filesystem.publish_template() {
            self.record_produced(ProducedOutputKind::Filesystem, &template, bytes);
        }
    }

    /// Records the disk at `index`, `bytes` long.
    pub(super) fn record_disk(&mut self, index: usize, bytes: u64) {
        let assembly = self.assembly;
        self.record_produced(
            ProducedOutputKind::Disk,
            &assembly.disks[index].output,
            bytes,
        );
    }

    /// Records the archive at `index`, as written to `output`.
    pub(super) fn record_archive(&mut self, index: usize, output: &Path) {
        let Ok(bytes) = file_len(output) else {
            return;
        };
        let assembly = self.assembly;
        self.record_produced(
            ProducedOutputKind::Archive,
            &assembly.archives[index].output,
            bytes,
        );
    }

    /// Records an output the step wrote, `bytes` long, at the path `template`
    /// names in the spec. A template that does not resolve is not recorded;
    /// its size is then measured from the file, as before.
    fn record_produced(&mut self, kind: ProducedOutputKind, template: &str, bytes: u64) {
        let Ok(path) = self.roots.resolve_path(self.spec, template) else {
            return;
        };
        let name = produced_output_name(&path, self.roots);
        self.outputs.push(ProducedOutput {
            kind,
            name,
            path,
            bytes,
        });
    }
}

/// The name a produced output is recorded under: its path relative to the
/// out dir or work dir that holds it, as the run summary names outputs.
fn produced_output_name(path: &Path, roots: &AssemblyRoots) -> String {
    [&roots.assembly_out, &roots.assembly_work]
        .into_iter()
        .find_map(|dir| {
            path.strip_prefix(dir)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| relative.display().to_string())
        })
        .unwrap_or_else(|| {
            path.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        })
}
