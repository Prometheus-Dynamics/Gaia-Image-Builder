use super::*;
use crate::raw::RawImageLayersConfig;
use crate::raw_assembly::{
    RawAssemblyArchiveConfig, RawAssemblyDeviceKind, RawAssemblyJsonValue, RawAssemblyOciConfig,
};
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyBinarySpec, AssemblyCompressedFsSpec,
    AssemblyGptDiskSpec, AssemblyGptPartitionSpec, AssemblyJsonValue, AssemblyOciConfigSpec,
    AssemblyOciLayerSpec, AssemblyOciSpec,
};

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
        RawBuildrootExpectedImageFormat::Xfs => BuildrootExpectedImageFormatSpec::Xfs,
        RawBuildrootExpectedImageFormat::File => BuildrootExpectedImageFormatSpec::File,
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

/// `[image.layers]` as the spec holds it. The layers are sorted by name, so
/// the spec does not depend on the order the tables were written in.
pub(crate) fn compile_image_layers(raw: RawImageLayersConfig) -> gaia_spec::ImageLayersSpec {
    let default = raw
        .default
        .unwrap_or_else(|| raw.order.first().cloned().unwrap_or_default());
    gaia_spec::ImageLayersSpec {
        order: raw.order,
        default,
        overrides: raw.overrides,
        erofs: raw.erofs.unwrap_or(true),
        verity: raw.verity.unwrap_or(false),
        erofs_options: raw.erofs_options.unwrap_or_else(|| {
            gaia_spec::DEFAULT_LAYER_EROFS_OPTIONS
                .iter()
                .map(|option| option.to_string())
                .collect()
        }),
        layers: raw
            .layers
            .into_iter()
            .map(|(name, layer)| gaia_spec::ImageLayerSpec {
                name,
                packages: layer.packages,
                paths: layer.paths,
                installs: layer.installs,
                stage: layer.stage,
                services: layer.services,
                erofs_options: layer.erofs_options,
            })
            .collect(),
    }
}

/// Sets `BR2_TARGET_ROOTFS_TAR=y` (replacing any value the file gave): with
/// layers, Buildroot must write the rootfs tar the layers are split from.
pub(crate) fn force_rootfs_tar_override(
    mut overrides: Vec<(String, String)>,
) -> Vec<(String, String)> {
    const KEY: &str = "BR2_TARGET_ROOTFS_TAR";
    match overrides.iter_mut().find(|(key, _)| key == KEY) {
        Some((_, value)) => *value = "y".into(),
        None => overrides.push((KEY.into(), "y".into())),
    }
    overrides
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
                strip: tree.strip,
            })
            .collect(),
        dirs: raw
            .dirs
            .into_iter()
            .map(|dir| AssemblyDirSpec {
                tree: dir.tree.into(),
                path: dir.path,
                mode: dir.mode,
            })
            .collect(),
        symlinks: raw
            .symlinks
            .into_iter()
            .map(|symlink| AssemblySymlinkSpec {
                tree: symlink.tree.into(),
                path: symlink.path,
                target: symlink.target,
            })
            .collect(),
        files: raw
            .files
            .into_iter()
            .map(|file| AssemblyFileSpec {
                tree: file.tree.into(),
                src: file.src.map(Into::into),
                src_glob: file.src_glob.map(Into::into),
                exclude: file.exclude,
                include: file.include,
                strip: file.strip,
                dest: file.dest,
                mode: file.mode,
                optional: file.optional,
                preserve_symlink: file.preserve_symlink,
                allow_missing_libs: file.allow_missing_libs,
                content: file.content,
                json: file.json.map(compile_assembly_json),
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
                level: transform.level,
                hash_dest: transform.hash_dest.map(Into::into),
                data_block_size: transform.data_block_size,
                hash_block_size: transform.hash_block_size,
                salt: transform.salt,
                uuid: transform.uuid,
                long: transform.long,
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
                free: filesystem.free,
                deterministic: filesystem
                    .deterministic
                    .unwrap_or_else(|| default_assembly_filesystem_deterministic(filesystem.kind)),
                compression_level: filesystem.compression_level,
                publish: filesystem.publish,
                label: filesystem.label,
                uuid: filesystem.uuid,
                journal: filesystem.journal,
                compressed: AssemblyCompressedFsSpec {
                    compression: filesystem.compression,
                    block_size: filesystem.block_size,
                    pcluster_size: filesystem.pcluster_size,
                    dedupe: filesystem.dedupe,
                    fragments: filesystem.fragments,
                    ztailpacking: filesystem.ztailpacking,
                },
                fat_size: filesystem.fat_size,
                reserved_percent: filesystem.reserved_percent,
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
                first_lba: disk.first_lba,
                alignment_lba: disk.alignment_lba,
                truncate: disk.truncate.map(compile_assembly_disk_truncate),
                ebr_placement: match disk.ebr_placement {
                    None | Some(RawAssemblyEbrPlacement::Default) => {
                        AssemblyEbrPlacementSpec::Default
                    }
                    Some(RawAssemblyEbrPlacement::Packed) => AssemblyEbrPlacementSpec::Packed,
                },
                partitions: disk
                    .partitions
                    .into_iter()
                    .map(|partition| AssemblyDiskPartitionSpec {
                        name: partition.name,
                        kind: partition.kind,
                        type_alias: partition.type_alias,
                        bootable: partition.bootable,
                        image: partition.image.map(Into::into),
                        size: partition.size,
                        wipe: partition.wipe,
                        materialize: partition.materialize.unwrap_or(true),
                        gpt: AssemblyGptPartitionSpec {
                            guid: partition.guid,
                        },
                    })
                    .collect(),
                gpt: AssemblyGptDiskSpec {
                    disk_guid: disk.disk_guid,
                },
            })
            .collect(),
        archives: raw
            .archives
            .into_iter()
            .map(compile_assembly_archive)
            .collect(),
        ocis: raw.ocis.into_iter().map(compile_assembly_oci).collect(),
        binaries: raw
            .binaries
            .into_iter()
            .map(|binary| AssemblyBinarySpec {
                tree: binary.tree.into(),
                src: binary.src.into(),
                dest: binary.dest,
                sysroot: binary.sysroot.map(Into::into),
                mode: binary.mode,
                strip: binary.strip,
            })
            .collect(),
        busybox_initramfs: raw
            .busybox_initramfs
            .into_iter()
            .map(|initramfs| AssemblyBusyboxInitramfsSpec {
                tree: initramfs.tree.into(),
                busybox: initramfs.busybox.into(),
                include_runtime_libs: initramfs.include_runtime_libs,
                sysroot: initramfs.sysroot.map(Into::into),
                applets: initramfs.applets,
            })
            .collect(),
        kernel_modules: raw
            .kernel_modules
            .into_iter()
            .map(|modules| AssemblyKernelModulesSpec {
                tree: modules.tree.into(),
                from: modules.from.into(),
                kernel_version: modules.kernel_version,
                modules: modules.modules,
                depmod: modules.depmod.map(Into::into),
                metadata: modules.metadata,
            })
            .collect(),
        devices: raw
            .devices
            .into_iter()
            .map(|device| AssemblyDeviceSpec {
                tree: device.tree.into(),
                path: device.path,
                kind: match device.kind {
                    RawAssemblyDeviceKind::Char => AssemblyDeviceKindSpec::Char,
                    RawAssemblyDeviceKind::Block => AssemblyDeviceKindSpec::Block,
                    RawAssemblyDeviceKind::Fifo => AssemblyDeviceKindSpec::Fifo,
                },
                major: device.major,
                minor: device.minor,
                mode: device.mode,
            })
            .collect(),
        strip_tool: raw.strip_tool.map(Into::into),
        // Filled in once the sources are known (see source_tokens).
        assembly_sources: Vec::new(),
    };
    Some(assembly)
}

fn compile_assembly_disk_truncate(raw: RawAssemblyDiskTruncate) -> AssemblyDiskTruncateSpec {
    match raw {
        RawAssemblyDiskTruncate::LastData => AssemblyDiskTruncateSpec::LastData,
    }
}

fn compile_assembly_transform_kind(raw: RawAssemblyTransformKind) -> AssemblyTransformKindSpec {
    match raw {
        RawAssemblyTransformKind::CompileDts => AssemblyTransformKindSpec::CompileDts,
        RawAssemblyTransformKind::Gzip => AssemblyTransformKindSpec::Gzip,
        RawAssemblyTransformKind::Zstd => AssemblyTransformKindSpec::Zstd,
        RawAssemblyTransformKind::Xz => AssemblyTransformKindSpec::Xz,
        RawAssemblyTransformKind::Copy => AssemblyTransformKindSpec::Copy,
        RawAssemblyTransformKind::Verity => AssemblyTransformKindSpec::Verity,
    }
}

fn compile_assembly_filesystem_kind(raw: RawAssemblyFilesystemKind) -> AssemblyFilesystemKindSpec {
    match raw {
        RawAssemblyFilesystemKind::Vfat => AssemblyFilesystemKindSpec::Vfat,
        RawAssemblyFilesystemKind::Cpio => AssemblyFilesystemKindSpec::Cpio,
        RawAssemblyFilesystemKind::CpioGzip => AssemblyFilesystemKindSpec::CpioGzip,
        RawAssemblyFilesystemKind::CpioZstd => AssemblyFilesystemKindSpec::CpioZstd,
        RawAssemblyFilesystemKind::CpioXz => AssemblyFilesystemKindSpec::CpioXz,
        RawAssemblyFilesystemKind::Ext4 => AssemblyFilesystemKindSpec::Ext4,
        RawAssemblyFilesystemKind::Squashfs => AssemblyFilesystemKindSpec::Squashfs,
        RawAssemblyFilesystemKind::Erofs => AssemblyFilesystemKindSpec::Erofs,
    }
}

fn default_assembly_filesystem_deterministic(raw: RawAssemblyFilesystemKind) -> bool {
    match raw {
        RawAssemblyFilesystemKind::Cpio
        | RawAssemblyFilesystemKind::CpioGzip
        | RawAssemblyFilesystemKind::CpioZstd
        | RawAssemblyFilesystemKind::CpioXz
        | RawAssemblyFilesystemKind::Ext4
        | RawAssemblyFilesystemKind::Squashfs
        | RawAssemblyFilesystemKind::Erofs => true,
        RawAssemblyFilesystemKind::Vfat => false,
    }
}

fn compile_assembly_partition_table(raw: RawAssemblyPartitionTable) -> AssemblyPartitionTableSpec {
    match raw {
        RawAssemblyPartitionTable::Mbr => AssemblyPartitionTableSpec::Mbr,
        RawAssemblyPartitionTable::Gpt => AssemblyPartitionTableSpec::Gpt,
    }
}

/// `generated` members come first, then `members` in their declared order.
/// A generated member with neither `entries` nor `json` is an empty file.
fn compile_assembly_archive(raw: RawAssemblyArchiveConfig) -> AssemblyArchiveSpec {
    let generated = raw.generated.into_iter().map(|generated| {
        let entries = match (&generated.entries, &generated.json) {
            (None, None) => Some(Vec::new()),
            _ => generated.entries,
        };
        AssemblyArchiveMemberSpec {
            tree: None,
            src_dir: None,
            src_glob: None,
            prefix: String::new(),
            exclude: Vec::new(),
            name: generated.name,
            src: None,
            entries,
            json: generated.json.map(compile_assembly_json),
        }
    });
    let members = raw
        .members
        .into_iter()
        .map(|member| AssemblyArchiveMemberSpec {
            name: member.name,
            src: member.src.map(Into::into),
            entries: member.entries,
            json: member.json.map(compile_assembly_json),
            tree: member.tree.map(Into::into),
            src_dir: member.src_dir.map(Into::into),
            src_glob: member.src_glob.map(Into::into),
            prefix: member.prefix,
            exclude: member.exclude,
        });
    AssemblyArchiveSpec {
        id: raw.id,
        output: raw.output.into(),
        members: generated.chain(members).collect(),
    }
}

/// An `[[image.assembly.oci]]` entry. The defaults are the spec's: an OCI
/// layout, gzip layers and `linux`. The validator reports the rest.
fn compile_assembly_oci(raw: RawAssemblyOciConfig) -> AssemblyOciSpec {
    let config = raw.config;
    AssemblyOciSpec {
        id: raw.id,
        dest: raw.dest.into(),
        format: raw.format.unwrap_or_else(|| "oci-layout".into()),
        layers: raw
            .layers
            .into_iter()
            .map(|layer| AssemblyOciLayerSpec {
                tar: layer.tar.map(Into::into),
                tree: layer.tree.map(Into::into),
            })
            .collect(),
        compression: raw.compression.unwrap_or_else(|| "gzip".into()),
        reference: raw.reference,
        config: AssemblyOciConfigSpec {
            architecture: config.architecture.unwrap_or_default(),
            os: config.os.unwrap_or_else(|| "linux".into()),
            entrypoint: config.entrypoint,
            cmd: config.cmd,
            env: config.env,
            working_dir: config.working_dir,
            stop_signal: config.stop_signal,
            labels: config.labels.into_iter().collect(),
        },
    }
}

/// A `json = { ... }` value as the spec holds it, keys in written order.
fn compile_assembly_json(raw: RawAssemblyJsonValue) -> AssemblyJsonValue {
    match raw {
        RawAssemblyJsonValue::String(text) => AssemblyJsonValue::String(text),
        RawAssemblyJsonValue::Integer(number) => AssemblyJsonValue::Integer(number),
        RawAssemblyJsonValue::Bool(flag) => AssemblyJsonValue::Bool(flag),
        RawAssemblyJsonValue::Array(items) => {
            AssemblyJsonValue::Array(items.into_iter().map(compile_assembly_json).collect())
        }
        RawAssemblyJsonValue::Object(members) => AssemblyJsonValue::Object(
            members
                .into_iter()
                .map(|(key, value)| (key, compile_assembly_json(value)))
                .collect(),
        ),
    }
}
