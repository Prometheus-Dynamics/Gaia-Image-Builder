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
    /// `[[image.assembly.oci]]`: OCI images built from layers.
    #[serde(rename = "oci")]
    pub ocis: Vec<RawAssemblyOciConfig>,
    pub busybox_initramfs: Vec<RawAssemblyBusyboxInitramfsConfig>,
    pub binaries: Vec<RawAssemblyBinaryConfig>,
    pub kernel_modules: Vec<RawAssemblyKernelModulesConfig>,
    pub devices: Vec<RawAssemblyDeviceConfig>,
    /// `[[image.assembly.checks]]`: commands run over the finished assembly.
    pub checks: Vec<RawAssemblyCheckConfig>,
    /// The `strip` program for `strip = true` entries: a program name
    /// (looked up like other assembly tools) or a path. Unset searches the
    /// provider's tools, then the host.
    pub strip_tool: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyTreeConfig {
    pub id: String,
    pub path: String,
    /// Strips every ELF file in the tree right before a filesystem or
    /// archive packs it.
    pub strip: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyDirConfig {
    pub tree: String,
    pub path: String,
    pub mode: Option<String>,
}

/// `[[image.assembly.devices]]`: a device node or FIFO added to the cpio
/// archive of `tree`, e.g. `path = "dev/console"`, `kind = "char"`,
/// `major = 5`, `minor = 1`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyDeviceConfig {
    pub tree: String,
    pub path: String,
    pub kind: RawAssemblyDeviceKind,
    /// `char` and `block` only: the major device number.
    pub major: Option<u32>,
    /// `char` and `block` only: the minor device number.
    pub minor: Option<u32>,
    /// Permission bits in octal, e.g. `"0600"` (the default).
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawAssemblyDeviceKind {
    #[default]
    Char,
    Block,
    Fifo,
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
    /// `src_glob` only: when set, only the matches whose file name one of
    /// these names (simple `*` globs allowed) matches are copied. An entry
    /// that matches nothing is an error unless `optional` is set.
    pub include: Vec<String>,
    /// Strips each copied ELF file (`src`, or each `src_glob` match).
    pub strip: bool,
    /// Text written to `dest` instead of copying a source.
    pub content: Option<String>,
    /// A JSON document written to `dest` instead of copying a source.
    pub json: Option<RawAssemblyJsonValue>,
    pub dest: String,
    pub mode: Option<String>,
    pub optional: bool,
    pub preserve_symlink: bool,
    /// A copied dynamically linked file whose libraries the tree lacks is
    /// accepted without the warning (for a tree that overlays a full rootfs).
    pub allow_missing_libs: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyTransformConfig {
    pub kind: RawAssemblyTransformKind,
    pub src: Option<String>,
    pub dest: String,
    pub deterministic: Option<bool>,
    /// Compression level: `gzip` 1-9, `zstd` 1-19, `xz` 0-9.
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
    /// `zstd` only: long-distance matching (2^27-byte window) for level 19
    /// and up over inputs above 128 MiB. Default false: the output decodes
    /// with a stock `zstd -d`.
    pub long: bool,
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
            long: false,
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
    /// zstd level for `cpio-zstd` (defaults to 19), xz level for `cpio-xz`
    /// (0-9, defaults to xz's own), or the level of a `squashfs` or `erofs`
    /// compressor that takes one.
    pub compression_level: Option<u32>,
    /// `squashfs` (gzip, xz, zstd, lz4; default gzip) or `erofs` (none, lz4,
    /// lz4hc, lzma, deflate; default lz4hc) only: the compressor.
    pub compression: Option<String>,
    /// `squashfs` and `erofs` only: block size in bytes.
    pub block_size: Option<u32>,
    /// `erofs` only: the largest physical cluster in bytes.
    pub pcluster_size: Option<u32>,
    /// `erofs` only: deduplicate compressed data.
    pub dedupe: bool,
    /// `erofs` only: pack file tails into shared fragments.
    pub fragments: bool,
    /// `erofs` only: inline compressed tails in their inodes.
    pub ztailpacking: bool,
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
    /// `vfat` only: the FAT type, 12, 16 or 32.
    pub fat_size: Option<u32>,
    /// `ext4` only: the percentage of blocks reserved for root (0-50). Unset
    /// is 0, the whole image usable.
    pub reserved_percent: Option<u32>,
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
            compression: None,
            block_size: None,
            pcluster_size: None,
            dedupe: false,
            fragments: false,
            ztailpacking: false,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            fat_size: None,
            reserved_percent: None,
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
    CpioXz,
    Ext4,
    Squashfs,
    Erofs,
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
    /// GPT only: the disk GUID; unset derives it from the id.
    pub disk_guid: Option<String>,
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
            disk_guid: None,
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
    /// GPT only: the partition GUID; unset derives it from the disk GUID
    /// and the name.
    pub guid: Option<String>,
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

/// `[[image.assembly.oci]]`: an OCI image (layout directory, or a
/// `docker load` tarball with `format = "docker-archive"`) built from
/// `layers`, lowest first, with the `[image.assembly.oci.config]` table.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyOciConfig {
    pub id: String,
    pub dest: String,
    /// `oci-layout` (the default) or `docker-archive`.
    pub format: Option<String>,
    pub layers: Vec<RawAssemblyOciLayerConfig>,
    /// `gzip` (the default), `zstd` or `none`.
    pub compression: Option<String>,
    /// The image's reference, e.g. `helios-sim:dev`.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub config: RawAssemblyOciImageConfig,
}

/// One layer: exactly one of `tar` and `tree`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyOciLayerConfig {
    /// An uncompressed layer tar, used as it is.
    pub tar: Option<String>,
    /// An assembly tree, tarred deterministically.
    pub tree: Option<String>,
}

/// `[image.assembly.oci.config]`: the image config.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyOciImageConfig {
    /// `amd64` or `arm64`; required.
    pub architecture: Option<String>,
    /// `linux` (the default).
    pub os: Option<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    /// `KEY=value` strings.
    pub env: Vec<String>,
    pub working_dir: Option<String>,
    pub stop_signal: Option<String>,
    pub labels: std::collections::BTreeMap<String, String>,
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
    /// Strips the copied program and every copied runtime library (the
    /// copies in the tree only).
    pub strip: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyKernelModulesConfig {
    pub tree: String,
    pub from: String,
    pub kernel_version: Option<String>,
    pub modules: Vec<String>,
    pub depmod: Option<String>,
    /// The `modules.*` metadata files to install (`modules.dep`,
    /// `modules.alias`, ...). Unset keeps the default set; set, only these
    /// are installed, and the other `modules.*` files depmod wrote are left out.
    pub metadata: Option<Vec<String>>,
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

/// `[[image.assembly.checks]]`: a command run after every assembly step; a
/// non-zero exit fails the build.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawAssemblyCheckConfig {
    pub id: String,
    /// The program and its arguments, run without a shell. Assembly tokens
    /// (`$assembly.tree.<id>`, `$assembly.work`, `$provider.images`, ...)
    /// are expanded in each argument.
    pub command: Vec<String>,
    /// Seconds the command may run before it is stopped and fails.
    pub timeout_secs: Option<u64>,
}
