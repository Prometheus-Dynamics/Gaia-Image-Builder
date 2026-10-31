use crate::raw::{
    RawBuildrootRootfsConfig, RawImageConfig, RawImageDefinition, RawImageFeedConfig,
    RawImageOutputConfig, RawStartingPointPackagesConfig,
};
use crate::raw_assembly::RawImageAssemblyConfig;

use super::{merge_by_key, merge_expected_images, merge_override_pairs, merge_string_lists};

/// Merges the `[image]` of a later layer onto the merged one. When the later
/// layer sets `kind` to another kind, the kind-specific fields are replaced
/// instead (see `kind_switch`). Returns the merged image and its notes.
pub(super) fn merge_image(
    base: RawImageConfig,
    overlay: RawImageConfig,
) -> (RawImageConfig, Vec<String>) {
    if let Some(note) = kind_switch_note(&base, &overlay) {
        let merged = RawImageConfig {
            definition: overlay.definition,
            feed: overlay.feed,
            signing: overlay.signing,
            checks: overlay.checks,
            hardware: overlay.hardware,
            layers: overlay.layers,
            output: merge_output(base.output, overlay.output),
            assembly: merge_image_assembly(base.assembly, overlay.assembly),
            kind_given: true,
            kind_layer: overlay.kind_layer,
        };
        return (merged, vec![note]);
    }
    let merged = RawImageConfig {
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
        output: merge_output(base.output, overlay.output),
        assembly: merge_image_assembly(base.assembly, overlay.assembly),
        signing: overlay.signing.or(base.signing),
        layers: overlay.layers.or(base.layers),
        checks: overlay.checks.or(base.checks),
        // Kept in order; a duplicate id is a validation error, not a merge.
        hardware: [base.hardware, overlay.hardware].concat(),
        kind_given: base.kind_given || overlay.kind_given,
        kind_layer: overlay.kind_layer.or(base.kind_layer),
    };
    (merged, Vec::new())
}

fn merge_output(base: RawImageOutputConfig, overlay: RawImageOutputConfig) -> RawImageOutputConfig {
    RawImageOutputConfig {
        collect_dir: overlay.collect_dir.or(base.collect_dir),
        archive_name: overlay.archive_name.or(base.archive_name),
        disk: overlay.disk.or(base.disk),
        export_dir: overlay.export_dir.or(base.export_dir),
        emit_report: base.emit_report || overlay.emit_report,
    }
}

/// The note of a kind switch: a later layer that sets `image.kind` to a kind
/// other than the merged base's. `None` when the kinds agree, or the base
/// has no kind of its own (its kind came from a layer without one).
fn kind_switch_note(base: &RawImageConfig, overlay: &RawImageConfig) -> Option<String> {
    let base_kind = kind_name(&base.definition);
    let overlay_kind = kind_name(&overlay.definition);
    let layer = overlay.kind_layer.as_deref()?;
    base.kind_layer.as_ref()?;
    if base_kind == overlay_kind {
        return None;
    }
    let fields = kind_specific_fields(base);
    let replaced = if fields.is_empty() {
        "no fields set".to_string()
    } else {
        fields.join(", ")
    };
    Some(format!(
        "image kind {overlay_kind} ({layer}) replaces {base_kind} fields: {replaced}"
    ))
}

fn kind_name(definition: &RawImageDefinition) -> &'static str {
    match definition {
        RawImageDefinition::Buildroot { .. } => "buildroot",
        RawImageDefinition::StartingPoint { .. } => "starting-point",
        RawImageDefinition::None {} => "none",
    }
}

/// The fields of `image` that a kind switch replaces: the definition's own
/// keys and the feed, signing, checks, hardware and layers. `[image.output]`
/// and `[image.assembly]` are shared by every kind and keep merging.
fn kind_specific_fields(image: &RawImageConfig) -> Vec<&'static str> {
    let mut fields = Vec::new();
    match &image.definition {
        RawImageDefinition::Buildroot {
            source,
            defconfig,
            defconfig_path,
            allow_fallback,
            legal_info,
            config_fragments,
            config_overrides,
            external_tree,
            external_tree_mode,
            expected_images,
            rootfs,
        } => {
            let set = [
                (source.is_some(), "source"),
                (defconfig.is_some(), "defconfig"),
                (defconfig_path.is_some(), "defconfig_path"),
                (*allow_fallback, "allow_fallback"),
                (*legal_info, "legal_info"),
                (!config_fragments.is_empty(), "config_fragments"),
                (!config_overrides.is_empty(), "config_overrides"),
                (external_tree.is_some(), "external_tree"),
                (external_tree_mode.is_some(), "external_tree_mode"),
                (!expected_images.is_empty(), "expected_images"),
                (
                    !rootfs.exclude_packages.is_empty(),
                    "rootfs.exclude_packages",
                ),
            ];
            fields.extend(set.iter().filter(|(on, _)| *on).map(|(_, name)| *name));
        }
        RawImageDefinition::StartingPoint {
            source,
            source_path,
            rootfs_path,
            image_partition,
            image_read_only,
            packages,
            rootfs_validation_mode,
            output_mode,
        } => {
            let set = [
                (source.is_some(), "source"),
                (source_path.is_some(), "source_path"),
                (!rootfs_path.trim().is_empty(), "rootfs_path"),
                (image_partition.is_some(), "image_partition"),
                (!*image_read_only, "image_read_only"),
                (
                    *packages != RawStartingPointPackagesConfig::default(),
                    "packages",
                ),
                (rootfs_validation_mode.is_some(), "rootfs_validation_mode"),
                (output_mode.is_some(), "output_mode"),
            ];
            fields.extend(set.iter().filter(|(on, _)| *on).map(|(_, name)| *name));
        }
        RawImageDefinition::None {} => {}
    }
    let shared = [
        (image.feed != RawImageFeedConfig::default(), "feed"),
        (image.signing.is_some(), "signing"),
        (image.checks.is_some(), "checks"),
        (!image.hardware.is_empty(), "hardware"),
        (image.layers.is_some(), "layers"),
    ];
    fields.extend(shared.iter().filter(|(on, _)| *on).map(|(_, name)| *name));
    fields
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
            devices: [base.devices, overlay.devices].concat(),
            files: [base.files, overlay.files].concat(),
            transforms: [base.transforms, overlay.transforms].concat(),
            filesystems: merge_by_key(base.filesystems, overlay.filesystems, |filesystem| {
                filesystem.id.clone()
            }),
            ubis: merge_by_key(base.ubis, overlay.ubis, |ubi| ubi.id.clone()),
            disks: merge_by_key(base.disks, overlay.disks, |disk| disk.id.clone()),
            archives: merge_by_key(base.archives, overlay.archives, |archive| {
                archive.id.clone()
            }),
            ocis: merge_by_key(base.ocis, overlay.ocis, |oci| oci.id.clone()),
            packages: merge_by_key(base.packages, overlay.packages, |package| {
                package.id.clone()
            }),
            checks: merge_by_key(base.checks, overlay.checks, |check| check.id.clone()),
            busybox_initramfs: [base.busybox_initramfs, overlay.busybox_initramfs].concat(),
            binaries: [base.binaries, overlay.binaries].concat(),
            kernel_modules: [base.kernel_modules, overlay.kernel_modules].concat(),
            strip_tool: overlay.strip_tool.or(base.strip_tool),
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
                legal_info: base_legal_info,
                config_fragments: base_config_fragments,
                config_overrides: base_config_overrides,
                external_tree: base_external_tree,
                external_tree_mode: base_external_tree_mode,
                expected_images: base_expected_images,
                rootfs: base_rootfs,
            },
            RawImageDefinition::Buildroot {
                source: overlay_source,
                defconfig: overlay_defconfig,
                defconfig_path: overlay_defconfig_path,
                allow_fallback: overlay_allow_fallback,
                legal_info: overlay_legal_info,
                config_fragments: overlay_config_fragments,
                config_overrides: overlay_config_overrides,
                external_tree: overlay_external_tree,
                external_tree_mode: overlay_external_tree_mode,
                expected_images: overlay_expected_images,
                rootfs: overlay_rootfs,
            },
        ) => RawImageDefinition::Buildroot {
            source: overlay_source.or(base_source),
            defconfig: overlay_defconfig.or(base_defconfig),
            defconfig_path: overlay_defconfig_path.or(base_defconfig_path),
            allow_fallback: base_allow_fallback || overlay_allow_fallback,
            legal_info: base_legal_info || overlay_legal_info,
            config_fragments: merge_string_lists(base_config_fragments, overlay_config_fragments),
            config_overrides: merge_override_pairs(base_config_overrides, overlay_config_overrides),
            external_tree: overlay_external_tree.or(base_external_tree),
            external_tree_mode: overlay_external_tree_mode.or(base_external_tree_mode),
            expected_images: merge_expected_images(base_expected_images, overlay_expected_images),
            // Kept as written (not merged as a set): validation reports a
            // name listed twice, and the provider dedups when it applies them.
            rootfs: RawBuildrootRootfsConfig {
                exclude_packages: base_rootfs
                    .exclude_packages
                    .into_iter()
                    .chain(overlay_rootfs.exclude_packages)
                    .collect(),
            },
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
                legal_info: false,
                config_fragments,
                config_overrides,
                external_tree: None,
                external_tree_mode: None,
                expected_images,
                rootfs,
            },
        ) if expected_images.is_empty()
            && config_fragments.is_empty()
            && config_overrides.is_empty()
            && rootfs.exclude_packages.is_empty() =>
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
                legal_info: false,
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
                rootfs: Default::default(),
            },
            RawImageDefinition::Buildroot {
                source: None,
                defconfig: Some("evk_defconfig".into()),
                defconfig_path: None,
                allow_fallback: false,
                legal_info: false,
                config_fragments: vec!["target.fragment".into()],
                config_overrides: vec![("BR2_ROOTFS_POST_IMAGE_SCRIPT".into(), "\"\"".into())],
                external_tree: None,
                external_tree_mode: None,
                expected_images: vec![],
                rootfs: Default::default(),
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
                assert_eq!(defconfig.as_deref(), Some("evk_defconfig"));
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
