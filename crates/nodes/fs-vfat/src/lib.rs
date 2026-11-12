//! `gaia.fs.vfat`: a stored tree as a FAT image, for boot partitions.
//!
//! `mformat` makes an empty FAT volume of the image's size and `mcopy`
//! copies the tree in. Both run with `TZ=UTC`, and `mcopy -m` keeps each
//! entry's modification time, which is set first to the FAT epoch
//! (1980-01-01 UTC, the earliest time FAT stores), so the image does not
//! depend on when or where the tree was built. The volume serial is four
//! bytes of the SHA-256 of the tree's digest, so the same tree gives the same
//! bytes on every host.
//!
//! FAT stores no owners and no modes, so the key is the tree's content
//! digest alone: an owner change in [`gaia_types::Tree::entries`] does not
//! remake the image.
//!
//! The size is either fixed, or the content (each file and each directory
//! rounded up to a 4 KiB cluster, plus 1 MiB of FAT and root directory),
//! plus a margin of 5% of the content or 1 MiB, whichever is more, rounded up
//! to 1 MiB. `mformat` and `mcopy` are pinned by path; their versions are in
//! the key.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool, Work, once_per_process};
use gaia_store::Digest;
use gaia_types::{Image, ImageKind, Size, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.fs.vfat";
const SECTOR: u64 = 512;
const MIB: u64 = 1024 * 1024;
/// The cluster each file and directory is counted at in an automatic size.
const CLUSTER: u64 = 4 * 1024;
/// The boot sector, the FAT tables and the root directory.
const FIXED_OVERHEAD: u64 = MIB;
/// The least free space an automatic size leaves.
const MIN_MARGIN: u64 = MIB;
/// 1980-01-01 00:00:00 UTC, the earliest time FAT stores.
const FAT_EPOCH_SECS: u64 = 315_532_800;
/// Images from 16 MiB up get FAT16 or FAT32 from `mformat`'s own choice.
const FAT16_DEFAULT_FROM: u64 = 16 * MIB;
/// The smallest image where one sector per cluster still gives FAT16's
/// minimum of 4085 clusters.
const FAT16_MIN: u64 = 3 * MIB;

/// The FAT volume of `tree` in an image of `size` bytes (0 for a size from
/// the content). `fat_size` is 12, 16 or 32 to ask for that FAT type, or 0
/// for `mformat`'s choice; an image that cannot hold the asked type fails.
#[node(
    id = "gaia.fs.vfat",
    inputs(store, mformat, mcopy, tree, size, fat_size),
    outputs(image)
)]
pub fn fs_vfat(
    store: &str,
    mformat: &str,
    mcopy: &str,
    tree: Tree,
    size: u64,
    fat_size: u32,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
    if !matches!(fat_size, 0 | 12 | 16 | 32) {
        return Err(fail(format!("fat_size {fat_size} is not 12, 16 or 32")));
    }
    let digest: Digest = tree
        .object
        .object_ref()
        .ok_or_else(|| fail("a tree that is not in the store".into()))?
        .digest;
    let dir = tree
        .object
        .path_in(&store)
        .ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let bytes = if size == 0 {
        auto_bytes(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?
    } else {
        size
    };
    if bytes == 0 || bytes % SECTOR != 0 {
        return Err(fail(format!(
            "{bytes} bytes is not a whole number of 512-byte sectors"
        )));
    }
    let version = |program: &str| {
        once_per_process(Path::new(program), || {
            let out = std::process::Command::new(program)
                .arg("--version")
                .env_clear()
                .output()?;
            Ok([out.stdout, out.stderr].concat())
        })
        .map_err(|e| fail(format!("{program} --version: {e}")))
    };
    let key = KeyBuilder::new(NAME, 1)
        .config("mformat", version(mformat)?)
        .config("mcopy", version(mcopy)?)
        .config("bytes", bytes.to_string())
        .config("fat_size", fat_size.to_string())
        .input("tree", digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("image", Path::new("image.vfat"))],
        |work| {
            let image = work.dir().join("image.vfat");
            let file = std::fs::File::create(&image).map_err(|e| fail(e.to_string()))?;
            file.set_len(bytes).map_err(|e| fail(e.to_string()))?;
            drop(file);
            let serial = volume_serial(&digest);
            work.run(
                &Tool::new(mformat)
                    .env("TZ", "UTC")
                    .args(mformat_args(bytes, fat_size))
                    .arg("-N")
                    .arg(&serial)
                    .arg("-i")
                    .arg(&image)
                    .arg("::"),
            )?;
            check_fat_size(&image, bytes, fat_size).map_err(fail)?;

            let staging = work.dir().join("tree.staged");
            let copied = stage(&dir, &staging)
                .map_err(fail)
                .and_then(|()| copy_in(work, mcopy, &staging, &image));
            let _ = std::fs::remove_dir_all(&staging);
            copied
        },
    )?;
    let object = made.outputs["image"];
    let len = std::fs::metadata(store.path(&object))
        .map_err(|e| fail(e.to_string()))?
        .len();
    Ok(Image {
        kind: ImageKind::Filesystem,
        object: object.into(),
        size: Size(len),
        partitions: Vec::new(),
    })
}

/// The `mformat` options for an image of `bytes` asked for `fat_size`. Below
/// 16 MiB `mformat` picks FAT12, which some boot firmwares do not boot from,
/// so with no `fat_size`, and with 16, one sector per cluster makes the
/// images FAT16 when they are big enough for it.
fn mformat_args(bytes: u64, fat_size: u32) -> Vec<&'static str> {
    match fat_size {
        12 => Vec::new(),
        32 => vec!["-F"],
        _ if (FAT16_MIN..FAT16_DEFAULT_FROM).contains(&bytes) => vec!["-c", "1"],
        _ => Vec::new(),
    }
}

/// The FAT type in a boot sector: 12 or 16 at offset 54, 32 at offset 82.
fn fat_type(boot_sector: &[u8]) -> Option<u32> {
    let at = |offset: usize| boot_sector.get(offset..offset + 8);
    match (at(54), at(82)) {
        (Some(b"FAT12   "), _) => Some(12),
        (Some(b"FAT16   "), _) => Some(16),
        (_, Some(b"FAT32   ")) => Some(32),
        _ => None,
    }
}

/// With a `fat_size` asked for, fails when `mformat` wrote another type.
fn check_fat_size(image: &Path, bytes: u64, fat_size: u32) -> Result<(), String> {
    if fat_size == 0 {
        return Ok(());
    }
    let mut boot = [0u8; 90];
    std::fs::File::open(image)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut boot))
        .map_err(|e| format!("failed to read the boot sector: {e}"))?;
    match fat_type(&boot) {
        Some(made) if made == fat_size => Ok(()),
        made => Err(format!(
            "a vfat image of {bytes} bytes asks for fat_size = {fat_size}, but mformat made {}; \
             FAT12 needs less than 16 MiB, FAT16 at least 3 MiB",
            made.map_or_else(|| "an unknown FAT type".to_string(), |m| format!("FAT{m}"))
        )),
    }
}

/// The 8 hex digits of the volume serial: four bytes of the SHA-256 of the
/// tree's digest, under a fixed prefix.
fn volume_serial(digest: &Digest) -> String {
    let hash = Digest::of_bytes(format!("gaia vfat volume serial\n{digest}").as_bytes());
    hash.hex()[..8].to_uppercase()
}

/// Every entry under `root`, `/`-separated, sorted, and whether it is a
/// directory. Links are not followed for recursion.
fn entries(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
        let mut children: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        children.sort_by_key(|entry| entry.path());
        for child in children {
            let path = child.path();
            let name = path
                .strip_prefix(root)
                .map_err(std::io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            let is_dir = child.file_type()?.is_dir();
            out.push((name, path.clone()));
            if is_dir {
                go(root, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(root, root, &mut out)?;
    Ok(out)
}

/// The size an automatic image needs. A link is sized by its target, as
/// `mcopy` reads it; a link to a directory counts as an empty directory.
fn auto_bytes(root: &Path) -> Result<u64, String> {
    fn content(dir: &Path) -> Result<u64, String> {
        let mut total = 0u64;
        let mut children: Vec<_> = std::fs::read_dir(dir)
            .and_then(|read| read.collect::<Result<_, _>>())
            .map_err(|e| format!("failed to read '{}': {e}", dir.display()))?;
        children.sort_by_key(|entry| entry.path());
        for child in children {
            let path = child.path();
            let link = child
                .file_type()
                .map_err(|e| format!("failed to inspect '{}': {e}", path.display()))?
                .is_symlink();
            match std::fs::metadata(&path) {
                Ok(target) if target.is_file() => {
                    total += target.len().div_ceil(CLUSTER) * CLUSTER;
                }
                Ok(target) if target.is_dir() => {
                    total += CLUSTER;
                    if !link {
                        total += content(&path)?;
                    }
                }
                _ => total += CLUSTER,
            }
        }
        Ok(total)
    }
    let content = content(root)?;
    let margin = (content / 20).max(MIN_MARGIN);
    let total = content + FIXED_OVERHEAD + margin;
    Ok(total.div_ceil(MIB) * MIB)
}

/// Copies `source` into `staging` with every entry at the FAT epoch. Files
/// are copied through their links; a dangling link has nothing to copy.
fn stage(source: &Path, staging: &Path) -> Result<(), String> {
    let fail = |what: &str, path: &Path, e: std::io::Error| {
        format!("failed to {what} '{}': {e}", path.display())
    };
    std::fs::create_dir_all(staging).map_err(|e| fail("create", staging, e))?;
    let fixed = std::time::UNIX_EPOCH + std::time::Duration::from_secs(FAT_EPOCH_SECS);
    let all = entries(source).map_err(|e| fail("read", source, e))?;
    let mut dirs = Vec::new();
    for (name, path) in &all {
        let target = staging.join(name);
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| fail("create", &target, e))?;
            dirs.push(target);
        } else if meta.is_file() {
            std::fs::copy(path, &target).map_err(|e| fail("copy", path, e))?;
            std::fs::File::open(&target)
                .and_then(|f| f.set_times(std::fs::FileTimes::new().set_modified(fixed)))
                .map_err(|e| fail("time", &target, e))?;
        }
    }
    // A directory's time changes when entries are copied into it: set last,
    // deepest first.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        std::fs::File::open(&dir)
            .and_then(|f| f.set_times(std::fs::FileTimes::new().set_modified(fixed)))
            .map_err(|e| fail("time", &dir, e))?;
    }
    Ok(())
}

/// Copies each top-level entry of `staging` into the image, with `mcopy -m`
/// (modification times kept) and `TZ=UTC`, so the image holds the same
/// times on every host.
fn copy_in(
    work: &mut Work<'_>,
    mcopy: &str,
    staging: &Path,
    image: &Path,
) -> Result<(), StepError> {
    let mut children: Vec<_> = std::fs::read_dir(staging)
        .and_then(|read| read.collect::<Result<_, _>>())
        .map_err(|e| StepError::body(NAME, format!("failed to read the staged tree: {e}")))?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        work.run(
            &Tool::new(mcopy)
                .env("TZ", "UTC")
                .arg("-i")
                .arg(image)
                .arg("-m")
                .arg("-s")
                .arg(child.path())
                .arg("::"),
        )?;
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FS_VFAT_NODE];
