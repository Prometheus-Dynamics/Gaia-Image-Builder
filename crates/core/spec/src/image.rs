use crate::{InstallId, SourceId, StageItemId};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSpec {
    pub definition: ImageDefinition,
    pub feed: ImageFeedSpec,
    pub output: ImageOutputSpec,
}

impl ImageSpec {
    pub fn new(definition: ImageDefinition) -> Self {
        Self {
            definition,
            feed: ImageFeedSpec::default(),
            output: ImageOutputSpec::default(),
        }
    }

    pub fn provider_kind(&self) -> ImageProviderKind {
        self.definition.provider_kind()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageDefinition {
    Buildroot(BuildrootImageSpec),
    StartingPoint(StartingPointImageSpec),
}

impl ImageDefinition {
    pub fn provider_kind(&self) -> ImageProviderKind {
        match self {
            Self::Buildroot(_) => ImageProviderKind::Buildroot,
            Self::StartingPoint(_) => ImageProviderKind::StartingPoint,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageFeedSpec {
    pub install_entries: Vec<InstallId>,
    pub stage_files: Vec<StageItemId>,
    pub stage_env_sets: Vec<StageItemId>,
    pub stage_services: Vec<StageItemId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildrootImageSpec {
    pub source: Option<SourceId>,
    pub defconfig: Option<String>,
    pub defconfig_path: Option<String>,
    pub allow_fallback: bool,
    pub config_fragments: Vec<String>,
    pub config_overrides: Vec<(String, String)>,
    pub external_tree: Option<String>,
    pub external_tree_mode: BuildrootExternalTreeModeSpec,
    pub expected_images: Vec<BuildrootExpectedImageSpec>,
}

impl BuildrootImageSpec {
    pub fn defconfig_path(&self) -> Option<&Path> {
        self.defconfig_path.as_deref().map(Path::new)
    }

    pub fn external_tree_path(&self) -> Option<&Path> {
        self.external_tree.as_deref().map(Path::new)
    }
}

impl Default for BuildrootImageSpec {
    fn default() -> Self {
        Self {
            source: None,
            defconfig: None,
            defconfig_path: None,
            allow_fallback: false,
            config_fragments: Vec::new(),
            config_overrides: Vec::new(),
            external_tree: None,
            external_tree_mode: BuildrootExternalTreeModeSpec::Auto,
            expected_images: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildrootExpectedImageSpec {
    pub name: String,
    pub format: BuildrootExpectedImageFormatSpec,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildrootExpectedImageFormatSpec {
    Tar,
    Ext4,
    Squashfs,
    Raw,
    Kernel,
}

impl BuildrootExpectedImageFormatSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tar => "tar",
            Self::Ext4 => "ext4",
            Self::Squashfs => "squashfs",
            Self::Raw => "raw",
            Self::Kernel => "kernel",
        }
    }
}

impl std::fmt::Display for BuildrootExpectedImageFormatSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildrootExternalTreeModeSpec {
    Auto,
    Required,
    Disabled,
}

impl BuildrootExternalTreeModeSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Required => "required",
            Self::Disabled => "disabled",
        }
    }
}

impl std::fmt::Display for BuildrootExternalTreeModeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartingPointImageSpec {
    pub source: Option<SourceId>,
    pub source_path: Option<String>,
    pub rootfs_path: String,
    pub image_partition: Option<String>,
    pub image_read_only: bool,
    pub packages: StartingPointPackagesSpec,
    pub rootfs_validation_mode: StartingPointRootfsValidationModeSpec,
    pub output_mode: StartingPointOutputModeSpec,
}

impl StartingPointImageSpec {
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref().map(Path::new)
    }

    pub fn rootfs_path(&self) -> &Path {
        Path::new(&self.rootfs_path)
    }
}

impl Default for StartingPointImageSpec {
    fn default() -> Self {
        Self {
            source: None,
            source_path: None,
            rootfs_path: String::new(),
            image_partition: None,
            image_read_only: true,
            packages: StartingPointPackagesSpec::default(),
            rootfs_validation_mode: StartingPointRootfsValidationModeSpec::RequireExists,
            output_mode: StartingPointOutputModeSpec::CopyRootfs,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartingPointPackagesSpec {
    pub enabled: bool,
    pub execute: bool,
    pub manager: Option<String>,
    pub release_version: Option<String>,
    pub allow_major_upgrade: bool,
    pub update: bool,
    pub dist_upgrade: bool,
    pub install: Vec<String>,
    pub remove: Vec<String>,
    pub extra_args: Vec<String>,
    pub os_release_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartingPointRootfsValidationModeSpec {
    RequireExists,
    RequireDirectory,
    RequireFile,
    AllowMissing,
}

impl StartingPointRootfsValidationModeSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequireExists => "require-exists",
            Self::RequireDirectory => "require-directory",
            Self::RequireFile => "require-file",
            Self::AllowMissing => "allow-missing",
        }
    }
}

impl std::fmt::Display for StartingPointRootfsValidationModeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartingPointOutputModeSpec {
    CopyRootfs,
    ArchiveOnly,
    CopyAndArchive,
}

impl StartingPointOutputModeSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CopyRootfs => "copy-rootfs",
            Self::ArchiveOnly => "archive-only",
            Self::CopyAndArchive => "copy-and-archive",
        }
    }
}

impl std::fmt::Display for StartingPointOutputModeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageOutputSpec {
    pub collect_dir: Option<String>,
    pub archive_name: Option<String>,
    pub emit_report: bool,
}

impl ImageOutputSpec {
    pub fn collect_dir_path(&self) -> Option<&Path> {
        self.collect_dir.as_deref().map(Path::new)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProviderKind {
    Buildroot,
    StartingPoint,
}

impl ImageProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buildroot => "buildroot",
            Self::StartingPoint => "starting-point",
        }
    }
}

impl std::fmt::Display for ImageProviderKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ImageProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "buildroot" => Ok(Self::Buildroot),
            "starting-point" => Ok(Self::StartingPoint),
            other => Err(format!("unknown image provider kind `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_provider_kind_parses_from_strings() {
        assert_eq!(
            "buildroot".parse::<ImageProviderKind>(),
            Ok(ImageProviderKind::Buildroot)
        );
        assert_eq!(
            "starting-point".parse::<ImageProviderKind>(),
            Ok(ImageProviderKind::StartingPoint)
        );
        assert!("unknown".parse::<ImageProviderKind>().is_err());
    }
}
