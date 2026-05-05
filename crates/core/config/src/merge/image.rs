use crate::raw::{
    RawImageConfig, RawImageDefinition, RawImageFeedConfig, RawImageOutputConfig,
    RawStartingPointPackagesConfig,
};
use crate::raw_assembly::RawImageAssemblyConfig;

use super::{merge_by_key, merge_expected_images, merge_override_pairs, merge_string_lists};

pub(super) fn merge_image(base: RawImageConfig, overlay: RawImageConfig) -> RawImageConfig {
    RawImageConfig {
        definition: merge_image_definition(base.definition, overlay.definition),
        feed: RawImageFeedConfig {
            install_entries: merge_string_lists(
                base.feed.install_entries,
                overlay.feed.install_entries,
            ),
            stage_files: merge_string_lists(base.feed.stage_files, overlay.feed.stage_files),
            stage_env_sets: merge_string_lists(
                base.feed.stage_env_sets,
                overlay.feed.stage_env_sets,
            ),
            stage_services: merge_string_lists(
                base.feed.stage_services,
                overlay.feed.stage_services,
            ),
        },
        output: RawImageOutputConfig {
            collect_dir: overlay.output.collect_dir.or(base.output.collect_dir),
            archive_name: overlay.output.archive_name.or(base.output.archive_name),
            emit_report: base.output.emit_report || overlay.output.emit_report,
        },
        assembly: merge_image_assembly(base.assembly, overlay.assembly),
    }
}

fn merge_image_assembly(
    base: Option<RawImageAssemblyConfig>,
    overlay: Option<RawImageAssemblyConfig>,
) -> Option<RawImageAssemblyConfig> {
    match (base, overlay) {
        (Some(base), Some(overlay)) => Some(RawImageAssemblyConfig {
            work_dir: overlay.work_dir.or(base.work_dir),
            out_dir: overlay.out_dir.or(base.out_dir),
            trees: merge_by_key(base.trees, overlay.trees, |tree| tree.id.clone()),
            dirs: [base.dirs, overlay.dirs].concat(),
            symlinks: [base.symlinks, overlay.symlinks].concat(),
            files: [base.files, overlay.files].concat(),
            transforms: [base.transforms, overlay.transforms].concat(),
            filesystems: merge_by_key(base.filesystems, overlay.filesystems, |filesystem| {
                filesystem.id.clone()
            }),
            disks: merge_by_key(base.disks, overlay.disks, |disk| disk.id.clone()),
            busybox_initramfs: [base.busybox_initramfs, overlay.busybox_initramfs].concat(),
        }),
        (None, Some(assembly)) | (Some(assembly), None) => Some(assembly),
        (None, None) => None,
    }
}

fn merge_image_definition(
    base: RawImageDefinition,
    overlay: RawImageDefinition,
) -> RawImageDefinition {
    match (base, overlay) {
        (
            RawImageDefinition::Buildroot {
                source: base_source,
                defconfig: base_defconfig,
                defconfig_path: base_defconfig_path,
                allow_fallback: base_allow_fallback,
                config_fragments: base_config_fragments,
                config_overrides: base_config_overrides,
                external_tree: base_external_tree,
                external_tree_mode: base_external_tree_mode,
                expected_images: base_expected_images,
            },
            RawImageDefinition::Buildroot {
                source: overlay_source,
                defconfig: overlay_defconfig,
                defconfig_path: overlay_defconfig_path,
                allow_fallback: overlay_allow_fallback,
                config_fragments: overlay_config_fragments,
                config_overrides: overlay_config_overrides,
                external_tree: overlay_external_tree,
                external_tree_mode: overlay_external_tree_mode,
                expected_images: overlay_expected_images,
            },
        ) => RawImageDefinition::Buildroot {
            source: overlay_source.or(base_source),
            defconfig: overlay_defconfig.or(base_defconfig),
            defconfig_path: overlay_defconfig_path.or(base_defconfig_path),
            allow_fallback: base_allow_fallback || overlay_allow_fallback,
            config_fragments: merge_string_lists(base_config_fragments, overlay_config_fragments),
            config_overrides: merge_override_pairs(base_config_overrides, overlay_config_overrides),
            external_tree: overlay_external_tree.or(base_external_tree),
            external_tree_mode: overlay_external_tree_mode.or(base_external_tree_mode),
            expected_images: merge_expected_images(base_expected_images, overlay_expected_images),
        },
        (
            RawImageDefinition::StartingPoint {
                source: base_source,
                source_path: base_source_path,
                rootfs_path: base_rootfs_path,
                image_partition: base_image_partition,
                image_read_only: base_image_read_only,
                packages: base_packages,
                rootfs_validation_mode: base_rootfs_validation_mode,
                output_mode: base_output_mode,
            },
            RawImageDefinition::StartingPoint {
                source: overlay_source,
                source_path: overlay_source_path,
                rootfs_path: overlay_rootfs_path,
                image_partition: overlay_image_partition,
                image_read_only: overlay_image_read_only,
                packages: overlay_packages,
                rootfs_validation_mode: overlay_rootfs_validation_mode,
                output_mode: overlay_output_mode,
            },
        ) => RawImageDefinition::StartingPoint {
            source: overlay_source.or(base_source),
            source_path: overlay_source_path.or(base_source_path),
            rootfs_path: if overlay_rootfs_path.trim().is_empty() {
                base_rootfs_path
            } else {
                overlay_rootfs_path
            },
            image_partition: overlay_image_partition.or(base_image_partition),
            image_read_only: overlay_image_read_only && base_image_read_only,
            packages: RawStartingPointPackagesConfig {
                enabled: base_packages.enabled || overlay_packages.enabled,
                execute: base_packages.execute || overlay_packages.execute,
                manager: overlay_packages.manager.or(base_packages.manager),
                release_version: overlay_packages
                    .release_version
                    .or(base_packages.release_version),
                allow_major_upgrade: base_packages.allow_major_upgrade
                    || overlay_packages.allow_major_upgrade,
                update: base_packages.update || overlay_packages.update,
                dist_upgrade: base_packages.dist_upgrade || overlay_packages.dist_upgrade,
                install: if overlay_packages.install.is_empty() {
                    base_packages.install
                } else {
                    overlay_packages.install
                },
                remove: if overlay_packages.remove.is_empty() {
                    base_packages.remove
                } else {
                    overlay_packages.remove
                },
                extra_args: if overlay_packages.extra_args.is_empty() {
                    base_packages.extra_args
                } else {
                    overlay_packages.extra_args
                },
                os_release_path: overlay_packages
                    .os_release_path
                    .or(base_packages.os_release_path),
            },
            rootfs_validation_mode: overlay_rootfs_validation_mode.or(base_rootfs_validation_mode),
            output_mode: overlay_output_mode.or(base_output_mode),
        },
        (
            base_definition,
            RawImageDefinition::Buildroot {
                source: _,
                defconfig: None,
                defconfig_path: None,
                allow_fallback: false,
                config_fragments,
                config_overrides,
                external_tree: None,
                external_tree_mode: None,
                expected_images,
            },
        ) if expected_images.is_empty()
            && config_fragments.is_empty()
            && config_overrides.is_empty() =>
        {
            base_definition
        }
        (base_definition, overlay_definition) => match overlay_definition {
            RawImageDefinition::StartingPoint {
                source,
                source_path,
                rootfs_path,
                image_partition,
                packages,
                rootfs_validation_mode,
                output_mode,
                image_read_only,
            } if source.is_none()
                && source_path.is_none()
                && rootfs_path.trim().is_empty()
                && image_partition.is_none()
                && rootfs_validation_mode.is_none()
                && output_mode.is_none()
                && image_read_only
                && !packages.enabled
                && !packages.execute
                && packages.manager.is_none()
                && packages.release_version.is_none()
                && !packages.allow_major_upgrade
                && !packages.update
                && !packages.dist_upgrade
                && packages.install.is_empty()
                && packages.remove.is_empty()
                && packages.extra_args.is_empty()
                && packages.os_release_path.is_none() =>
            {
                base_definition
            }
            overlay_definition => overlay_definition,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::merge_image_definition;
    use crate::raw::{
        RawBuildrootExpectedImageConfig, RawBuildrootExpectedImageFormat, RawImageDefinition,
    };

    #[test]
    fn layered_buildroot_image_definition_composes_overrides_and_expected_images() {
        let merged = merge_image_definition(
            RawImageDefinition::Buildroot {
                source: None,
                defconfig: None,
                defconfig_path: None,
                allow_fallback: false,
                config_fragments: vec!["storage.fragment".into()],
                config_overrides: vec![
                    ("BR2_TARGET_ROOTFS_EXT2".into(), "n".into()),
                    ("BR2_TARGET_ROOTFS_SQUASHFS".into(), "y".into()),
                ],
                external_tree: None,
                external_tree_mode: None,
                expected_images: vec![RawBuildrootExpectedImageConfig {
                    name: "rootfs.squashfs".into(),
                    format: RawBuildrootExpectedImageFormat::Squashfs,
                    required: true,
                }],
            },
            RawImageDefinition::Buildroot {
                source: None,
                defconfig: Some("raspberrypicm5io_defconfig".into()),
                defconfig_path: None,
                allow_fallback: false,
                config_fragments: vec!["target.fragment".into()],
                config_overrides: vec![("BR2_ROOTFS_POST_IMAGE_SCRIPT".into(), "\"\"".into())],
                external_tree: None,
                external_tree_mode: None,
                expected_images: vec![],
            },
        );

        match merged {
            RawImageDefinition::Buildroot {
                defconfig,
                config_fragments,
                config_overrides,
                expected_images,
                ..
            } => {
                assert_eq!(defconfig.as_deref(), Some("raspberrypicm5io_defconfig"));
                assert_eq!(
                    config_fragments,
                    vec![
                        "storage.fragment".to_string(),
                        "target.fragment".to_string()
                    ]
                );
                assert_eq!(
                    config_overrides,
                    vec![
                        (
                            "BR2_ROOTFS_POST_IMAGE_SCRIPT".to_string(),
                            "\"\"".to_string()
                        ),
                        ("BR2_TARGET_ROOTFS_EXT2".to_string(), "n".to_string()),
                        ("BR2_TARGET_ROOTFS_SQUASHFS".to_string(), "y".to_string()),
                    ]
                );
                assert_eq!(expected_images.len(), 1);
                assert_eq!(expected_images[0].name, "rootfs.squashfs");
                assert!(expected_images[0].required);
            }
            other => panic!("expected buildroot image, got {other:?}"),
        }
    }
}
