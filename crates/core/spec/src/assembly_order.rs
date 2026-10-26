//! The order assembly steps run in: a step that reads a path another step
//! writes (the same path, a file inside a directory it fills, or a
//! directory it writes into) runs after it, whatever their kinds or
//! declaration order. Steps that do not depend on each other keep the
//! classic order: dirs, symlinks, files, busybox initramfs, binaries, kernel
//! modules, transforms, filesystems, disks, archives, each in declaration
//! order.

use std::path::{Path, PathBuf};

use crate::{
    AssemblyPathTemplate, AssemblyValueToken, ImageAssemblySpec, assembly_json_string_tokens,
    assembly_value_tokens,
};

/// One assembly step, by kind and index within its kind's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum AssemblyStep {
    Dir(usize),
    Symlink(usize),
    File(usize),
    BusyboxInitramfs(usize),
    Binary(usize),
    KernelModules(usize),
    Transform(usize),
    Filesystem(usize),
    Disk(usize),
    Archive(usize),
    Oci(usize),
    Package(usize),
}

impl AssemblyStep {
    /// How the step is named in messages.
    pub fn describe(self, assembly: &ImageAssemblySpec) -> String {
        match self {
            Self::Dir(index) => format!("dir '{}'", assembly.dirs[index].path),
            Self::Symlink(index) => format!("symlink '{}'", assembly.symlinks[index].path),
            Self::File(index) => format!(
                "file entry {} ('{}')",
                index + 1,
                assembly.files[index].dest
            ),
            Self::BusyboxInitramfs(index) => {
                format!(
                    "busybox initramfs for tree '{}'",
                    assembly.busybox_initramfs[index].tree
                )
            }
            Self::Binary(index) => format!(
                "binary '{}' for tree '{}'",
                assembly.binaries[index].dest, assembly.binaries[index].tree
            ),
            Self::KernelModules(index) => format!(
                "kernel modules for tree '{}'",
                assembly.kernel_modules[index].tree
            ),
            Self::Transform(index) => {
                let transform = &assembly.transforms[index];
                format!(
                    "{} transform to '{}'",
                    transform.kind.as_str(),
                    transform.dest.as_str()
                )
            }
            Self::Filesystem(index) => format!("filesystem '{}'", assembly.filesystems[index].id),
            Self::Disk(index) => format!("disk '{}'", assembly.disks[index].id),
            Self::Archive(index) => format!("archive '{}'", assembly.archives[index].id),
            Self::Oci(index) => format!("oci image '{}'", assembly.ocis[index].id),
            Self::Package(index) => format!("package '{}'", assembly.packages[index].id),
        }
    }

    /// Whether the step creates its outputs from scratch (as opposed to
    /// filling a tree), so old outputs can be removed before a run.
    pub fn replaces_outputs(self) -> bool {
        matches!(
            self,
            Self::Transform(_)
                | Self::Filesystem(_)
                | Self::Disk(_)
                | Self::Archive(_)
                | Self::Oci(_)
                | Self::Package(_)
        )
    }
}

/// What a step reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyStepPaths {
    pub step: AssemblyStep,
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
}

/// Every step's paths. `resolve` maps a path template to a path (runtime
/// resolution, or the template text itself for validation) and `tree`
/// maps a tree id to its directory; either may give up with `None`.
pub fn assembly_step_paths(
    assembly: &ImageAssemblySpec,
    resolve: &dyn Fn(&AssemblyPathTemplate) -> Option<PathBuf>,
    tree: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<AssemblyStepPaths> {
    let mut steps = Vec::new();
    let mut push = |step, reads: Vec<Option<PathBuf>>, writes: Vec<Option<PathBuf>>| {
        steps.push(AssemblyStepPaths {
            step,
            reads: reads.into_iter().flatten().collect(),
            writes: writes.into_iter().flatten().collect(),
        });
    };
    let in_tree =
        |id: &str, path: &str| tree(id).map(|root| root.join(path.trim_start_matches('/')));
    for (index, dir) in assembly.dirs.iter().enumerate() {
        push(
            AssemblyStep::Dir(index),
            vec![],
            vec![in_tree(dir.tree.as_str(), &dir.path)],
        );
    }
    for (index, symlink) in assembly.symlinks.iter().enumerate() {
        push(
            AssemblyStep::Symlink(index),
            vec![],
            vec![in_tree(symlink.tree.as_str(), &symlink.path)],
        );
    }
    for (index, file) in assembly.files.iter().enumerate() {
        let mut reads = match (&file.src, &file.src_glob) {
            (Some(src), _) => vec![resolve(src)],
            // A glob reads what its pattern matches.
            (None, Some(glob)) => vec![resolve(glob)],
            // Generated content reads the paths its tokens name.
            (None, None) => {
                let mut token_reads = Vec::new();
                if let Some(content) = &file.content {
                    value_token_reads(content, resolve, &mut token_reads);
                }
                if let Some(json) = &file.json {
                    for text in json.strings() {
                        json_token_reads(text, resolve, tree, &mut token_reads);
                    }
                }
                token_reads
            }
        };
        let dest = file.dest.trim_end_matches('/');
        let dest = if dest.is_empty() { "." } else { dest };
        reads.retain(Option::is_some);
        push(
            AssemblyStep::File(index),
            reads,
            vec![in_tree(file.tree.as_str(), dest)],
        );
    }
    for (index, initramfs) in assembly.busybox_initramfs.iter().enumerate() {
        let mut reads = vec![resolve(&initramfs.busybox)];
        if let Some(sysroot) = &initramfs.sysroot {
            reads.push(resolve(sysroot));
        }
        push(
            AssemblyStep::BusyboxInitramfs(index),
            reads,
            vec![tree(initramfs.tree.as_str())],
        );
    }
    for (index, binary) in assembly.binaries.iter().enumerate() {
        let mut reads = vec![resolve(&binary.src)];
        if let Some(sysroot) = &binary.sysroot {
            reads.push(resolve(sysroot));
        }
        push(
            AssemblyStep::Binary(index),
            reads,
            vec![tree(binary.tree.as_str())],
        );
    }
    for (index, modules) in assembly.kernel_modules.iter().enumerate() {
        push(
            AssemblyStep::KernelModules(index),
            vec![resolve(&modules.from)],
            vec![tree(modules.tree.as_str())],
        );
    }
    for (index, transform) in assembly.transforms.iter().enumerate() {
        let mut writes = vec![resolve(&transform.dest)];
        if let Some(hash_dest) = &transform.hash_dest {
            writes.push(resolve(hash_dest));
        }
        push(
            AssemblyStep::Transform(index),
            vec![transform.src.as_ref().and_then(resolve)],
            writes,
        );
    }
    for (index, filesystem) in assembly.filesystems.iter().enumerate() {
        let mut writes = vec![resolve(&filesystem.output)];
        if let Some(publish) = filesystem.publish_template() {
            writes.push(resolve(&publish));
        }
        push(
            AssemblyStep::Filesystem(index),
            vec![tree(filesystem.source_tree.as_str())],
            writes,
        );
    }
    for (index, disk) in assembly.disks.iter().enumerate() {
        push(
            AssemblyStep::Disk(index),
            disk.partitions
                .iter()
                .map(|partition| partition.image.as_ref().and_then(resolve))
                .collect(),
            vec![resolve(&disk.output)],
        );
    }
    for (index, archive) in assembly.archives.iter().enumerate() {
        let mut reads = Vec::new();
        for member in &archive.members {
            if let Some(src) = &member.src {
                reads.push(resolve(src));
            }
            // A tree or directory member reads its root, so it waits for
            // every step that fills it; a glob reads what it matches.
            if let Some(tree_id) = &member.tree {
                reads.push(tree(tree_id.as_str()));
            }
            if let Some(src_dir) = &member.src_dir {
                reads.push(resolve(src_dir));
            }
            if let Some(src_glob) = &member.src_glob {
                reads.push(resolve(src_glob));
            }
            for (_, value) in member.entries.iter().flatten() {
                value_token_reads(value, resolve, &mut reads);
            }
            if let Some(json) = &member.json {
                for text in json.strings() {
                    json_token_reads(text, resolve, tree, &mut reads);
                }
            }
        }
        push(
            AssemblyStep::Archive(index),
            reads,
            vec![resolve(&archive.output)],
        );
    }
    for (index, oci) in assembly.ocis.iter().enumerate() {
        // A layer tar is read as it is; a tree layer reads its root, so it
        // waits for every step that fills the tree.
        let reads = oci
            .layers
            .iter()
            .filter_map(|layer| match layer.source() {
                Some(crate::AssemblyOciLayerSource::Tar(tar)) => Some(resolve(tar)),
                Some(crate::AssemblyOciLayerSource::Tree(tree_id)) => Some(tree(tree_id.as_str())),
                None => None,
            })
            .collect();
        push(AssemblyStep::Oci(index), reads, vec![resolve(&oci.dest)]);
    }
    for (index, package) in assembly.packages.iter().enumerate() {
        // The package reads its tree as installed, so it waits for every
        // step that fills the tree.
        let reads = vec![tree(package.tree.as_str())];
        push(
            AssemblyStep::Package(index),
            reads,
            vec![resolve(&package.output)],
        );
    }
    steps
}

/// The paths a text value's `${assembly...}` tokens read, resolved.
fn value_token_reads(
    value: &str,
    resolve: &dyn Fn(&AssemblyPathTemplate) -> Option<PathBuf>,
    reads: &mut Vec<Option<PathBuf>>,
) {
    let _ = assembly_value_tokens(value, |token| {
        reads.push(resolve(token.path()));
        Ok(String::new())
    });
}

/// The paths a JSON string's tokens read. A tree digest reads the tree's
/// root when it names a tree id, so it waits for every step that fills it.
fn json_token_reads(
    value: &str,
    resolve: &dyn Fn(&AssemblyPathTemplate) -> Option<PathBuf>,
    tree: &dyn Fn(&str) -> Option<PathBuf>,
    reads: &mut Vec<Option<PathBuf>>,
) {
    let _ = assembly_json_string_tokens(value, |token| {
        let read = match token {
            AssemblyValueToken::TreeDigest(digest) => {
                tree(digest.path.as_str().trim()).or_else(|| resolve(&digest.path))
            }
            other => resolve(other.path()),
        };
        reads.push(read);
        Ok(String::new())
    });
}

/// Whether a read (a path, or a pattern with `*` components) and a write
/// touch the same files: one is inside the other, or the write is a path
/// the pattern matches, is inside one, or holds the directory it scans.
/// [`overlaps`], for the step scheduler: the same test as the ordering.
pub fn assembly_paths_overlap(read: &Path, write: &Path) -> bool {
    overlaps(read, write)
}

fn overlaps(read: &Path, write: &Path) -> bool {
    let read_components = read
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let write_components = write
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    read_components
        .iter()
        .zip(&write_components)
        .all(|(pattern, component)| crate::wildcard_match(pattern, component))
}

/// Whether `later` reads something `earlier` writes.
fn depends_on(later: &AssemblyStepPaths, earlier: &AssemblyStepPaths) -> bool {
    later.step != earlier.step
        && later
            .reads
            .iter()
            .any(|read| earlier.writes.iter().any(|write| overlaps(read, write)))
}

/// A dependency cycle between assembly steps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyCycle {
    pub steps: Vec<AssemblyStep>,
}

impl AssemblyCycle {
    pub fn describe(&self, assembly: &ImageAssemblySpec) -> String {
        format!(
            "assembly steps depend on each other's outputs in a cycle: {}",
            self.steps
                .iter()
                .map(|step| step.describe(assembly))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The steps in the order they run: each after every step it reads from,
/// otherwise in the classic order.
pub fn order_assembly_steps(
    steps: &[AssemblyStepPaths],
) -> Result<Vec<AssemblyStep>, AssemblyCycle> {
    let mut remaining = steps.iter().collect::<Vec<_>>();
    remaining.sort_by_key(|step| step.step);
    let mut ordered = Vec::with_capacity(steps.len());
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .position(|candidate| !remaining.iter().any(|other| depends_on(candidate, other)));
        match ready {
            Some(index) => ordered.push(remaining.remove(index).step),
            None => {
                return Err(AssemblyCycle {
                    steps: remaining.iter().map(|step| step.step).collect(),
                });
            }
        }
    }
    Ok(ordered)
}

/// Steps that read what a step declared after them writes, as
/// `(reader, writer)`: they now run after it, where earlier Gaia versions
/// ran them first, on the previous run's files.
pub fn assembly_steps_reading_later_outputs(
    steps: &[AssemblyStepPaths],
) -> Vec<(AssemblyStep, AssemblyStep)> {
    let mut found = Vec::new();
    for reader in steps {
        for writer in steps {
            if writer.step > reader.step && depends_on(reader, writer) {
                found.push((reader.step, writer.step));
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "assembly_order_tests.rs"]
mod tests;
