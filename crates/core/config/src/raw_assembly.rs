use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;

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
    pub binaries: Vec<RawAssemblyBinaryConfig>,
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
    /// `src_glob` only: file names (simple `*` globs allowed) left out of the match set.
    pub exclude: Vec<String>,
    /// Text written to `dest` instead of copying a source.
    pub content: Option<String>,
    /// A JSON document written to `dest` instead of copying a source.
    pub json: Option<RawAssemblyJsonValue>,
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
    /// Compression level: `zstd` 1-19, `xz` 0-9.
    pub level: Option<u32>,
    /// `verity` only: the hash tree's own file; unset appends it to `dest`.
    pub hash_dest: Option<String>,
    /// `verity` only: data block size in bytes, default 4096.
    pub data_block_size: Option<u32>,
    /// `verity` only: hash block size in bytes, default 4096.
    pub hash_block_size: Option<u32>,
    /// `verity` only: hex salt; unset derives it from the data image.
    pub salt: Option<String>,
    /// `verity` only: superblock UUID; unset derives it from the data image.
    pub uuid: Option<String>,
}

impl Default for RawAssemblyTransformConfig {
    fn default() -> Self {
        Self {
            kind: RawAssemblyTransformKind::Copy,
            src: None,
            dest: String::new(),
            deterministic: None,
            level: None,
            hash_dest: None,
            data_block_size: None,
            hash_block_size: None,
            salt: None,
            uuid: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyTransformKind {
    CompileDts,
    Gzip,
    Zstd,
    Xz,
    Copy,
    Verity,
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
    /// `ext4` only: the volume label, at most 16 bytes.
    pub label: Option<String>,
    /// `ext4` only: the filesystem UUID; unset derives it from the id.
    pub uuid: Option<String>,
    /// `ext4` only: create a journal, the default.
    pub journal: Option<bool>,
    /// With `size = "auto"` only: free space to leave beyond the content
    /// estimate (a byte size). The auto margin is used when larger.
    pub free: Option<String>,
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
            label: None,
            uuid: None,
            journal: None,
            free: None,
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
    Ext4,
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
    pub json: Option<RawAssemblyJsonValue>,
    /// Every entry under this assembly tree's root, stored under `prefix`.
    pub tree: Option<String>,
    /// Every entry under this directory, stored under `prefix`.
    pub src_dir: Option<String>,
    /// Each file the pattern matches, stored under `name` (a prefix ending in `/`).
    pub src_glob: Option<String>,
    /// `tree` and `src_dir`: archive path prefix, empty or ending in `/`.
    pub prefix: String,
    /// `tree` and `src_glob`: file names (simple `*` globs allowed) left out.
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyArchiveGeneratedConfig {
    pub name: String,
    /// `KEY = "value"` lines; unset with `json` unset writes an empty file.
    pub entries: Option<Vec<(String, String)>>,
    pub json: Option<RawAssemblyJsonValue>,
}

/// A JSON value as written in a `json = { ... }` table. Objects keep their
/// keys in the order the file wrote them (the `toml` crate's `preserve_order`
/// feature). Floats are rejected; a datetime is taken as its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawAssemblyJsonValue {
    String(String),
    Integer(i64),
    Bool(bool),
    Array(Vec<RawAssemblyJsonValue>),
    Object(Vec<(String, RawAssemblyJsonValue)>),
}

impl<'de> Deserialize<'de> for RawAssemblyJsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = RawAssemblyJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string, integer, boolean, array or table")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(RawAssemblyJsonValue::String(value.to_string()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(RawAssemblyJsonValue::String(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(RawAssemblyJsonValue::Integer(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        i64::try_from(value)
            .map(RawAssemblyJsonValue::Integer)
            .map_err(|_| E::custom(format!("integer {value} is too large for a JSON value")))
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(RawAssemblyJsonValue::Bool(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<RawAssemblyJsonValue>()? {
            items.push(item);
        }
        Ok(RawAssemblyJsonValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members: Vec<(String, RawAssemblyJsonValue)> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if members.iter().any(|(existing, _)| *existing == key) {
                return Err(de::Error::custom(format!("duplicate json key '{key}'")));
            }
            let value = map.next_value::<RawAssemblyJsonValue>()?;
            members.push((key, value));
        }
        Ok(RawAssemblyJsonValue::Object(members))
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyBinaryConfig {
    pub tree: String,
    pub src: String,
    pub dest: String,
    /// Root for the runtime libraries; derived from `src` when unset.
    pub sysroot: Option<String>,
    /// Octal file mode; `0755` when unset.
    pub mode: Option<String>,
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
    /// Root for the runtime libraries; derived from `busybox` when unset.
    pub sysroot: Option<String>,
    pub applets: Vec<String>,
}
