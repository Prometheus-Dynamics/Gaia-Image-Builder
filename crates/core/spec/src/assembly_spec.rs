use crate::{
    AssemblyArchiveSpec, AssemblyFilesystemId, AssemblyTreeId, ByteSize, ByteSizeParseError,
    ImageProviderKind,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageAssemblySpec {
    pub work_dir: Option<AssemblyPathTemplate>,
    pub out_dir: Option<AssemblyPathTemplate>,
    pub trees: Vec<AssemblyTreeSpec>,
    pub dirs: Vec<AssemblyDirSpec>,
    pub symlinks: Vec<AssemblySymlinkSpec>,
    pub files: Vec<AssemblyFileSpec>,
    pub transforms: Vec<AssemblyTransformSpec>,
    pub filesystems: Vec<AssemblyFilesystemSpec>,
    pub disks: Vec<AssemblyDiskSpec>,
    pub archives: Vec<AssemblyArchiveSpec>,
    pub busybox_initramfs: Vec<AssemblyBusyboxInitramfsSpec>,
    pub kernel_modules: Vec<AssemblyKernelModulesSpec>,
}

impl ImageAssemblySpec {
    pub fn is_empty(&self) -> bool {
        self.work_dir.is_none()
            && self.out_dir.is_none()
            && self.trees.is_empty()
            && self.dirs.is_empty()
            && self.symlinks.is_empty()
            && self.files.is_empty()
            && self.transforms.is_empty()
            && self.filesystems.is_empty()
            && self.disks.is_empty()
            && self.archives.is_empty()
            && self.busybox_initramfs.is_empty()
            && self.kernel_modules.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyTreeSpec {
    pub id: AssemblyTreeId,
    pub path: AssemblyPathTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyDirSpec {
    pub tree: AssemblyTreeId,
    pub path: String,
    pub mode: Option<String>,
}

impl AssemblyDirSpec {
    pub fn parsed_mode(&self) -> Result<Option<FileMode>, FileModeParseError> {
        self.mode.as_deref().map(FileMode::from_str).transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblySymlinkSpec {
    pub tree: AssemblyTreeId,
    pub path: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyFileSpec {
    pub tree: AssemblyTreeId,
    pub src: Option<AssemblyPathTemplate>,
    pub src_glob: Option<AssemblyPathTemplate>,
    pub dest: String,
    pub mode: Option<String>,
    pub optional: bool,
    pub preserve_symlink: bool,
}

impl AssemblyFileSpec {
    pub fn parsed_mode(&self) -> Result<Option<FileMode>, FileModeParseError> {
        self.mode.as_deref().map(FileMode::from_str).transpose()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(u32);

impl FileMode {
    pub fn bits(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileModeParseError {
    Empty,
    InvalidOctal(String),
    OutOfRange(u32),
}

impl std::fmt::Display for FileModeParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("file mode cannot be empty"),
            Self::InvalidOctal(value) => {
                write!(formatter, "file mode '{value}' must be an octal value")
            }
            Self::OutOfRange(value) => {
                write!(formatter, "file mode '{value:o}' must not exceed 07777")
            }
        }
    }
}

impl FromStr for FileMode {
    type Err = FileModeParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(FileModeParseError::Empty);
        }
        if !trimmed
            .chars()
            .all(|character| matches!(character, '0'..='7'))
        {
            return Err(FileModeParseError::InvalidOctal(raw.into()));
        }
        let parsed = u32::from_str_radix(trimmed, 8)
            .map_err(|_| FileModeParseError::InvalidOctal(raw.into()))?;
        if parsed > 0o7777 {
            return Err(FileModeParseError::OutOfRange(parsed));
        }
        Ok(Self(parsed))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyTransformSpec {
    pub kind: AssemblyTransformKindSpec,
    pub src: Option<AssemblyPathTemplate>,
    pub dest: AssemblyPathTemplate,
    pub deterministic: bool,
    /// Compression level for `zstd` (1-19); `None` uses the tool default.
    pub level: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyTransformKindSpec {
    CompileDts,
    Gzip,
    Zstd,
    Copy,
}

impl AssemblyTransformKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompileDts => "compile-dts",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Copy => "copy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyFilesystemSpec {
    pub id: AssemblyFilesystemId,
    pub kind: AssemblyFilesystemKindSpec,
    pub source_tree: AssemblyTreeId,
    pub output: AssemblyPathTemplate,
    /// Byte size, or `"auto"` for vfat (sized from the tree).
    pub size: Option<String>,
    pub deterministic: bool,
    /// zstd level for `cpio-zstd` (1-19); `None` uses the default.
    pub compression_level: Option<u32>,
    /// Also copy the image to `$assembly.out/<file name of output>`, the
    /// image output dir, where it is published as an image of the run.
    pub publish: bool,
}

impl AssemblyFilesystemSpec {
    pub fn parsed_size(&self) -> Result<Option<ByteSize>, ByteSizeParseError> {
        self.size.as_deref().map(ByteSize::from_str).transpose()
    }

    /// Where a published filesystem image is copied: the image output dir
    /// (`$assembly.out`) under the output's file name. `None` when the
    /// filesystem is not published or the output has no file name.
    pub fn publish_template(&self) -> Option<AssemblyPathTemplate> {
        if !self.publish {
            return None;
        }
        let name = std::path::Path::new(self.output.as_str())
            .file_name()?
            .to_str()?;
        Some(AssemblyPathTemplate::new(format!("$assembly.out/{name}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyFilesystemKindSpec {
    Vfat,
    Cpio,
    CpioGzip,
    CpioZstd,
}

impl AssemblyFilesystemKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vfat => "vfat",
            Self::Cpio => "cpio",
            Self::CpioGzip => "cpio-gzip",
            Self::CpioZstd => "cpio-zstd",
        }
    }
}

/// Modules copied from a kernel's module tree into a tree, with their
/// dependency closure, and `depmod` run over the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyKernelModulesSpec {
    pub tree: AssemblyTreeId,
    /// The directory holding the `<kernel version>` directories.
    pub from: AssemblyPathTemplate,
    /// Required when `from` holds more than one kernel version directory.
    pub kernel_version: Option<String>,
    /// Module names, `-` and `_` equivalent, with or without a `.ko` suffix.
    pub modules: Vec<String>,
    /// The `depmod` to run; `None` takes the provider's, then the host's.
    pub depmod: Option<AssemblyPathTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyDiskSpec {
    pub id: String,
    pub output: AssemblyPathTemplate,
    pub partition_table: AssemblyPartitionTableSpec,
    pub signature: Option<String>,
    pub signature_text: Option<String>,
    pub first_lba: Option<u64>,
    pub alignment_lba: Option<u64>,
    /// `None` keeps the full disk size. `LastData` ends the file after the
    /// last written byte, rounded up to the alignment; the partition table
    /// still describes every partition at its full size.
    pub truncate: Option<AssemblyDiskTruncateSpec>,
    /// Where the MBR extended partition's EBR chain goes. `Default` puts each
    /// EBR right before its logical partition; `Packed` writes the whole
    /// chain in the first sectors of the extended partition.
    pub ebr_placement: AssemblyEbrPlacementSpec,
    pub partitions: Vec<AssemblyDiskPartitionSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyEbrPlacementSpec {
    Default,
    Packed,
}

impl AssemblyEbrPlacementSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Packed => "packed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyDiskTruncateSpec {
    LastData,
}

impl AssemblyDiskTruncateSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastData => "last-data",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyPartitionTableSpec {
    Mbr,
    Gpt,
}

impl AssemblyPartitionTableSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mbr => "mbr",
            Self::Gpt => "gpt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyDiskPartitionSpec {
    pub name: String,
    pub kind: Option<String>,
    pub type_alias: Option<String>,
    pub bootable: bool,
    /// Image written at the partition start; `None` leaves the partition
    /// empty, which requires `size`.
    pub image: Option<AssemblyPathTemplate>,
    /// Fixed partition size; defaults to the image size.
    pub size: Option<String>,
    /// Zero the first MiB of an empty partition so stale filesystem
    /// signatures from a previous flash do not survive.
    pub wipe: bool,
    /// `false` keeps the partition in the table at its full size with
    /// nothing written: no image and no wipe. Requires `size`.
    pub materialize: bool,
}

impl AssemblyDiskPartitionSpec {
    pub fn parsed_size(&self) -> Result<Option<ByteSize>, ByteSizeParseError> {
        self.size.as_deref().map(ByteSize::from_str).transpose()
    }

    pub fn partition_type(&self) -> Result<MbrPartitionType, MbrPartitionTypeParseError> {
        if let Some(kind) = &self.kind {
            return MbrPartitionType::from_raw_hex(kind);
        }
        match self.type_alias.as_deref() {
            Some(alias) => MbrPartitionType::from_alias(alias),
            None => Ok(MbrPartitionType::Linux),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbrPartitionType {
    Fat32Lba,
    Linux,
    Raw(u8),
}

impl MbrPartitionType {
    pub fn byte(self) -> u8 {
        match self {
            Self::Fat32Lba => 0x0c,
            Self::Linux => 0x83,
            Self::Raw(value) => value,
        }
    }

    pub fn from_alias(raw: &str) -> Result<Self, MbrPartitionTypeParseError> {
        match raw {
            "fat32-lba" => Ok(Self::Fat32Lba),
            "linux" => Ok(Self::Linux),
            alias => Err(MbrPartitionTypeParseError::UnknownAlias(alias.into())),
        }
    }

    pub fn from_raw_hex(raw: &str) -> Result<Self, MbrPartitionTypeParseError> {
        let trimmed = raw.trim();
        let value = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        let parsed = u8::from_str_radix(value, 16)
            .map_err(|_| MbrPartitionTypeParseError::InvalidRaw(raw.into()))?;
        Ok(Self::Raw(parsed))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MbrPartitionTypeParseError {
    InvalidRaw(String),
    UnknownAlias(String),
}

impl std::fmt::Display for MbrPartitionTypeParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRaw(value) => {
                write!(formatter, "partition type '{value}' must be a raw hex byte")
            }
            Self::UnknownAlias(value) => {
                write!(formatter, "unknown partition type alias '{value}'")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyBusyboxInitramfsSpec {
    pub tree: AssemblyTreeId,
    pub busybox: AssemblyPathTemplate,
    pub include_runtime_libs: bool,
    /// Target root for the runtime libraries; `None` derives it from `busybox`.
    pub sysroot: Option<AssemblyPathTemplate>,
    pub applets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssemblyPathTemplate(String);

impl AssemblyPathTemplate {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate_tokens(
        &self,
        provider_kind: ImageProviderKind,
        mut tree_exists: impl FnMut(&str) -> bool,
    ) -> Result<(), AssemblyPathTemplateError> {
        for token in assembly_template_tokens(&self.0) {
            if let Some(variable) = token.strip_prefix("$provider.") {
                match variable {
                    "images" | "target" => {}
                    "buildroot_output" if provider_kind == ImageProviderKind::Buildroot => {}
                    "host" | "staging" if provider_kind == ImageProviderKind::Buildroot => {}
                    "buildroot_output" | "host" | "staging" => {
                        return Err(AssemblyPathTemplateError::UnavailableProviderVariable(
                            token.into(),
                        ));
                    }
                    _ => {
                        return Err(AssemblyPathTemplateError::UnknownProviderVariable(
                            token.into(),
                        ));
                    }
                }
            } else if let Some(variable) = token.strip_prefix("$assembly.") {
                match variable {
                    "work" | "out" => {}
                    variable => {
                        if let Some(id) = variable.strip_prefix("tree.") {
                            if id.is_empty() {
                                return Err(AssemblyPathTemplateError::EmptyTreeReference);
                            }
                            if !tree_exists(id) {
                                return Err(AssemblyPathTemplateError::UnknownTree(id.into()));
                            }
                        } else {
                            return Err(AssemblyPathTemplateError::UnknownAssemblyVariable(
                                token.into(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        context: &AssemblyPathTemplateContext<'_>,
    ) -> Result<String, AssemblyPathTemplateError> {
        let mut resolved = self.0.clone();
        for token in assembly_template_tokens(&self.0) {
            resolved = resolved.replace(token, &context.resolve_token(token)?);
        }
        Ok(resolved)
    }

    pub fn referenced_tree_ids(&self) -> Vec<&str> {
        assembly_template_tokens(&self.0)
            .into_iter()
            .filter_map(|token| token.strip_prefix("$assembly.tree."))
            .filter(|id| !id.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AssemblyPathTemplateContext<'a> {
    pub provider_kind: ImageProviderKind,
    pub provider_images: &'a Path,
    pub provider_buildroot_output: Option<&'a Path>,
    pub provider_target: &'a Path,
    pub provider_host: Option<&'a Path>,
    pub provider_staging: Option<&'a Path>,
    pub assembly_work: &'a Path,
    pub assembly_out: &'a Path,
    pub trees: &'a BTreeMap<AssemblyTreeId, PathBuf>,
}

impl AssemblyPathTemplateContext<'_> {
    fn resolve_token(&self, token: &str) -> Result<String, AssemblyPathTemplateError> {
        let path = match token {
            "$provider.images" => Some(self.provider_images),
            "$provider.buildroot_output" if self.provider_kind == ImageProviderKind::Buildroot => {
                self.provider_buildroot_output
            }
            "$provider.target" => Some(self.provider_target),
            "$provider.host" if self.provider_kind == ImageProviderKind::Buildroot => {
                self.provider_host
            }
            "$provider.staging" if self.provider_kind == ImageProviderKind::Buildroot => {
                self.provider_staging
            }
            "$assembly.work" => Some(self.assembly_work),
            "$assembly.out" => Some(self.assembly_out),
            _ => None,
        };
        if let Some(path) = path {
            return Ok(path.display().to_string());
        }
        if matches!(
            token,
            "$provider.buildroot_output" | "$provider.host" | "$provider.staging"
        ) {
            return Err(AssemblyPathTemplateError::UnavailableProviderVariable(
                token.into(),
            ));
        }
        if let Some(id) = token.strip_prefix("$assembly.tree.") {
            if id.is_empty() {
                return Err(AssemblyPathTemplateError::EmptyTreeReference);
            }
            return self
                .trees
                .get(id)
                .map(|path| path.display().to_string())
                .ok_or_else(|| AssemblyPathTemplateError::UnknownTree(id.into()));
        }
        if token.starts_with("$provider.") {
            Err(AssemblyPathTemplateError::UnknownProviderVariable(
                token.into(),
            ))
        } else {
            Err(AssemblyPathTemplateError::UnknownAssemblyVariable(
                token.into(),
            ))
        }
    }
}

impl AsRef<str> for AssemblyPathTemplate {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for AssemblyPathTemplate {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::fmt::Display for AssemblyPathTemplate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for AssemblyPathTemplate {
    fn from(raw: String) -> Self {
        Self::new(raw)
    }
}

impl From<&str> for AssemblyPathTemplate {
    fn from(raw: &str) -> Self {
        Self::new(raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyPathTemplateError {
    EmptyTreeReference,
    UnknownTree(String),
    UnknownAssemblyVariable(String),
    UnknownProviderVariable(String),
    UnavailableProviderVariable(String),
}

impl std::fmt::Display for AssemblyPathTemplateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTreeReference => {
                formatter.write_str("assembly path references '$assembly.tree.' without a tree id")
            }
            Self::UnknownTree(id) => {
                write!(formatter, "assembly path references unknown tree '{id}'")
            }
            Self::UnknownAssemblyVariable(variable) => {
                write!(
                    formatter,
                    "assembly path references unknown variable '{variable}'"
                )
            }
            Self::UnknownProviderVariable(variable) => {
                write!(
                    formatter,
                    "assembly path references unknown variable '{variable}'"
                )
            }
            Self::UnavailableProviderVariable(variable) => write!(
                formatter,
                "assembly path references provider variable '{variable}' that is not available for the active image provider"
            ),
        }
    }
}

fn assembly_template_tokens(raw: &str) -> Vec<&str> {
    let bytes = raw.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len()
            && matches!(
                bytes[index],
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-'
            )
        {
            index += 1;
        }
        if index > start + 1 {
            let token = &raw[start..index];
            if token.starts_with("$assembly.") || token.starts_with("$provider.") {
                tokens.push(token);
            }
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageProviderKind;

    #[test]
    fn file_mode_parses_octal_modes_including_zero() {
        assert_eq!("0000".parse::<FileMode>().expect("0000").bits(), 0o0000);
        assert_eq!("0644".parse::<FileMode>().expect("0644").bits(), 0o0644);
        assert!("0888".parse::<FileMode>().is_err());
        assert!("10000".parse::<FileMode>().is_err());
    }

    #[test]
    fn mbr_partition_type_parses_aliases_and_raw_hex() {
        assert_eq!(
            MbrPartitionType::from_alias("fat32-lba")
                .expect("fat32-lba")
                .byte(),
            0x0c
        );
        assert_eq!(
            MbrPartitionType::from_alias("linux").expect("linux").byte(),
            0x83
        );
        assert_eq!(
            MbrPartitionType::from_raw_hex("0x0C").expect("raw").byte(),
            0x0c
        );
        assert!(MbrPartitionType::from_alias("unknown").is_err());
        assert!(MbrPartitionType::from_raw_hex("0x100").is_err());
    }

    #[test]
    fn assembly_path_template_validates_known_tokens() {
        let template = AssemblyPathTemplate::new(
            "$provider.images/$provider.buildroot_output/$assembly.work/$assembly.out/$assembly.tree.boot/Image",
        );

        assert_eq!(
            template.validate_tokens(ImageProviderKind::Buildroot, |id| id == "boot"),
            Ok(())
        );
        let template = AssemblyPathTemplate::new(
            "$provider.images/$assembly.work/$assembly.out/$assembly.tree.boot/Image",
        );
        assert_eq!(
            template.validate_tokens(ImageProviderKind::StartingPoint, |id| id == "boot"),
            Ok(())
        );
    }

    #[test]
    fn assembly_path_template_rejects_unknown_and_unavailable_tokens() {
        assert_eq!(
            AssemblyPathTemplate::new("$assembly.tree.")
                .validate_tokens(ImageProviderKind::Buildroot, |_| true),
            Err(AssemblyPathTemplateError::EmptyTreeReference)
        );
        assert_eq!(
            AssemblyPathTemplate::new("$assembly.tree.root")
                .validate_tokens(ImageProviderKind::Buildroot, |_| false),
            Err(AssemblyPathTemplateError::UnknownTree("root".into()))
        );
        assert_eq!(
            AssemblyPathTemplate::new("$assembly.unknown")
                .validate_tokens(ImageProviderKind::Buildroot, |_| true),
            Err(AssemblyPathTemplateError::UnknownAssemblyVariable(
                "$assembly.unknown".into()
            ))
        );
        assert_eq!(
            AssemblyPathTemplate::new("$provider.host")
                .validate_tokens(ImageProviderKind::StartingPoint, |_| true),
            Err(AssemblyPathTemplateError::UnavailableProviderVariable(
                "$provider.host".into()
            ))
        );
        assert_eq!(
            AssemblyPathTemplate::new("$provider.buildroot_output")
                .validate_tokens(ImageProviderKind::StartingPoint, |_| true),
            Err(AssemblyPathTemplateError::UnavailableProviderVariable(
                "$provider.buildroot_output".into()
            ))
        );
        assert_eq!(
            AssemblyPathTemplate::new("$provider.sdk")
                .validate_tokens(ImageProviderKind::Buildroot, |_| true),
            Err(AssemblyPathTemplateError::UnknownProviderVariable(
                "$provider.sdk".into()
            ))
        );
    }

    #[test]
    fn assembly_path_template_resolves_known_tokens() {
        let mut trees = BTreeMap::new();
        trees.insert(
            AssemblyTreeId::from("boot"),
            PathBuf::from("/tmp/work/boot"),
        );
        let context = AssemblyPathTemplateContext {
            provider_kind: ImageProviderKind::Buildroot,
            provider_images: Path::new("/tmp/images"),
            provider_buildroot_output: Some(Path::new("/tmp/build/image/buildroot-output")),
            provider_target: Path::new("/tmp/images/buildroot-output/target"),
            provider_host: Some(Path::new("/tmp/images/buildroot-output/host")),
            provider_staging: Some(Path::new("/tmp/images/buildroot-output/staging")),
            assembly_work: Path::new("/tmp/work"),
            assembly_out: Path::new("/tmp/images"),
            trees: &trees,
        };

        assert_eq!(
            AssemblyPathTemplate::new(
                "$provider.images:$provider.buildroot_output:$provider.target:$provider.host:$provider.staging:$assembly.work:$assembly.out:$assembly.tree.boot/Image"
            )
            .resolve(&context)
            .expect("resolved"),
            "/tmp/images:/tmp/build/image/buildroot-output:/tmp/images/buildroot-output/target:/tmp/images/buildroot-output/host:/tmp/images/buildroot-output/staging:/tmp/work:/tmp/images:/tmp/work/boot/Image"
        );
    }

    #[test]
    fn assembly_path_template_resolution_rejects_unknown_and_unavailable_tokens() {
        let trees = BTreeMap::new();
        let context = AssemblyPathTemplateContext {
            provider_kind: ImageProviderKind::StartingPoint,
            provider_images: Path::new("/tmp/images"),
            provider_buildroot_output: None,
            provider_target: Path::new("/tmp/images/rootfs"),
            provider_host: None,
            provider_staging: None,
            assembly_work: Path::new("/tmp/work"),
            assembly_out: Path::new("/tmp/images"),
            trees: &trees,
        };

        assert_eq!(
            AssemblyPathTemplate::new("$provider.host").resolve(&context),
            Err(AssemblyPathTemplateError::UnavailableProviderVariable(
                "$provider.host".into()
            ))
        );
        assert_eq!(
            AssemblyPathTemplate::new("$assembly.tree.boot").resolve(&context),
            Err(AssemblyPathTemplateError::UnknownTree("boot".into()))
        );
    }

    #[test]
    fn assembly_path_template_exposes_referenced_tree_ids() {
        assert_eq!(
            AssemblyPathTemplate::new(
                "$assembly.tree.boot/Image:$assembly.tree.rootfs:$assembly.tree."
            )
            .referenced_tree_ids(),
            vec!["boot", "rootfs"]
        );
    }
}
