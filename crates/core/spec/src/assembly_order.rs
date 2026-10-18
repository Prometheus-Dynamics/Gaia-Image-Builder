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
        assert!(overlaps(dtbs, Path::new("$provider.images/evk.dtb")));
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
                    strip: false,
                    id: "initramfs".into(),
                    path: "/work/initramfs".into(),
                },
                AssemblyTreeSpec {
                    strip: false,
                    id: "boot".into(),
                    path: "/work/boot".into(),
                },
            ],
            files: vec![AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: Some("$assembly.out/boot.img".into()),
                src_glob: None,
                dest: "boot.img".into(),
                mode: None,
                optional: false,
                preserve_symlink: false,
                allow_missing_libs: false,
                content: None,
                json: None,
            }],
            kernel_modules: vec![AssemblyKernelModulesSpec {
                metadata: None,
                tree: "initramfs".into(),
                from: "/provider/target/lib/modules".into(),
                kernel_version: None,
                modules: vec!["libcomposite".into()],
                depmod: None,
            }],
            filesystems: vec![AssemblyFilesystemSpec {
                free: None,
                id: "boot".into(),
                kind: AssemblyFilesystemKindSpec::CpioZstd,
                source_tree: "initramfs".into(),
                output: "/work/boot.img".into(),
                size: None,
                deterministic: true,
                compression_level: None,
                publish: true,
                label: None,
                uuid: None,
                journal: None,
                compressed: Default::default(),
                fat_size: None,
                reserved_percent: None,
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
            free: None,
            id: "boot".into(),
            kind: AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: "$assembly.work/sub/boot.img".into(),
            size: None,
            deterministic: false,
            compression_level: None,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            compressed: Default::default(),
            fat_size: None,
            reserved_percent: None,
        };
        assert_eq!(filesystem.publish_template(), None);
        filesystem.publish = true;
        assert_eq!(
            filesystem.publish_template().expect("published").as_str(),
            "$assembly.out/boot.img"
        );
    }

    /// A JSON tree digest reads its tree: the file waits for every step that
    /// fills the tree, however it is declared.
    #[test]
    fn a_tree_digest_runs_after_the_steps_that_fill_its_tree() {
        use crate::{AssemblyFileSpec, AssemblyJsonValue, AssemblyTreeSpec};
        let file =
            |tree: &str, dest: &str, content: Option<&str>, json: Option<AssemblyJsonValue>| {
                AssemblyFileSpec {
                    include: Vec::new(),
                    strip: false,
                    exclude: Vec::new(),
                    tree: tree.into(),
                    src: None,
                    src_glob: None,
                    content: content.map(str::to_string),
                    json,
                    dest: dest.into(),
                    mode: None,
                    optional: false,
                    preserve_symlink: false,
                    allow_missing_libs: false,
                }
            };
        let assembly = ImageAssemblySpec {
            trees: vec![
                AssemblyTreeSpec {
                    strip: false,
                    id: "boot".into(),
                    path: "/build/boot".into(),
                },
                AssemblyTreeSpec {
                    strip: false,
                    id: "payload".into(),
                    path: "/build/payload".into(),
                },
            ],
            files: vec![
                file(
                    "boot",
                    "slot.json",
                    None,
                    Some(AssemblyJsonValue::Object(vec![(
                        "files".into(),
                        AssemblyJsonValue::String("${assembly.sha256_tree:payload}".into()),
                    )])),
                ),
                file("payload", "a.txt", Some("alpha"), None),
            ],
            ..ImageAssemblySpec::default()
        };
        let paths = assembly_step_paths(
            &assembly,
            &|template| Some(PathBuf::from(template.as_str())),
            &|id| match id {
                "boot" => Some(PathBuf::from("/build/boot")),
                "payload" => Some(PathBuf::from("/build/payload")),
                _ => None,
            },
        );
        assert_eq!(
            order_assembly_steps(&paths),
            Ok(vec![AssemblyStep::File(1), AssemblyStep::File(0)])
        );
    }

    /// Generated text and JSON read the paths their tokens name, so a file
    /// declared before the transform that writes its `${assembly.size:...}`
    /// path still runs after it.
    #[test]
    fn generated_content_runs_after_the_steps_whose_outputs_its_tokens_read() {
        use crate::{
            AssemblyFileSpec, AssemblyJsonValue, AssemblyTransformKindSpec, AssemblyTransformSpec,
            AssemblyTreeSpec,
        };
        let assembly = ImageAssemblySpec {
            trees: vec![AssemblyTreeSpec {
                strip: false,
                id: "boot".into(),
                path: "$assembly.work/boot".into(),
            }],
            files: vec![
                AssemblyFileSpec {
                    include: Vec::new(),
                    strip: false,
                    exclude: Vec::new(),
                    tree: "boot".into(),
                    src: None,
                    src_glob: None,
                    content: Some(
                        "root=${assembly.verity:$assembly.work/rootfs.erofs:root_hash}".into(),
                    ),
                    json: None,
                    dest: "cmdline.txt".into(),
                    mode: None,
                    optional: false,
                    preserve_symlink: false,
                    allow_missing_libs: false,
                },
                AssemblyFileSpec {
                    include: Vec::new(),
                    strip: false,
                    exclude: Vec::new(),
                    tree: "boot".into(),
                    src: None,
                    src_glob: None,
                    content: None,
                    json: Some(AssemblyJsonValue::Object(vec![(
                        "size".into(),
                        AssemblyJsonValue::String(
                            "${assembly.size:$assembly.work/rootfs.erofs}".into(),
                        ),
                    )])),
                    dest: "state.json".into(),
                    mode: None,
                    optional: false,
                    preserve_symlink: false,
                    allow_missing_libs: false,
                },
            ],
            transforms: vec![AssemblyTransformSpec {
                kind: AssemblyTransformKindSpec::Verity,
                src: Some("$provider.images/rootfs.erofs".into()),
                dest: "$assembly.work/rootfs.erofs".into(),
                deterministic: true,
                level: None,
                hash_dest: None,
                data_block_size: None,
                hash_block_size: None,
                salt: None,
                uuid: None,
                long: false,
            }],
            ..ImageAssemblySpec::default()
        };
        let paths = assembly_step_paths(
            &assembly,
            &|template| Some(PathBuf::from(template.as_str())),
            &|tree| Some(PathBuf::from(format!("/build/{tree}"))),
        );
        let order = order_assembly_steps(&paths).expect("no cycle");
        let transform = order
            .iter()
            .position(|step| *step == AssemblyStep::Transform(0))
            .expect("transform");
        assert!(
            order
                .iter()
                .position(|step| *step == AssemblyStep::File(0))
                .expect("file")
                > transform
        );
        assert!(
            order
                .iter()
                .position(|step| *step == AssemblyStep::File(1))
                .expect("json")
                > transform
        );
    }
}
