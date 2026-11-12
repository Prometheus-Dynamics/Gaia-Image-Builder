//! `gaia.os.import`: an existing image as the OS a product adds to.
//!
//! The image is a stored artifact whose digest the lock pins: a raw disk
//! image (GPT or MBR), a bare filesystem image, or a root filesystem
//! archive (tar, plain or gzip, xz or zstd compressed). Nothing is mounted
//! and nothing runs as root: partition tables are read here, ext2/3/4 with
//! the `ext4-view` crate and FAT with `fatfs`, both in-process.
//!
//! Outputs, keyed on the image's digest and the two partition choices:
//!
//! - **rootfs**: the root filesystem's tree. Modes are kept in the store;
//!   paths not owned by root are listed in its entries.
//! - **boot**: the boot files' tree: a FAT partition (an EFI system or
//!   firmware partition), another partition, or the root's `/boot`.
//! - **owners**: `owners.json` with both trees' non-root owners and the
//!   device nodes, FIFOs and sockets the store cannot hold, for assembly
//!   to recreate.
//!
//! Partition choices: `auto`, a 1-based partition number, or `label:<GPT
//! name>`. For the root, `auto` is the largest ext partition. For boot,
//! `auto` is the first FAT partition, else the root's `/boot`; `rootfs`
//! is always the root's `/boot`, and `none` is the empty tree. An archive
//! has no partitions: its root is the archive, and its boot is `/boot`.
//!
//! Package installs into the imported tree are not done here: they are a
//! separate declared step. Nothing runs inside the image (no chroot).

mod archive;
mod disk;
mod ext;
mod fat;
mod out;

pub use out::{Owners, Special};

/// The libraries this node links, by the version Cargo resolved. Its step key
/// names them, and `tests/libs.rs` checks each version against Cargo.lock.
pub const LIBS: &[(&str, &str)] = &[
    ("tar", "0.4.46"),
    ("flate2", "1.1.11"),
    ("miniz_oxide", "0.9.1"),
    ("xz2", "0.1.7"),
    ("lzma-sys", "0.1.20"),
    ("zstd", "0.13.4"),
    ("zstd-safe", "7.4.0"),
    ("zstd-sys", "2.1.2+zstd.1.5.7"),
    ("ext4-view", "1.0.0"),
    ("fatfs", "0.3.6"),
];

use daedalus::prelude::*;
use disk::{Fs, Part, Slice};
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use out::Sink;
use std::fs::{self, File};
use std::io;
use std::path::Path;

const NAME: &str = "gaia.os.import";

/// The root filesystem and boot files of `image`, and their owners.
#[node(
    id = "gaia.os.import",
    inputs(store, image, root_part, boot_part),
    outputs(rootfs, boot, owners)
)]
pub fn os_import(
    store: &str,
    image: Artifact,
    root_part: &str,
    boot_part: &str,
    cx: &Ctx,
) -> Result<(Tree, Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let object = image
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(NAME, "an image that is not in the store"))?;
    let file = store.path(&object);
    let key = KeyBuilder::new(NAME, 1)
        .config("root", root_part)
        .config("boot", boot_part)
        .input("image", object.digest)
        .libs(LIBS)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("rootfs", Path::new("rootfs")),
            ("boot", Path::new("boot")),
            ("owners", Path::new("owners.json")),
        ],
        |work| {
            let owners = import(&file, root_part, boot_part, work.dir())
                .map_err(|e| StepError::body(NAME, format!("{}: {e}", object.digest)))?;
            let text = serde_json::to_vec_pretty(&owners).map_err(|e| StepError::body(NAME, e))?;
            fs::write(work.dir().join("owners.json"), text).map_err(|e| StepError::body(NAME, e))
        },
    )?;
    let owners_ref = made.outputs["owners"];
    let owners: Owners = serde_json::from_slice(
        &fs::read(store.path(&owners_ref)).map_err(|e| StepError::body(NAME, e))?,
    )
    .map_err(|e| StepError::body(NAME, e))?;
    Ok((
        Tree {
            object: made.outputs["rootfs"].into(),
            entries: owners.rootfs,
        },
        Tree {
            object: made.outputs["boot"].into(),
            entries: owners.boot,
        },
        Artifact {
            kind: ArtifactKind::parse("owners"),
            object: owners_ref.into(),
        },
    ))
}

/// Writes `rootfs/` and `boot/` under `out`.
fn import(file: &Path, root: &str, boot: &str, out: &Path) -> io::Result<Owners> {
    let rootfs_dir = out.join("rootfs");
    let mut rootfs = Sink::new(&rootfs_dir)?;
    if let Some(stream) = archive::open(file)? {
        if root != "auto" {
            return Err(io::Error::other(format!(
                "root {root:?}: an archive has no partitions; use auto"
            )));
        }
        archive::unpack(stream, &mut rootfs)?;
        let (owners, specials) = rootfs.finish()?;
        let boot_owners = match boot {
            "auto" | "rootfs" => boot_from_root(out, &owners)?,
            "none" => empty(out)?,
            other => {
                return Err(io::Error::other(format!(
                    "boot {other:?}: an archive has no partitions; use auto, rootfs or none"
                )));
            }
        };
        return Ok(Owners {
            rootfs: owners,
            boot: boot_owners,
            specials,
        });
    }
    let image = File::open(file)?;
    let parts = disk::partitions(&image)?;
    let list = || {
        parts
            .iter()
            .map(Part::describe)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let root_part = match root {
        "auto" => parts
            .iter()
            .filter(|p| p.fs == Fs::Ext)
            .max_by_key(|p| p.len)
            .ok_or_else(|| {
                io::Error::other(format!("no ext partition for the root; it has {}", list()))
            })?,
        choice => pick(&parts, choice).ok_or_else(|| {
            io::Error::other(format!(
                "root {choice:?}: no such partition; it has {}",
                list()
            ))
        })?,
    };
    if root_part.fs != Fs::Ext {
        return Err(io::Error::other(format!(
            "root: {} is not ext2/3/4",
            root_part.describe()
        )));
    }
    let fs = ext::load(Slice::new(image.try_clone()?, root_part))?;
    ext::copy(&fs, "/", &mut rootfs)?;
    let (owners, specials) = rootfs.finish()?;
    let boot_part = match boot {
        "none" | "rootfs" => None,
        "auto" => parts.iter().find(|p| p.fs == Fs::Fat),
        choice => Some(pick(&parts, choice).ok_or_else(|| {
            io::Error::other(format!(
                "boot {choice:?}: no such partition; it has {}",
                list()
            ))
        })?),
    };
    let boot_owners = match (boot, boot_part) {
        ("none", _) => empty(out)?,
        (_, None) => boot_from_root(out, &owners)?,
        (_, Some(part)) => {
            let boot_dir = out.join("boot");
            let mut sink = Sink::new(&boot_dir)?;
            match part.fs {
                Fs::Fat => fat::copy(Slice::new(image.try_clone()?, part), &mut sink)?,
                Fs::Ext => ext::copy(
                    &ext::load(Slice::new(image.try_clone()?, part))?,
                    "/",
                    &mut sink,
                )?,
                Fs::Other => {
                    return Err(io::Error::other(format!(
                        "boot: {} is neither FAT nor ext",
                        part.describe()
                    )));
                }
            }
            sink.finish()?.0
        }
    };
    Ok(Owners {
        rootfs: owners,
        boot: boot_owners,
        specials,
    })
}

/// `1`-based number or `label:<name>`.
fn pick<'a>(parts: &'a [Part], choice: &str) -> Option<&'a Part> {
    match choice.strip_prefix("label:") {
        Some(name) => parts.iter().find(|p| p.name == name),
        None => {
            let number: u32 = choice.parse().ok()?;
            parts.iter().find(|p| p.number == number)
        }
    }
}

fn empty(out: &Path) -> io::Result<Vec<gaia_types::FileEntry>> {
    fs::create_dir_all(out.join("boot"))?;
    Ok(Vec::new())
}

/// The root's `/boot`, copied as it is, with its owners.
fn boot_from_root(
    out: &Path,
    owners: &[gaia_types::FileEntry],
) -> io::Result<Vec<gaia_types::FileEntry>> {
    let from = out.join("rootfs/boot");
    let to = out.join("boot");
    if fs::symlink_metadata(&from).is_ok_and(|m| m.is_dir()) {
        copy_dir(&from, &to)?;
    } else {
        fs::create_dir_all(&to)?;
    }
    Ok(owners
        .iter()
        .filter_map(|e| {
            e.path
                .strip_prefix("boot/")
                .map(|rest| gaia_types::FileEntry {
                    path: rest.to_string(),
                    ..e.clone()
                })
        })
        .collect())
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&OS_IMPORT_NODE];
