use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawImageAssemblyConfig {
    pub work_dir: Option<String>,
    pub out_dir: Option<String>,
    pub trees: Vec<RawAssemblyTreeConfig>,
    pub files: Vec<RawAssemblyFileConfig>,
    pub transforms: Vec<RawAssemblyTransformConfig>,
    pub filesystems: Vec<RawAssemblyFilesystemConfig>,
    pub disks: Vec<RawAssemblyDiskConfig>,
    pub busybox_initramfs: Vec<RawAssemblyBusyboxInitramfsConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyTreeConfig {
    pub id: String,
    pub path: String,
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
}

impl Default for RawAssemblyTransformConfig {
    fn default() -> Self {
        Self {
            kind: RawAssemblyTransformKind::Copy,
            src: None,
            dest: String::new(),
            deterministic: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyTransformKind {
    CompileDts,
    Gzip,
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
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyFilesystemKind {
    Vfat,
    Cpio,
    CpioGzip,
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
            partitions: Vec::new(),
        }
    }
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
    pub image: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyBusyboxInitramfsConfig {
    pub tree: String,
    pub busybox: String,
    pub include_runtime_libs: bool,
    pub applets: Vec<String>,
}
