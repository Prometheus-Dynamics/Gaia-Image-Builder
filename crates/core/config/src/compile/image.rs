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
        RawBuildrootExpectedImageFormat::Ext4 => BuildrootExpectedImageFormatSpec::Ext4,
        RawBuildrootExpectedImageFormat::Squashfs => BuildrootExpectedImageFormatSpec::Squashfs,
        RawBuildrootExpectedImageFormat::Raw => BuildrootExpectedImageFormatSpec::Raw,
        RawBuildrootExpectedImageFormat::Kernel => BuildrootExpectedImageFormatSpec::Kernel,
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
