use crate::{ImageAssemblySpec, InstallId, SourceId, StageItemId};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImageSpec {
    pub definition: ImageDefinition,
    pub feed: ImageFeedSpec,
    pub output: ImageOutputSpec,
    pub assembly: Option<ImageAssemblySpec>,
    /// `[image.signing]`: signatures written next to the published outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing: Option<ImageSigningSpec>,
    /// `[image.layers]`: the rootfs split into layer tars. Left out of
    /// serialized specs when unset, so existing builds keep their digests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<ImageLayersSpec>,
    /// `[image.checks]`: what `image:check` verifies in the built image. Left
    /// out of serialized specs when unset, so existing builds keep their digests.
    #[serde(skip_serializing_if = "ImageChecksSpec::is_default")]
    pub checks: ImageChecksSpec,
    /// `[[image.hardware]]`: the hardware the image must support, checked by
    /// `image:check`. Left out of serialized specs when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hardware: Vec<ImageHardwareSpec>,
}

/// `[[image.hardware]]`: one device the image must carry. Every list is as
/// written; the check reads module names with `-` and `_` the same.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImageHardwareSpec {
    pub id: String,
    /// A free-form label for the reports.
    pub kind: String,
    /// Module names the driver is in (built-in drivers too). Empty means any
    /// shipped or built-in driver may claim the compatibles.
    pub modules: Vec<String>,
    /// Device-tree compatibles the driver must claim.
    pub compatible: Vec<String>,
    /// `.dtbo` basenames that must ship in a boot overlays directory.
    pub overlays: Vec<String>,
    /// Absolute rootfs paths that must exist.
    pub files: Vec<String>,
    /// Firmware names that must exist under `/lib/firmware`, with or without
    /// a compression suffix.
    pub firmware: Vec<String>,
}

/// `[image.layers]`: the root filesystem split into one tar per layer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImageLayersSpec {
    /// Layer names, lowest first.
    pub order: Vec<String>,
    /// The layer that takes every path no selector claims.
    pub default: String,
    /// Non-directory paths that more than one layer selects, which ship from
    /// the upper layer (the lower copy is left out).
    pub overrides: Vec<String>,
    /// Whether each layer also gets an erofs image.
    pub erofs: bool,
    /// Whether each erofs image gets a dm-verity tree (needs `erofs`).
    pub verity: bool,
    /// The `mkfs.erofs` options of every layer's erofs (compression, cluster
    /// size, extended options), unless the layer sets its own.
    pub erofs_options: Vec<String>,
    /// The selectors of the layers that declare any (a layer may be empty).
    pub layers: Vec<ImageLayerSpec>,
}

/// One layer's selectors. Ids and package names are globbed (`*`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImageLayerSpec {
    pub name: String,
    /// Buildroot package names: every path the package installs.
    pub packages: Vec<String>,
    /// Absolute rootfs path prefixes.
    pub paths: Vec<String>,
    /// Install entry ids: each install's destination.
    pub installs: Vec<String>,
    /// Stage file ids: the paths each stage entry wrote.
    pub stage: Vec<String>,
    /// Stage service ids: each service's unit file.
    pub services: Vec<String>,
    /// This layer's `mkfs.erofs` options, replacing the layer set's.
    pub erofs_options: Option<Vec<String>>,
}

/// The `mkfs.erofs` options a layer's erofs is made with when nothing sets
/// them: LZMA level 6, 256 KiB physical clusters, packed tails, shared
/// fragments and deduplication (what the HeliOS Raze root used).
pub const DEFAULT_LAYER_EROFS_OPTIONS: [&str; 3] = [
    "-zlzma,level=6",
    "-C262144",
    "-Eall-fragments,dedupe,ztailpacking",
];

impl ImageLayersSpec {
    /// The `mkfs.erofs` options of the layer called `name`.
    pub fn erofs_options_for(&self, name: &str) -> &[String] {
        self.layers
            .iter()
            .find(|layer| layer.name == name)
            .and_then(|layer| layer.erofs_options.as_deref())
            .unwrap_or(&self.erofs_options)
    }
}

/// `[image.checks]`. Paths are as written: a relative one resolves against
/// the workspace root.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImageChecksSpec {
    /// Runs the kernel module checks. `None` takes the default, which is on
    /// for Buildroot images.
    pub kernel_modules: Option<bool>,
    /// The module tree (default `target/lib/modules/<kernel release>`).
    pub modules_dir: Option<String>,
    /// The kernel's `Module.symvers` (default `build/linux-*/Module.symvers`).
    pub symvers: Option<String>,
    /// The kernel image the release is read from (default `images/Image`).
    pub kernel_image: Option<String>,
    /// The Raspberry Pi model the boot config's `[model]` sections are read
    /// for, such as `cm5`. Unset, every model section applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi_model: Option<String>,
}

impl ImageChecksSpec {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[image.signing]`. `kind` is as written; [`ImageSigningKind::parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImageSigningSpec {
    pub kind: String,
    /// A path to the key, or `env:VAR` to read the path (or, for gpg, the key id) from `VAR`.
    pub key: String,
    /// Glob patterns over the published output file names.
    pub outputs: Vec<String>,
}

/// The signing tools `[image.signing] kind` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSigningKind {
    Minisign,
    Gpg,
    CosignBlob,
}

impl ImageSigningKind {
    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            "minisign" => Some(Self::Minisign),
            "gpg" => Some(Self::Gpg),
            "cosign-blob" => Some(Self::CosignBlob),
            _ => None,
        }
    }

    /// The program the kind runs.
    pub fn program(self) -> &'static str {
        match self {
            Self::Minisign => "minisign",
            Self::Gpg => "gpg",
            Self::CosignBlob => "cosign",
        }
    }

    /// The suffix of a signature file written next to its output.
    pub fn signature_suffix(self) -> &'static str {
        match self {
            Self::Minisign => ".minisig",
            Self::Gpg => ".asc",
            Self::CosignBlob => ".sig",
        }
    }
}

impl ImageSpec {
    pub fn new(definition: ImageDefinition) -> Self {
        Self {
            definition,
            feed: ImageFeedSpec::default(),
            output: ImageOutputSpec::default(),
            assembly: None,
            signing: None,
            layers: None,
            checks: ImageChecksSpec::default(),
            hardware: Vec::new(),
        }
    }

    pub fn provider_kind(&self) -> ImageProviderKind {
        self.definition.provider_kind()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ImageDefinition {
    Buildroot(BuildrootImageSpec),
    StartingPoint(StartingPointImageSpec),
    /// `kind = "none"`: no rootfs, kernel or image is built. The build is its
    /// sources, artifacts and `[image.assembly]` only (a package for a device
    /// OS, for example). There is no image provider.
    None,
}

impl ImageDefinition {
    pub fn provider_kind(&self) -> ImageProviderKind {
        match self {
            Self::Buildroot(_) => ImageProviderKind::Buildroot,
            Self::StartingPoint(_) => ImageProviderKind::StartingPoint,
            Self::None => ImageProviderKind::None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImageFeedSpec {
    pub install_entries: Vec<InstallId>,
    pub stage_files: Vec<StageItemId>,
    pub stage_env_sets: Vec<StageItemId>,
    pub stage_services: Vec<StageItemId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootImageSpec {
    pub source: Option<SourceId>,
    pub defconfig: Option<String>,
    pub defconfig_path: Option<String>,
    pub allow_fallback: bool,
    /// Run `make legal-info` after the image build and publish its archive
    /// (`legal-info.tar.zst`). Left out of serialized specs when off, so
    /// existing builds keep their reuse digests.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub legal_info: bool,
    pub config_fragments: Vec<String>,
    pub config_overrides: Vec<(String, String)>,
    pub external_tree: Option<String>,
    pub external_tree_mode: BuildrootExternalTreeModeSpec,
    pub expected_images: Vec<BuildrootExpectedImageSpec>,
    /// Buildroot packages whose files are left out of `target/` before the
    /// filesystem images are made (`[image.rootfs] exclude_packages`).
    pub rootfs_exclude_packages: Vec<String>,
}

impl BuildrootImageSpec {
    pub fn defconfig_path(&self) -> Option<&Path> {
        self.defconfig_path.as_deref().map(Path::new)
    }
}

impl Default for BuildrootImageSpec {
    fn default() -> Self {
        Self {
            source: None,
            defconfig: None,
            defconfig_path: None,
            allow_fallback: false,
            legal_info: false,
            config_fragments: Vec::new(),
            config_overrides: Vec::new(),
            external_tree: None,
            external_tree_mode: BuildrootExternalTreeModeSpec::Auto,
            expected_images: Vec::new(),
            rootfs_exclude_packages: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootExpectedImageSpec {
    pub name: String,
    pub format: BuildrootExpectedImageFormatSpec,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BuildrootExpectedImageFormatSpec {
    Tar,
    Cpio,
    Ext2,
    Ext3,
    Ext4,
    Ubifs,
    Ubi,
    Jffs2,
    Romfs,
    Cramfs,
    Cloop,
    F2fs,
    Btrfs,
    Squashfs,
    Raw,
    Kernel,
    Erofs,
    /// `rootfs.xfs`, Buildroot 2026.08+ (`BR2_TARGET_ROOTFS_XFS`).
    Xfs,
    /// A plain file with any name (`format = "file"`).
    File,
}

impl BuildrootExpectedImageFormatSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tar => "tar",
            Self::Cpio => "cpio",
            Self::Ext2 => "ext2",
            Self::Ext3 => "ext3",
            Self::Ext4 => "ext4",
            Self::Ubifs => "ubifs",
            Self::Ubi => "ubi",
            Self::Jffs2 => "jffs2",
            Self::Romfs => "romfs",
            Self::Cramfs => "cramfs",
            Self::Cloop => "cloop",
            Self::F2fs => "f2fs",
            Self::Btrfs => "btrfs",
            Self::Squashfs => "squashfs",
            Self::Raw => "raw",
            Self::Kernel => "kernel",
            Self::Erofs => "erofs",
            Self::Xfs => "xfs",
            Self::File => "file",
        }
    }
}

impl std::fmt::Display for BuildrootExpectedImageFormatSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImageOutputSpec {
    pub collect_dir: Option<String>,
    pub archive_name: Option<String>,
    /// The assembly disk (`[[image.assembly.disks]]` id) the raw disk archive
    /// `archive_name` is built from. Required when the assembly builds more
    /// than one disk.
    pub disk: Option<String>,
    pub emit_report: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ImageProviderKind {
    Buildroot,
    StartingPoint,
    /// No image provider: `[image] kind = "none"` builds no image.
    None,
}

impl ImageProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buildroot => "buildroot",
            Self::StartingPoint => "starting-point",
            Self::None => "none",
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
            "none" => Ok(Self::None),
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
        assert_eq!(
            "none".parse::<ImageProviderKind>(),
            Ok(ImageProviderKind::None)
        );
        assert!("unknown".parse::<ImageProviderKind>().is_err());
    }
}
