use std::collections::HashSet;
use std::fs;

use gaia_spec::{
    BuildrootExpectedImageFormatSpec, BuildrootExternalTreeModeSpec, ImageDefinition,
    ResolvedBuildSpec, StartingPointImageSpec, StartingPointOutputModeSpec,
    StartingPointRootfsValidationModeSpec,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};
use crate::image_assembly::{assembly_expected_image_names, validate_image_assembly};
use crate::workspace::resolve_workspace_path;

pub(crate) fn validate_image_contract(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let assembly_expected_image_names = assembly_expected_image_names(spec);
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            if buildroot.defconfig.is_none()
                && buildroot.defconfig_path.is_none()
                && buildroot.external_tree.is_none()
                && buildroot.source.is_none()
            {
                diagnostics.push(error(
                "buildroot_config_missing",
                "buildroot image requires at least one of defconfig, defconfig_path, external_tree, or source"
                    .into(),
                Some("image".into()),
            ));
            }
            if let Some(defconfig_path) = &buildroot.defconfig_path
                && defconfig_path.trim().is_empty()
            {
                diagnostics.push(error(
                    "buildroot_defconfig_path_empty",
                    "buildroot defconfig_path cannot be empty".into(),
                    Some("image".into()),
                ));
            }
            if let Some(source_id) = &buildroot.source
                && !spec.sources.iter().any(|source| &source.id == source_id)
            {
                diagnostics.push(error(
                    "buildroot_source_unknown",
                    format!("buildroot source '{}' does not exist", source_id.as_str()),
                    Some("image".into()),
                ));
            }
            if buildroot.external_tree_mode == BuildrootExternalTreeModeSpec::Required
                && buildroot.external_tree.is_none()
            {
                diagnostics.push(error(
                    "buildroot_external_tree_required",
                    "buildroot external_tree_mode=required requires external_tree to be set".into(),
                    Some("image".into()),
                ));
            }
            if buildroot.external_tree_mode == BuildrootExternalTreeModeSpec::Disabled
                && buildroot.external_tree.is_some()
            {
                diagnostics.push(error(
                    "buildroot_external_tree_disabled",
                    "buildroot external_tree_mode=disabled does not allow external_tree to be set"
                        .into(),
                    Some("image".into()),
                ));
            }
            if let Some(defconfig_path) = &buildroot.defconfig_path
                && !defconfig_path.trim().is_empty()
            {
                match resolve_workspace_path(spec, defconfig_path) {
                    Ok(resolved_path) => {
                        if !resolved_path.is_file() {
                            diagnostics.push(error(
                            "buildroot_defconfig_path_missing",
                            format!(
                                "buildroot defconfig_path '{}' does not resolve to an existing file",
                                resolved_path.display()
                            ),
                            Some("image".into()),
                        ));
                        } else if let Ok(defconfig_contents) = fs::read_to_string(&resolved_path) {
                            diagnostics.extend(
                                validate_buildroot_expected_images_against_defconfig(
                                    buildroot,
                                    &defconfig_contents,
                                    &assembly_expected_image_names,
                                ),
                            );
                        }
                    }
                    Err(message) => diagnostics.push(error(
                        "buildroot_defconfig_path_invalid",
                        message,
                        Some("image".into()),
                    )),
                }
            }
            if !buildroot.config_fragments.is_empty()
                && buildroot.defconfig.is_none()
                && buildroot.defconfig_path.is_none()
            {
                diagnostics.push(error(
                    "buildroot_config_fragments_require_base_config",
                    "buildroot config_fragments require defconfig or defconfig_path to also be set"
                        .into(),
                    Some("image".into()),
                ));
            }
            for fragment in &buildroot.config_fragments {
                if fragment.trim().is_empty() {
                    diagnostics.push(error(
                        "buildroot_config_fragment_empty",
                        "buildroot config_fragments entries cannot be empty".into(),
                        Some("image".into()),
                    ));
                    continue;
                }
                match resolve_workspace_path(spec, fragment) {
                    Ok(resolved_path) => {
                        if !resolved_path.is_file() {
                            diagnostics.push(error(
                            "buildroot_config_fragment_missing",
                            format!(
                                "buildroot config fragment '{}' does not resolve to an existing file",
                                resolved_path.display()
                            ),
                            Some("image".into()),
                        ));
                        }
                    }
                    Err(message) => diagnostics.push(error(
                        "buildroot_config_fragment_invalid",
                        message,
                        Some("image".into()),
                    )),
                }
            }
            if !buildroot.config_overrides.is_empty()
                && buildroot.defconfig.is_none()
                && buildroot.defconfig_path.is_none()
            {
                diagnostics.push(error(
                    "buildroot_config_overrides_require_base_config",
                    "buildroot config_overrides require defconfig or defconfig_path to also be set"
                        .into(),
                    Some("image".into()),
                ));
            }
            for (key, value) in &buildroot.config_overrides {
                if key.trim().is_empty() {
                    diagnostics.push(error(
                        "buildroot_config_override_key_empty",
                        "buildroot config_overrides keys cannot be empty".into(),
                        Some("image".into()),
                    ));
                    continue;
                }
                if !key.starts_with("BR2_") {
                    diagnostics.push(error(
                        "buildroot_config_override_key_invalid",
                        format!("buildroot config_overrides key '{key}' must start with 'BR2_'"),
                        Some("image".into()),
                    ));
                }
                if value.trim().is_empty() {
                    diagnostics.push(error(
                        "buildroot_config_override_value_empty",
                        format!(
                            "buildroot config_overrides entry '{key}' cannot have an empty value"
                        ),
                        Some("image".into()),
                    ));
                }
            }
            let mut expected_image_names = HashSet::new();
            for expected_image in &buildroot.expected_images {
                if expected_image.name.trim().is_empty() {
                    diagnostics.push(error(
                        "buildroot_expected_image_empty",
                        "buildroot expected image name cannot be empty".into(),
                        Some("image".into()),
                    ));
                } else if !expected_image_names.insert(expected_image.name.clone()) {
                    diagnostics.push(error(
                        "buildroot_expected_image_duplicate",
                        format!(
                            "buildroot expected image '{}' is declared more than once",
                            expected_image.name
                        ),
                        Some("image".into()),
                    ));
                } else if !expected_image_name_matches_format(
                    &expected_image.name,
                    expected_image.format,
                ) {
                    diagnostics.push(error(
                        "buildroot_expected_image_name_mismatch",
                        format!(
                            "buildroot expected image '{}' does not match declared format '{}'",
                            expected_image.name,
                            expected_image.format.as_str()
                        ),
                        Some("image".into()),
                    ));
                }
            }
        }
        ImageDefinition::StartingPoint(starting_point) => {
            if starting_point.source.is_none() && starting_point.rootfs_path.trim().is_empty() {
                diagnostics.push(error(
                    "starting_point_rootfs_empty",
                    "starting-point image requires either source or a non-empty rootfs path".into(),
                    Some("image".into()),
                ));
            }
            if let Some(source_id) = &starting_point.source
                && !spec.sources.iter().any(|source| &source.id == source_id)
            {
                diagnostics.push(error(
                    "starting_point_source_unknown",
                    format!(
                        "starting-point source '{}' does not exist",
                        source_id.as_str()
                    ),
                    Some("image".into()),
                ));
            }
            if starting_point.source.is_some()
                && starting_point
                    .source_path
                    .as_deref()
                    .is_some_and(|path| path.trim().is_empty())
            {
                diagnostics.push(error(
                    "starting_point_source_path_empty",
                    "starting-point source_path cannot be empty when source is set".into(),
                    Some("image".into()),
                ));
            }
            if starting_point.output_mode == StartingPointOutputModeSpec::ArchiveOnly
                && spec.image.output.archive_name.is_none()
            {
                diagnostics.push(error(
                    "starting_point_archive_without_archive_name",
                    "starting-point output_mode=archive-only requires image.output.archive_name"
                        .into(),
                    Some("image.output".into()),
                ));
            }
            if starting_point.output_mode == StartingPointOutputModeSpec::CopyAndArchive
                && spec.image.output.archive_name.is_none()
            {
                diagnostics.push(error(
                    "starting_point_combined_output_incomplete",
                    "starting-point output_mode=copy-and-archive requires archive_name".into(),
                    Some("image.output".into()),
                ));
            }
            if starting_point.rootfs_validation_mode
                == StartingPointRootfsValidationModeSpec::AllowMissing
                && starting_point.output_mode == StartingPointOutputModeSpec::CopyAndArchive
                && spec.image.output.archive_name.is_some()
            {
                diagnostics.push(warning(
                    "starting_point_archive_from_optional_rootfs",
                    "starting-point allows missing rootfs while also requesting archive output"
                        .into(),
                    Some("image".into()),
                ));
            }
            if starting_point_looks_like_raw_image(starting_point)
                && spec.image.output.archive_name.is_none()
            {
                diagnostics.push(error(
                    "starting_point_raw_image_requires_archive_name",
                    "starting-point raw image mutation requires image.output.archive_name".into(),
                    Some("image.output".into()),
                ));
            }
            if starting_point_looks_like_raw_image(starting_point)
                && !starting_point.image_read_only
                && starting_point.packages.enabled
                && starting_point.packages.execute
                && spec.image.output.archive_name.is_none()
            {
                diagnostics.push(error(
                    "starting_point_raw_image_requires_archive_name",
                    "starting-point raw image package execution requires image.output.archive_name"
                        .into(),
                    Some("image.output".into()),
                ));
            }
            if starting_point_looks_like_raw_image(starting_point)
                && starting_point.image_read_only
                && (!spec.image.feed.install_entries.is_empty()
                    || !spec.image.feed.stage_files.is_empty()
                    || !spec.image.feed.stage_env_sets.is_empty()
                    || !spec.image.feed.stage_services.is_empty())
            {
                diagnostics.push(error(
                    "starting_point_raw_image_read_only_overlay",
                    "starting-point raw image overlay requires image_read_only=false".into(),
                    Some("image".into()),
                ));
            }
            if starting_point_looks_like_raw_image(starting_point)
                && starting_point.image_read_only
                && starting_point.packages.enabled
                && starting_point.packages.execute
            {
                diagnostics.push(error(
                    "starting_point_raw_image_read_only_packages",
                    "starting-point raw image package execution requires image_read_only=false"
                        .into(),
                    Some("image".into()),
                ));
            }
        }
    }

    for install_id in &spec.image.feed.install_entries {
        if !spec
            .install
            .entries
            .iter()
            .any(|entry| entry.id == *install_id)
        {
            diagnostics.push(error(
                "unknown_image_feed_install",
                format!(
                    "image feed references unknown install entry '{}'",
                    install_id.as_str()
                ),
                Some("image.feed.install_entries".into()),
            ));
        }
    }
    for stage_id in &spec.image.feed.stage_files {
        if !spec.stage.files.iter().any(|file| file.id == *stage_id) {
            diagnostics.push(error(
                "unknown_image_feed_stage_file",
                format!(
                    "image feed references unknown stage file '{}'",
                    stage_id.as_str()
                ),
                Some("image.feed.stage_files".into()),
            ));
        }
    }
    for stage_id in &spec.image.feed.stage_env_sets {
        if !spec
            .stage
            .env_sets
            .iter()
            .any(|env_set| env_set.id == *stage_id)
        {
            diagnostics.push(error(
                "unknown_image_feed_stage_env_set",
                format!(
                    "image feed references unknown stage env-set '{}'",
                    stage_id.as_str()
                ),
                Some("image.feed.stage_env_sets".into()),
            ));
        }
    }
    for stage_id in &spec.image.feed.stage_services {
        if !spec
            .stage
            .services
            .iter()
            .any(|service| service.id == *stage_id)
        {
            diagnostics.push(error(
                "unknown_image_feed_stage_service",
                format!(
                    "image feed references unknown stage service '{}'",
                    stage_id.as_str()
                ),
                Some("image.feed.stage_services".into()),
            ));
        }
    }

    if spec.image.output.archive_name.is_some() && spec.image.output.collect_dir.is_none() {
        diagnostics.push(error(
            "image_archive_without_collect_dir",
            "image output archive_name requires collect_dir to also be set".into(),
            Some("image.output".into()),
        ));
    }

    validate_image_assembly(spec, diagnostics);
}

fn starting_point_looks_like_raw_image(starting_point: &StartingPointImageSpec) -> bool {
    fn rawish(value: &str) -> bool {
        let lowered = value.trim().to_ascii_lowercase();
        lowered.ends_with(".img") || lowered.ends_with(".raw")
    }

    rawish(&starting_point.rootfs_path) || starting_point.source_path.as_deref().is_some_and(rawish)
}

fn validate_buildroot_expected_images_against_defconfig(
    buildroot: &gaia_spec::BuildrootImageSpec,
    defconfig_contents: &str,
    assembly_expected_image_names: &HashSet<String>,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    for expected_image in &buildroot.expected_images {
        if assembly_expected_image_names.contains(&expected_image.name) {
            continue;
        }
        let supported = match expected_image.format {
            BuildrootExpectedImageFormatSpec::Tar => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_TAR=y")
            }
            BuildrootExpectedImageFormatSpec::Cpio => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_CPIO=y")
            }
            BuildrootExpectedImageFormatSpec::Ext2 => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2=y")
                    && (defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2_2r0=y")
                        || defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2_2r1=y")
                        || defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2_2=y"))
            }
            BuildrootExpectedImageFormatSpec::Ext3 => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2=y")
                    && defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2_3=y")
            }
            BuildrootExpectedImageFormatSpec::Ext4 => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2=y")
                    && defconfig_contents.contains("BR2_TARGET_ROOTFS_EXT2_4=y")
            }
            BuildrootExpectedImageFormatSpec::Ubifs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_UBIFS=y")
            }
            BuildrootExpectedImageFormatSpec::Ubi => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_UBI=y")
            }
            BuildrootExpectedImageFormatSpec::Jffs2 => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_JFFS2=y")
            }
            BuildrootExpectedImageFormatSpec::Romfs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_ROMFS=y")
            }
            BuildrootExpectedImageFormatSpec::Cramfs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_CRAMFS=y")
            }
            BuildrootExpectedImageFormatSpec::Cloop => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_CLOOP=y")
            }
            BuildrootExpectedImageFormatSpec::F2fs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_F2FS=y")
            }
            BuildrootExpectedImageFormatSpec::Btrfs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_BTRFS=y")
            }
            BuildrootExpectedImageFormatSpec::Squashfs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_SQUASHFS=y")
            }
            BuildrootExpectedImageFormatSpec::Kernel => {
                defconfig_contents.contains("BR2_LINUX_KERNEL=y")
            }
            BuildrootExpectedImageFormatSpec::Raw => {
                defconfig_contents.contains("BR2_PACKAGE_HOST_GENIMAGE=y")
                    || defconfig_contents.contains("BR2_ROOTFS_POST_IMAGE_SCRIPT")
            }
            BuildrootExpectedImageFormatSpec::Erofs => {
                defconfig_contents.contains("BR2_TARGET_ROOTFS_EROFS=y")
            }
        };
        if !supported {
            diagnostics.push(error(
                "buildroot_expected_image_format_not_enabled",
                format!(
                    "buildroot expected image '{}' requires format '{}' support that is not enabled in the selected defconfig",
                    expected_image.name,
                    expected_image.format.as_str()
                ),
                Some("image".into()),
            ));
        }
    }
    diagnostics
}

fn expected_image_name_matches_format(
    name: &str,
    format: BuildrootExpectedImageFormatSpec,
) -> bool {
    let name = name.trim().to_ascii_lowercase();
    match format {
        BuildrootExpectedImageFormatSpec::Tar => name.ends_with(".tar"),
        BuildrootExpectedImageFormatSpec::Cpio => {
            name.ends_with(".cpio")
                || name.ends_with(".cpio.gz")
                || name.ends_with(".cpio.xz")
                || name.ends_with(".cpio.zst")
                || name.ends_with(".cpio.lz4")
        }
        BuildrootExpectedImageFormatSpec::Ext2 => name.ends_with(".ext2"),
        BuildrootExpectedImageFormatSpec::Ext3 => name.ends_with(".ext3"),
        BuildrootExpectedImageFormatSpec::Ext4 => name.ends_with(".ext4"),
        BuildrootExpectedImageFormatSpec::Ubifs => name.ends_with(".ubifs"),
        BuildrootExpectedImageFormatSpec::Ubi => name.ends_with(".ubi"),
        BuildrootExpectedImageFormatSpec::Jffs2 => name.ends_with(".jffs2"),
        BuildrootExpectedImageFormatSpec::Romfs => name.ends_with(".romfs"),
        BuildrootExpectedImageFormatSpec::Cramfs => name.ends_with(".cramfs"),
        BuildrootExpectedImageFormatSpec::Cloop => name.ends_with(".cloop"),
        BuildrootExpectedImageFormatSpec::F2fs => name.ends_with(".f2fs"),
        BuildrootExpectedImageFormatSpec::Btrfs => name.ends_with(".btrfs"),
        BuildrootExpectedImageFormatSpec::Squashfs => name.ends_with(".squashfs"),
        BuildrootExpectedImageFormatSpec::Raw => name.ends_with(".img") || name.ends_with(".raw"),
        BuildrootExpectedImageFormatSpec::Kernel => {
            name == "image"
                || name.ends_with("/image")
                || name.ends_with("zimage")
                || name.ends_with("uimage")
                || name.ends_with("bzimage")
                || name.ends_with(".itb")
        }
        BuildrootExpectedImageFormatSpec::Erofs => name.ends_with(".erofs"),
    }
}
