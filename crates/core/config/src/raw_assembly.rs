use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawImageAssemblyConfig {
    pub work_dir: Option<String>,
    pub out_dir: Option<String>,
    pub trees: Vec<RawAssemblyTreeConfig>,
    pub dirs: Vec<RawAssemblyDirConfig>,
    pub symlinks: Vec<RawAssemblySymlinkConfig>,
    pub files: Vec<RawAssemblyFileConfig>,
    pub transforms: Vec<RawAssemblyTransformConfig>,
    pub filesystems: Vec<RawAssemblyFilesystemConfig>,
    pub disks: Vec<RawAssemblyDiskConfig>,
    pub archives: Vec<RawAssemblyArchiveConfig>,
    pub busybox_initramfs: Vec<RawAssemblyBusyboxInitramfsConfig>,
    pub kernel_modules: Vec<RawAssemblyKernelModulesConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyTreeConfig {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyDirConfig {
    pub tree: String,
    pub path: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblySymlinkConfig {
    pub tree: String,
    pub path: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyFileConfig {
    pub tree: String,
    pub src: Option<String>,
    pub src_glob: Option<String>,
    pub dest: String,
    pub mode: Option<String>,
    pub optional: bool,
    pub preserve_symlink: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyTransformConfig {
    pub kind: RawAssemblyTransformKind,
    pub src: Option<String>,
    pub dest: String,
    pub deterministic: Option<bool>,
    pub level: Option<u32>,
}

impl Default for RawAssemblyTransformConfig {
    fn default() -> Self {
        Self {
            kind: RawAssemblyTransformKind::Copy,
            src: None,
            dest: String::new(),
            deterministic: None,
            level: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyTransformKind {
    CompileDts,
    Gzip,
    Zstd,
    Copy,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyFilesystemConfig {
    pub id: String,
    pub kind: RawAssemblyFilesystemKind,
    pub source_tree: String,
    pub output: String,
    pub size: Option<String>,
    pub deterministic: Option<bool>,
    /// zstd level for `cpio-zstd`; defaults to 19.
    pub compression_level: Option<u32>,
    /// Copy the image to the image output dir under its file name.
    pub publish: bool,
}

impl Default for RawAssemblyFilesystemConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: RawAssemblyFilesystemKind::Cpio,
            source_tree: String::new(),
            output: String::new(),
            size: None,
            deterministic: None,
            compression_level: None,
            publish: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyFilesystemKind {
    Vfat,
    Cpio,
    CpioGzip,
    CpioZstd,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyDiskConfig {
    pub id: String,
    pub output: String,
    pub partition_table: RawAssemblyPartitionTable,
    pub signature: Option<String>,
    pub signature_text: Option<String>,
    pub first_lba: Option<u64>,
    pub alignment_lba: Option<u64>,
    pub truncate: Option<RawAssemblyDiskTruncate>,
    pub ebr_placement: Option<RawAssemblyEbrPlacement>,
    pub partitions: Vec<RawAssemblyDiskPartitionConfig>,
}

impl Default for RawAssemblyDiskConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            output: String::new(),
            partition_table: RawAssemblyPartitionTable::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            truncate: None,
            ebr_placement: None,
            partitions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyDiskTruncate {
    LastData,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyEbrPlacement {
    Default,
    Packed,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyPartitionTable {
    Mbr,
    Gpt,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyDiskPartitionConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub type_alias: Option<String>,
    pub bootable: bool,
    pub image: Option<String>,
    pub size: Option<String>,
    pub wipe: bool,
    /// Defaults to true; `false` leaves the partition unwritten.
    pub materialize: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyArchiveConfig {
    pub id: String,
    pub output: String,
    /// Ordered members; each sets `src` (a file) or `entries` (generated).
    pub members: Vec<RawAssemblyArchiveMemberConfig>,
    /// Generated members, written before `members`.
    pub generated: Vec<RawAssemblyArchiveGeneratedConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyArchiveMemberConfig {
    pub name: String,
    pub src: Option<String>,
    pub entries: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyArchiveGeneratedConfig {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyKernelModulesConfig {
    pub tree: String,
    pub from: String,
    pub kernel_version: Option<String>,
    pub modules: Vec<String>,
    pub depmod: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyBusyboxInitramfsConfig {
    pub tree: String,
    pub busybox: String,
    pub include_runtime_libs: bool,
    pub applets: Vec<String>,
}
