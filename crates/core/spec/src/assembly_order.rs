//! The order assembly steps run in: a step that reads a path another step
//! writes (the same path, a file inside a directory it fills, or a
//! directory it writes into) runs after it, whatever their kinds or
//! declaration order. Steps that do not depend on each other keep the
//! classic order: dirs, symlinks, files, busybox initramfs, kernel modules,
//! transforms, filesystems, disks, archives, each in declaration order.

use std::path::{Path, PathBuf};

use crate::{AssemblyPathTemplate, ImageAssemblySpec, assembly_digest_tokens};

/// One assembly step, by kind and index within its kind's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssemblyStep {
    Dir(usize),
    Symlink(usize),
    File(usize),
    BusyboxInitramfs(usize),
    KernelModules(usize),
    Transform(usize),
    Filesystem(usize),
    Disk(usize),
    Archive(usize),
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
        }
    }

    /// Whether the step creates its outputs from scratch (as opposed to
    /// filling a tree), so old outputs can be removed before a run.
    pub fn replaces_outputs(self) -> bool {
        matches!(
            self,
            Self::Transform(_) | Self::Filesystem(_) | Self::Disk(_) | Self::Archive(_)
        )
    }
}

/// What a step reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        let read = match (&file.src, &file.src_glob) {
            (Some(src), _) => resolve(src),
            // A glob reads what its pattern matches.
            (None, Some(glob)) => resolve(glob),
            (None, None) => None,
        };
        let dest = file.dest.trim_end_matches('/');
        let dest = if dest.is_empty() { "." } else { dest };
        push(
            AssemblyStep::File(index),
            vec![read],
            vec![in_tree(file.tree.as_str(), dest)],
        );
    }
    for (index, initramfs) in assembly.busybox_initramfs.iter().enumerate() {
        push(
            AssemblyStep::BusyboxInitramfs(index),
            vec![resolve(&initramfs.busybox)],
            vec![tree(initramfs.tree.as_str())],
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
        push(
            AssemblyStep::Transform(index),
            vec![transform.src.as_ref().and_then(resolve)],
            vec![resolve(&transform.dest)],
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
            for (_, value) in member.entries.iter().flatten() {
                let _ = assembly_digest_tokens(value, |token| {
                    reads.push(resolve(&token.path));
                    Ok(String::new())
                });
            }
        }
        push(
            AssemblyStep::Archive(index),
            reads,
            vec![resolve(&archive.output)],
        );
    }
    steps
}

/// Whether a read (a path, or a pattern with `*` components) and a write
/// touch the same files: one is inside the other, or the write is a path
/// the pattern matches, is inside one, or holds the directory it scans.
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
mod tests {
    use super::*;

    fn step(step: AssemblyStep, reads: &[&str], writes: &[&str]) -> AssemblyStepPaths {
        AssemblyStepPaths {
            step,
            reads: reads.iter().map(PathBuf::from).collect(),
            writes: writes.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn readers_run_after_writers_whatever_their_kind() {
        let steps = [
            step(
                AssemblyStep::File(0),
                &["/src/cmdline.txt"],
                &["/work/boot/cmdline.txt"],
            ),
            step(
                AssemblyStep::Transform(0),
                &["/images/boot.vfat"],
                &["/work/boot.vfat.zst"],
            ),
            step(
                AssemblyStep::Filesystem(0),
                &["/work/boot"],
                &["/images/boot.vfat"],
            ),
            step(
                AssemblyStep::Disk(0),
                &["/images/boot.vfat"],
                &["/out/disk.img"],
            ),
            step(
                AssemblyStep::Archive(0),
                &["/work/boot.vfat.zst"],
                &["/out/update.tar"],
            ),
        ];
        assert_eq!(
            order_assembly_steps(&steps),
            Ok(vec![
                AssemblyStep::File(0),
                AssemblyStep::Filesystem(0),
                AssemblyStep::Transform(0),
                AssemblyStep::Disk(0),
                AssemblyStep::Archive(0),
            ])
        );
        assert_eq!(
            assembly_steps_reading_later_outputs(&steps),
            [(AssemblyStep::Transform(0), AssemblyStep::Filesystem(0))]
        );
    }

    #[test]
    fn independent_steps_keep_the_classic_order() {
        let steps = [
            step(
                AssemblyStep::Disk(0),
                &["/images/rootfs.ext4"],
                &["/out/disk.img"],
            ),
            step(AssemblyStep::Transform(1), &["/a"], &["/b"]),
            step(AssemblyStep::Transform(0), &["/c"], &["/d"]),
        ];
        assert_eq!(
            order_assembly_steps(&steps),
            Ok(vec![
                AssemblyStep::Transform(0),
                AssemblyStep::Transform(1),
                AssemblyStep::Disk(0),
            ])
        );
        assert!(assembly_steps_reading_later_outputs(&steps).is_empty());
    }

    #[test]
    fn cycles_are_refused() {
        let steps = [
            step(AssemblyStep::Transform(0), &["/b"], &["/a"]),
            step(AssemblyStep::Transform(1), &["/a"], &["/b"]),
            step(AssemblyStep::Dir(0), &[], &["/tree/x"]),
        ];
        assert_eq!(
            order_assembly_steps(&steps),
            Err(AssemblyCycle {
                steps: vec![AssemblyStep::Transform(0), AssemblyStep::Transform(1)]
            })
        );
    }

    #[test]
    fn globs_read_only_what_they_match() {
        let dtbs = Path::new("$provider.images/*.dtb");
        assert!(overlaps(dtbs, Path::new("$provider.images/cm5.dtb")));
        assert!(!overlaps(dtbs, Path::new("$provider.images/boot.vfat")));
        // The writer fills the directory the glob scans.
        assert!(overlaps(dtbs, Path::new("$provider.images")));
        assert!(overlaps(
            Path::new("$assembly.work/overlays/*/x"),
            Path::new("$assembly.work/overlays/a/x/y")
        ));
        // Plain paths: either inside the other.
        assert!(overlaps(
            Path::new("/work/boot"),
            Path::new("/work/boot/cmdline.txt")
        ));
        assert!(overlaps(
            Path::new("/work/boot/cmdline.txt"),
            Path::new("/work/boot")
        ));
        assert!(!overlaps(
            Path::new("/work/boot"),
            Path::new("/work/bootfs")
        ));
    }

    #[test]
    fn a_boot_tree_filled_from_images_is_not_a_cycle() {
        // Boot files come from the provider's images; the boot filesystem
        // is written there too, under a name the glob does not match.
        let steps = [
            step(AssemblyStep::File(0), &["/images/*.dtb"], &["/work/boot"]),
            step(
                AssemblyStep::Filesystem(0),
                &["/work/boot"],
                &["/images/boot.vfat"],
            ),
        ];
        assert_eq!(
            order_assembly_steps(&steps),
            Ok(vec![AssemblyStep::File(0), AssemblyStep::Filesystem(0)])
        );
    }

    #[test]
    fn kernel_modules_fill_the_tree_before_it_is_packed_and_published_copies_are_writes() {
        use crate::{
            AssemblyFileSpec, AssemblyFilesystemKindSpec, AssemblyFilesystemSpec,
            AssemblyKernelModulesSpec, AssemblyTreeSpec,
        };
        let assembly = ImageAssemblySpec {
            trees: vec![
                AssemblyTreeSpec {
                    id: "initramfs".into(),
                    path: "/work/initramfs".into(),
                },
                AssemblyTreeSpec {
                    id: "boot".into(),
                    path: "/work/boot".into(),
                },
            ],
            files: vec![AssemblyFileSpec {
                tree: "boot".into(),
                src: Some("$assembly.out/boot.img".into()),
                src_glob: None,
                dest: "boot.img".into(),
                mode: None,
                optional: false,
                preserve_symlink: false,
            }],
            kernel_modules: vec![AssemblyKernelModulesSpec {
                tree: "initramfs".into(),
                from: "/provider/target/lib/modules".into(),
                kernel_version: None,
                modules: vec!["libcomposite".into()],
                depmod: None,
            }],
            filesystems: vec![AssemblyFilesystemSpec {
                id: "boot".into(),
                kind: AssemblyFilesystemKindSpec::CpioZstd,
                source_tree: "initramfs".into(),
                output: "/work/boot.img".into(),
                size: None,
                deterministic: true,
                compression_level: None,
                publish: true,
            }],
            ..ImageAssemblySpec::default()
        };
        let paths = assembly_step_paths(
            &assembly,
            &|template| Some(PathBuf::from(template.as_str())),
            &|id| match id {
                "initramfs" => Some(PathBuf::from("/work/initramfs")),
                "boot" => Some(PathBuf::from("/work/boot")),
                _ => None,
            },
        );
        // The filesystem reads the tree the modules fill, and the file in
        // another tree reads the published copy the filesystem writes.
        assert_eq!(
            order_assembly_steps(&paths),
            Ok(vec![
                AssemblyStep::KernelModules(0),
                AssemblyStep::Filesystem(0),
                AssemblyStep::File(0),
            ])
        );
        assert!(
            AssemblyStep::KernelModules(0)
                .describe(&assembly)
                .contains("initramfs")
        );
    }

    #[test]
    fn published_filesystem_images_go_to_the_output_dir_under_their_file_name() {
        use crate::{AssemblyFilesystemKindSpec, AssemblyFilesystemSpec};
        let mut filesystem = AssemblyFilesystemSpec {
            id: "boot".into(),
            kind: AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: "$assembly.work/sub/boot.img".into(),
            size: None,
            deterministic: false,
            compression_level: None,
            publish: false,
        };
        assert_eq!(filesystem.publish_template(), None);
        filesystem.publish = true;
        assert_eq!(
            filesystem.publish_template().expect("published").as_str(),
            "$assembly.out/boot.img"
        );
    }
}
