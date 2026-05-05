use super::*;

pub(crate) fn compile_image_feed(raw: &RawBuildConfig) -> ImageFeedSpec {
    ImageFeedSpec {
        install_entries: if raw.image.feed.install_entries.is_empty() {
            raw.install
                .iter()
                .map(|entry| InstallId::new(entry.id.clone()))
                .collect()
        } else {
            raw.image
                .feed
                .install_entries
                .iter()
                .cloned()
                .map(InstallId::new)
                .collect()
        },
        stage_files: if raw.image.feed.stage_files.is_empty() {
            raw.stage
                .files
                .iter()
                .map(|file| StageItemId::new(file.id.clone()))
                .collect()
        } else {
            raw.image
                .feed
                .stage_files
                .iter()
                .cloned()
                .map(StageItemId::new)
                .collect()
        },
        stage_env_sets: if raw.image.feed.stage_env_sets.is_empty() {
            raw.stage
                .env_sets
                .iter()
                .map(|env_set| StageItemId::new(env_set.id.clone()))
                .collect()
        } else {
            raw.image
                .feed
                .stage_env_sets
                .iter()
                .cloned()
                .map(StageItemId::new)
                .collect()
        },
        stage_services: if raw.image.feed.stage_services.is_empty() {
            raw.stage
                .services
                .iter()
                .map(|service| StageItemId::new(service.id.clone()))
                .collect()
        } else {
            raw.image
                .feed
                .stage_services
                .iter()
                .cloned()
                .map(StageItemId::new)
                .collect()
        },
    }
}

pub(crate) fn compile_buildroot_expected_image_format(
    raw: RawBuildrootExpectedImageFormat,
) -> BuildrootExpectedImageFormatSpec {
    match raw {
        RawBuildrootExpectedImageFormat::Tar => BuildrootExpectedImageFormatSpec::Tar,
        RawBuildrootExpectedImageFormat::Cpio => BuildrootExpectedImageFormatSpec::Cpio,
        RawBuildrootExpectedImageFormat::Ext2 => BuildrootExpectedImageFormatSpec::Ext2,
        RawBuildrootExpectedImageFormat::Ext3 => BuildrootExpectedImageFormatSpec::Ext3,
        RawBuildrootExpectedImageFormat::Ext4 => BuildrootExpectedImageFormatSpec::Ext4,
        RawBuildrootExpectedImageFormat::Ubifs => BuildrootExpectedImageFormatSpec::Ubifs,
        RawBuildrootExpectedImageFormat::Ubi => BuildrootExpectedImageFormatSpec::Ubi,
        RawBuildrootExpectedImageFormat::Jffs2 => BuildrootExpectedImageFormatSpec::Jffs2,
        RawBuildrootExpectedImageFormat::Romfs => BuildrootExpectedImageFormatSpec::Romfs,
        RawBuildrootExpectedImageFormat::Cramfs => BuildrootExpectedImageFormatSpec::Cramfs,
        RawBuildrootExpectedImageFormat::Cloop => BuildrootExpectedImageFormatSpec::Cloop,
        RawBuildrootExpectedImageFormat::F2fs => BuildrootExpectedImageFormatSpec::F2fs,
        RawBuildrootExpectedImageFormat::Btrfs => BuildrootExpectedImageFormatSpec::Btrfs,
        RawBuildrootExpectedImageFormat::Squashfs => BuildrootExpectedImageFormatSpec::Squashfs,
        RawBuildrootExpectedImageFormat::Raw => BuildrootExpectedImageFormatSpec::Raw,
        RawBuildrootExpectedImageFormat::Kernel => BuildrootExpectedImageFormatSpec::Kernel,
        RawBuildrootExpectedImageFormat::Erofs => BuildrootExpectedImageFormatSpec::Erofs,
    }
}

pub(crate) fn compile_buildroot_external_tree_mode(
    raw: Option<RawBuildrootExternalTreeMode>,
) -> BuildrootExternalTreeModeSpec {
    match raw.unwrap_or(RawBuildrootExternalTreeMode::Auto) {
        RawBuildrootExternalTreeMode::Auto => BuildrootExternalTreeModeSpec::Auto,
        RawBuildrootExternalTreeMode::Required => BuildrootExternalTreeModeSpec::Required,
        RawBuildrootExternalTreeMode::Disabled => BuildrootExternalTreeModeSpec::Disabled,
    }
}

pub(crate) fn compile_rootfs_validation_mode(
    raw: Option<RawStartingPointRootfsValidationMode>,
) -> StartingPointRootfsValidationModeSpec {
    match raw.unwrap_or(RawStartingPointRootfsValidationMode::RequireExists) {
        RawStartingPointRootfsValidationMode::RequireExists => {
            StartingPointRootfsValidationModeSpec::RequireExists
        }
        RawStartingPointRootfsValidationMode::RequireDirectory => {
            StartingPointRootfsValidationModeSpec::RequireDirectory
        }
        RawStartingPointRootfsValidationMode::RequireFile => {
            StartingPointRootfsValidationModeSpec::RequireFile
        }
        RawStartingPointRootfsValidationMode::AllowMissing => {
            StartingPointRootfsValidationModeSpec::AllowMissing
        }
    }
}

pub(crate) fn compile_starting_point_output_mode(
    raw: Option<RawStartingPointOutputMode>,
) -> StartingPointOutputModeSpec {
    match raw.unwrap_or(RawStartingPointOutputMode::CopyRootfs) {
        RawStartingPointOutputMode::CopyRootfs => StartingPointOutputModeSpec::CopyRootfs,
        RawStartingPointOutputMode::ArchiveOnly => StartingPointOutputModeSpec::ArchiveOnly,
        RawStartingPointOutputMode::CopyAndArchive => StartingPointOutputModeSpec::CopyAndArchive,
    }
}

pub(crate) fn compile_image_assembly(
    raw: Option<RawImageAssemblyConfig>,
) -> Option<ImageAssemblySpec> {
    let raw = raw?;
    let assembly = ImageAssemblySpec {
        work_dir: raw.work_dir.map(Into::into),
        out_dir: raw.out_dir.map(Into::into),
        trees: raw
            .trees
            .into_iter()
            .map(|tree| AssemblyTreeSpec {
                id: tree.id.into(),
                path: tree.path.into(),
            })
            .collect(),
        files: raw
            .files
            .into_iter()
            .map(|file| AssemblyFileSpec {
                tree: file.tree.into(),
                src: file.src.map(Into::into),
                src_glob: file.src_glob.map(Into::into),
                dest: file.dest,
                mode: file.mode,
                optional: file.optional,
                preserve_symlink: file.preserve_symlink,
            })
            .collect(),
        transforms: raw
            .transforms
            .into_iter()
            .map(|transform| AssemblyTransformSpec {
                kind: compile_assembly_transform_kind(transform.kind),
                src: transform.src.map(Into::into),
                dest: transform.dest.into(),
                deterministic: transform.deterministic.unwrap_or(true),
            })
            .collect(),
        filesystems: raw
            .filesystems
            .into_iter()
            .map(|filesystem| AssemblyFilesystemSpec {
                id: filesystem.id.into(),
                kind: compile_assembly_filesystem_kind(filesystem.kind),
                source_tree: filesystem.source_tree.into(),
                output: filesystem.output.into(),
                size: filesystem.size,
                deterministic: filesystem
                    .deterministic
                    .unwrap_or_else(|| default_assembly_filesystem_deterministic(filesystem.kind)),
            })
            .collect(),
        disks: raw
            .disks
            .into_iter()
            .map(|disk| AssemblyDiskSpec {
                id: disk.id,
                output: disk.output.into(),
                partition_table: compile_assembly_partition_table(disk.partition_table),
                signature: disk.signature,
                signature_text: disk.signature_text,
                partitions: disk
                    .partitions
                    .into_iter()
                    .map(|partition| AssemblyDiskPartitionSpec {
                        name: partition.name,
                        kind: partition.kind,
                        type_alias: partition.type_alias,
                        bootable: partition.bootable,
                        image: partition.image.into(),
                    })
                    .collect(),
            })
            .collect(),
        busybox_initramfs: raw
            .busybox_initramfs
            .into_iter()
            .map(|initramfs| AssemblyBusyboxInitramfsSpec {
                tree: initramfs.tree.into(),
                busybox: initramfs.busybox.into(),
                include_runtime_libs: initramfs.include_runtime_libs,
                applets: initramfs.applets,
            })
            .collect(),
    };
    Some(assembly)
}

fn compile_assembly_transform_kind(raw: RawAssemblyTransformKind) -> AssemblyTransformKindSpec {
    match raw {
        RawAssemblyTransformKind::CompileDts => AssemblyTransformKindSpec::CompileDts,
        RawAssemblyTransformKind::Gzip => AssemblyTransformKindSpec::Gzip,
        RawAssemblyTransformKind::Copy => AssemblyTransformKindSpec::Copy,
    }
}

fn compile_assembly_filesystem_kind(raw: RawAssemblyFilesystemKind) -> AssemblyFilesystemKindSpec {
    match raw {
        RawAssemblyFilesystemKind::Vfat => AssemblyFilesystemKindSpec::Vfat,
        RawAssemblyFilesystemKind::Cpio => AssemblyFilesystemKindSpec::Cpio,
        RawAssemblyFilesystemKind::CpioGzip => AssemblyFilesystemKindSpec::CpioGzip,
    }
}

fn default_assembly_filesystem_deterministic(raw: RawAssemblyFilesystemKind) -> bool {
    match raw {
        RawAssemblyFilesystemKind::Cpio | RawAssemblyFilesystemKind::CpioGzip => true,
        RawAssemblyFilesystemKind::Vfat => false,
    }
}

fn compile_assembly_partition_table(raw: RawAssemblyPartitionTable) -> AssemblyPartitionTableSpec {
    match raw {
        RawAssemblyPartitionTable::Mbr => AssemblyPartitionTableSpec::Mbr,
        RawAssemblyPartitionTable::Gpt => AssemblyPartitionTableSpec::Gpt,
    }
}
