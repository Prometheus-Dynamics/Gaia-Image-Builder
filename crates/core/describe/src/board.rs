//! The board file: what the hardware is and what it needs to boot.
//!
//! A board file may extend one family file, which has the same format and
//! extends nothing. The board's keys win: a table is merged key by key, and
//! any other value the board sets (a list too) replaces the family's. Paths
//! stay relative to the file they are written in.

use crate::common::{FileRef, Mode, RelPath, SizeText, Source, check_source, resolve_sources};
use crate::{Error, read};
use gaia_lock::Want;
use gaia_types::{Arch, Os, PartitionRole, Target};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A board, its family file merged in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Board {
    /// The board file.
    pub file: PathBuf,
    /// The family file it extends.
    pub family: Option<PathBuf>,
    pub name: String,
    pub description: Option<String>,
    pub target: Target,
    pub sources: BTreeMap<String, Source>,
    pub kernel: Option<Kernel>,
    pub bootloader: Option<Bootloader>,
    /// Files installed into the image as the board needs them (radio and
    /// GPU firmware).
    pub firmware: Vec<Firmware>,
    /// Partitions the board needs whatever the layout: the bootloader's raw
    /// area, its environment.
    pub partitions: Vec<PartitionNeed>,
    /// The disk image's partition table, for a board written as one disk.
    pub disk: Option<Disk>,
    pub flash: Option<Flash>,
}

/// The kernel: its source, configuration and device trees.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kernel {
    pub source: Option<String>,
    /// The in-tree defconfig the configuration starts from.
    pub defconfig: Option<String>,
    /// Config fragments applied on top, in order.
    pub fragments: Option<Vec<RelPath>>,
    /// The device tree the board boots, in-tree (`rockchip/rk3566-board`).
    pub device_tree: Option<String>,
    /// A directory of the board's own device tree sources, built beside the
    /// in-tree ones.
    pub device_tree_dir: Option<RelPath>,
    /// Device tree overlays built and installed.
    pub overlays: Option<Vec<String>>,
}

/// The bootloader: its source, configuration and the files it is built
/// with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootloader {
    pub source: Option<String>,
    pub defconfig: Option<String>,
    pub fragments: Option<Vec<RelPath>>,
    /// Binary files the build takes in, by what they are to it (`tpl`).
    #[serde(default)]
    pub blobs: BTreeMap<String, FileRef>,
    pub trusted_firmware: Option<TrustedFirmware>,
}

/// Trusted firmware built for the bootloader to carry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedFirmware {
    pub source: Option<String>,
    pub platform: Option<String>,
}

/// One firmware file put into the image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Firmware {
    pub from: FileRef,
    /// Where it goes in the image: an absolute path.
    pub to: String,
    pub mode: Option<Mode>,
}

/// The partition table of the board's disk image.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// `gpt` or `mbr`.
    pub scheme: Option<Scheme>,
    /// The first byte a partition may use. Unset: 1 MiB.
    pub first: Option<SizeText>,
    /// Partitions start on multiples of this. Unset: 1 MiB.
    pub alignment: Option<SizeText>,
    /// The disk's size. Unset: as large as its partitions.
    pub size: Option<SizeText>,
}

/// A partition table's scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Gpt,
    Mbr,
}

/// A partition the board needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionNeed {
    pub name: String,
    pub role: PartitionRole,
    /// Where it starts, when the boot ROM or bootloader needs it there.
    pub offset: Option<SizeText>,
    pub size: Option<SizeText>,
    /// The partition table's type: a GPT type GUID or an MBR type byte.
    #[serde(rename = "type")]
    pub table_type: Option<String>,
    /// Whether the partition table lists it. A raw bootloader area is not.
    #[serde(default = "yes")]
    pub in_table: bool,
    /// For an area outside the table: the file written there, by its name
    /// in the OS's bootloader output (`u-boot-rockchip.bin`).
    pub content: Option<String>,
}

fn yes() -> bool {
    true
}

/// How the board is written to. Gaia does not use these values; they are
/// data for the tool that flashes the board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flash {
    /// The ways it can be written (`sd-card`, `maskrom`, `usb-boot`).
    pub methods: Option<Vec<String>>,
    /// Files a method needs, by name.
    #[serde(default)]
    pub files: BTreeMap<String, FileRef>,
    /// The address the flashing tool loads its program at. TOML integers
    /// are signed, so a negative value is refused; hex (`0x80360000`) reads.
    #[serde(default, deserialize_with = "non_negative")]
    pub load_address: Option<u64>,
    /// The medium the image is written to.
    pub medium: Option<Medium>,
}

/// The medium a board's image is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Medium {
    Sd,
    Emmc,
    SpiNand,
    SpiNor,
}

/// A whole number of 0 or more, read from a TOML integer.
fn non_negative<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    match Option::<i64>::deserialize(deserializer)? {
        None => Ok(None),
        Some(value) => u64::try_from(value).map(Some).map_err(|_| {
            de::Error::custom(format!("`{value}` is negative; an address is 0 or more"))
        }),
    }
}

/// One file as written: a board or a family.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardFile {
    #[allow(dead_code)]
    version: i64,
    extends: Option<RelPath>,
    name: Option<String>,
    description: Option<String>,
    target: Option<TargetText>,
    #[serde(default)]
    sources: BTreeMap<String, Source>,
    kernel: Option<Kernel>,
    bootloader: Option<Bootloader>,
    firmware: Option<Vec<Firmware>>,
    partitions: Option<Vec<PartitionNeed>>,
    disk: Option<Disk>,
    flash: Option<Flash>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetText {
    arch: Option<Arch>,
    cpu: Option<String>,
    os: Option<Os>,
}

impl Board {
    /// Reads the board file at `path`, and the family file it extends.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let board = read_file(path)?;
        let (family, family_path) = match &board.extends {
            None => (None, None),
            Some(extends) => {
                let family_path = extends.0.clone();
                let family = read_file(&family_path)?;
                if family.extends.is_some() {
                    return Err(Error::new(
                        &family_path,
                        "extends",
                        "a family file extends nothing; only a board extends, once",
                    ));
                }
                (Some(family), Some(family_path))
            }
        };
        let mut sources = family
            .as_ref()
            .map(|f| f.sources.clone())
            .unwrap_or_default();
        sources.extend(board.sources.clone());
        if let (Some(family), Some(family_path)) = (&family, &family_path) {
            check_refs(family_path, family, &sources)?;
        }
        check_refs(path, &board, &sources)?;
        let merged = match family {
            Some(family) => merge(board, family),
            None => board,
        };
        let name = merged
            .name
            .ok_or_else(|| Error::new(path, "name", "missing"))?;
        let target = merged.target.unwrap_or_default();
        let target = Target {
            arch: target
                .arch
                .ok_or_else(|| Error::new(path, "target.arch", "missing"))?,
            cpu: target.cpu,
            os: target.os.unwrap_or(Os::Linux),
        };
        Ok(Self {
            file: path.to_path_buf(),
            family: family_path,
            name,
            description: merged.description,
            target,
            sources,
            kernel: merged.kernel,
            bootloader: merged.bootloader,
            firmware: merged.firmware.unwrap_or_default(),
            partitions: merged.partitions.unwrap_or_default(),
            disk: match merged.disk {
                Some(disk) if disk.scheme.is_none() => {
                    return Err(Error::new(path, "disk.scheme", "missing: `gpt` or `mbr`"));
                }
                disk => disk,
            },
            flash: merged.flash,
        })
    }

    /// What the lock must hold for the board's sources, named
    /// `<board>.<source>`.
    pub fn wants(&self) -> Vec<(String, Want)> {
        self.sources
            .iter()
            .map(|(name, source)| (format!("{}.{name}", self.name), source.want()))
            .collect()
    }
}

/// Checks a family file alone: its keys, that it extends nothing, and the
/// sources it names itself. A board that extends it is checked with it.
pub(crate) fn check_family(path: &Path) -> Result<(), Error> {
    let family = read_file(path)?;
    if family.extends.is_some() {
        return Err(Error::new(
            path,
            "extends",
            "a family file extends nothing; only a board extends, once",
        ));
    }
    check_refs(path, &family, &family.sources)
}

/// Reads one file and resolves its paths against its directory.
fn read_file(path: &Path) -> Result<BoardFile, Error> {
    let mut file: BoardFile = read::read(path)?;
    let dir = path.parent().unwrap_or(Path::new(""));
    let each = |paths: &mut Option<Vec<RelPath>>| {
        for path in paths.iter_mut().flatten() {
            path.resolve(dir);
        }
    };
    if let Some(extends) = &mut file.extends {
        extends.resolve(dir);
    }
    resolve_sources(&mut file.sources, dir);
    if let Some(kernel) = &mut file.kernel {
        each(&mut kernel.fragments);
        if let Some(path) = &mut kernel.device_tree_dir {
            path.resolve(dir);
        }
    }
    if let Some(bootloader) = &mut file.bootloader {
        each(&mut bootloader.fragments);
        for blob in bootloader.blobs.values_mut() {
            blob.resolve(dir);
        }
    }
    for firmware in file.firmware.iter_mut().flatten() {
        firmware.from.resolve(dir);
    }
    if let Some(flash) = &mut file.flash {
        for file in flash.files.values_mut() {
            file.resolve(dir);
        }
    }
    let mut local: Vec<(String, &Path)> = Vec::new();
    if let Some(kernel) = &file.kernel {
        for (index, fragment) in kernel.fragments.iter().flatten().enumerate() {
            local.push((format!("kernel.fragments[{index}]"), fragment.path()));
        }
        if let Some(dir) = &kernel.device_tree_dir {
            local.push(("kernel.device_tree_dir".into(), dir.path()));
        }
    }
    if let Some(bootloader) = &file.bootloader {
        for (index, fragment) in bootloader.fragments.iter().flatten().enumerate() {
            local.push((format!("bootloader.fragments[{index}]"), fragment.path()));
        }
        for (name, blob) in &bootloader.blobs {
            if let FileRef::Local { file } = blob {
                local.push((format!("bootloader.blobs.{name}.file"), file.path()));
            }
        }
    }
    for (index, firmware) in file.firmware.iter().flatten().enumerate() {
        if let FileRef::Local { file } = &firmware.from {
            local.push((format!("firmware[{index}].from.file"), file.path()));
        }
    }
    for (name, file_ref) in file.flash.iter().flat_map(|flash| &flash.files) {
        if let FileRef::Local { file } = file_ref {
            local.push((format!("flash.files.{name}.file"), file.path()));
        }
    }
    for (key, local) in local {
        crate::common::check_exists(path, &key, local)?;
    }
    Ok(file)
}

/// Every source name `file` uses is one of `sources`, and every path it
/// installs to is absolute.
fn check_refs(
    path: &Path,
    file: &BoardFile,
    sources: &BTreeMap<String, Source>,
) -> Result<(), Error> {
    let check = |key: &str, name: &str| check_source(path, key, name, sources.keys());
    let check_ref = |key: String, file_ref: &FileRef| match file_ref.source() {
        Some(name) => check(&format!("{key}.source"), name),
        None => Ok(()),
    };
    if let Some(name) = file.kernel.as_ref().and_then(|k| k.source.as_deref()) {
        check("kernel.source", name)?;
    }
    if let Some(bootloader) = &file.bootloader {
        if let Some(name) = &bootloader.source {
            check("bootloader.source", name)?;
        }
        if let Some(name) = bootloader
            .trusted_firmware
            .as_ref()
            .and_then(|t| t.source.as_deref())
        {
            check("bootloader.trusted_firmware.source", name)?;
        }
        for (blob, file_ref) in &bootloader.blobs {
            check_ref(format!("bootloader.blobs.{blob}"), file_ref)?;
        }
    }
    for (index, need) in file.partitions.iter().flatten().enumerate() {
        if need.content.is_none() {
            continue;
        }
        if need.in_table {
            return Err(Error::new(
                path,
                format!("partitions[{index}].content"),
                "only an area outside the table (`in_table = false`) names its content; a partition's content comes from the recipe",
            ));
        }
        if need.offset.is_none() {
            return Err(Error::new(
                path,
                format!("partitions[{index}].offset"),
                "missing: an area outside the table is written at its offset",
            ));
        }
    }
    for (index, firmware) in file.firmware.iter().flatten().enumerate() {
        check_ref(format!("firmware[{index}].from"), &firmware.from)?;
        if !firmware.to.starts_with('/') {
            return Err(Error::new(
                path,
                format!("firmware[{index}].to"),
                "a path in the image is absolute",
            ));
        }
    }
    if let Some(flash) = &file.flash {
        for (name, file_ref) in &flash.files {
            check_ref(format!("flash.files.{name}"), file_ref)?;
        }
    }
    Ok(())
}

/// `board` over `family`.
fn merge(board: BoardFile, family: BoardFile) -> BoardFile {
    BoardFile {
        version: board.version,
        extends: board.extends,
        name: board.name.or(family.name),
        description: board.description.or(family.description),
        target: match (board.target, family.target) {
            (Some(b), Some(f)) => Some(TargetText {
                arch: b.arch.or(f.arch),
                cpu: b.cpu.or(f.cpu),
                os: b.os.or(f.os),
            }),
            (b, f) => b.or(f),
        },
        sources: {
            let mut sources = family.sources;
            sources.extend(board.sources);
            sources
        },
        kernel: match (board.kernel, family.kernel) {
            (Some(b), Some(f)) => Some(Kernel {
                source: b.source.or(f.source),
                defconfig: b.defconfig.or(f.defconfig),
                fragments: b.fragments.or(f.fragments),
                device_tree: b.device_tree.or(f.device_tree),
                device_tree_dir: b.device_tree_dir.or(f.device_tree_dir),
                overlays: b.overlays.or(f.overlays),
            }),
            (b, f) => b.or(f),
        },
        bootloader: match (board.bootloader, family.bootloader) {
            (Some(b), Some(f)) => Some(Bootloader {
                source: b.source.or(f.source),
                defconfig: b.defconfig.or(f.defconfig),
                fragments: b.fragments.or(f.fragments),
                blobs: {
                    let mut blobs = f.blobs;
                    blobs.extend(b.blobs);
                    blobs
                },
                trusted_firmware: match (b.trusted_firmware, f.trusted_firmware) {
                    (Some(b), Some(f)) => Some(TrustedFirmware {
                        source: b.source.or(f.source),
                        platform: b.platform.or(f.platform),
                    }),
                    (b, f) => b.or(f),
                },
            }),
            (b, f) => b.or(f),
        },
        firmware: board.firmware.or(family.firmware),
        partitions: board.partitions.or(family.partitions),
        disk: match (board.disk, family.disk) {
            (Some(b), Some(f)) => Some(Disk {
                scheme: b.scheme.or(f.scheme),
                first: b.first.or(f.first),
                alignment: b.alignment.or(f.alignment),
                size: b.size.or(f.size),
            }),
            (b, f) => b.or(f),
        },
        flash: match (board.flash, family.flash) {
            (Some(b), Some(f)) => Some(Flash {
                methods: b.methods.or(f.methods),
                files: {
                    let mut files = f.files;
                    files.extend(b.files);
                    files
                },
                load_address: b.load_address.or(f.load_address),
                medium: b.medium.or(f.medium),
            }),
            (b, f) => b.or(f),
        },
    }
}
