//! `gaia.fs.erofs`: a stored tree as an EROFS image.
//!
//! The tree is written as a tar stream, sorted by path, every time 0, each
//! path's uid, gid and mode from the tree's entries (root and the stored
//! mode for a path not listed), and `mkfs.erofs --tar=f` builds the image
//! from it. So owners reach the image without root, and the builder's own
//! uid never does. The UUID comes from the tree's [`owned_digest`], the
//! build time is 0 and extended attributes are off, so the same tree gives
//! the same bytes on every host.
//!
//! Compression is declared: `none`, or one `mkfs.erofs` was built with
//! (`lz4`, `lz4hc`, `lzma`, `deflate`, `zstd`), with an optional level.
//! `mkfs.erofs` is pinned by path; its version is in the key, with the
//! tree's `owned_digest`, so an owner change remakes the image.

/// The libraries this node links, pinned in its Cargo.toml. Its step key names
/// them, and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[("tar", "0.4.46")];

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool, once_per_process};
use gaia_store::Digest;
use gaia_types::owned_digest;
use gaia_types::{FileEntry, Image, ImageKind, Size, Tree};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.fs.erofs";
const COMPRESSORS: [&str; 6] = ["none", "lz4", "lz4hc", "lzma", "deflate", "zstd"];

/// The kernel options an erofs compressed with `compression` needs built
/// in, for a kernel that opens it as its root: the filesystem, and for a
/// compressed one its decompression and the algorithm's own option (lz4
/// has none of its own).
pub fn kernel_options(compression: &str) -> Vec<&'static str> {
    let mut options = vec!["CONFIG_EROFS_FS"];
    if compression != "none" {
        options.push("CONFIG_EROFS_FS_ZIP");
    }
    match compression {
        "lzma" => options.push("CONFIG_EROFS_FS_ZIP_LZMA"),
        "deflate" => options.push("CONFIG_EROFS_FS_ZIP_DEFLATE"),
        "zstd" => options.push("CONFIG_EROFS_FS_ZIP_ZSTD"),
        _ => {}
    }
    options
}

/// How `option` stands in the kernel configuration `config`: its value
/// (`y`, `m`), or `not set` for a line that says so or for no line.
pub fn option_value<'a>(config: &'a str, option: &str) -> &'a str {
    config
        .lines()
        .find_map(|line| {
            line.strip_prefix(option)
                .and_then(|rest| rest.strip_prefix('='))
        })
        .unwrap_or("not set")
}

/// Whether a kernel with the final configuration `config` can open an erofs
/// compressed with `compression` as its root.
///
/// # Errors
///
/// The algorithm, each option that is not `=y` and how it stands.
pub fn kernel_reads(config: &str, compression: &str) -> Result<(), String> {
    let lacking: Vec<String> = kernel_options(compression)
        .into_iter()
        .filter(|option| option_value(config, option) != "y")
        .map(|option| format!("{option} is `{}`", option_value(config, option)))
        .collect();
    if lacking.is_empty() {
        return Ok(());
    }
    // What is missing first: a failure's reason is kept short.
    Err(format!(
        "{} in the kernel's .config; an erofs root ({compression}) needs =y",
        lacking.join(", ")
    ))
}

/// The kernel's version, from the first lines of its build tree's
/// `Makefile` (`VERSION = 6`, `PATCHLEVEL = 12`).
pub fn kernel_version(makefile: &str) -> Option<(u32, u32)> {
    let number = |name: &str| {
        makefile.lines().take(8).find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim().parse::<u32>().ok())?
        })
    };
    Some((number("VERSION")?, number("PATCHLEVEL")?))
}

/// The first kernel that reads each thing an erofs image can be made
/// with: an algorithm, or a way of packing compressed data.
const KERNEL_SINCE: [(&str, (u32, u32)); 7] = [
    ("big clusters", (5, 13)),
    ("lzma", (5, 16)),
    ("ztailpacking", (5, 17)),
    ("fragments", (6, 1)),
    ("dedupe", (6, 1)),
    ("deflate", (6, 6)),
    ("zstd", (6, 10)),
];

/// Whether a kernel of `version` reads an erofs made with each of `asked`
/// (names of [`KERNEL_SINCE`]; any other needs no version).
///
/// # Errors
///
/// The kernel's version and each thing it is too old for, with the version
/// that reads it.
pub fn kernel_new_enough(version: (u32, u32), asked: &[&str]) -> Result<(), String> {
    let short: Vec<String> = asked
        .iter()
        .filter_map(|name| {
            KERNEL_SINCE
                .iter()
                .find(|(what, since)| what == name && version < *since)
                .map(|(what, since)| format!("{what} needs {}.{}", since.0, since.1))
        })
        .collect();
    if short.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the image's kernel is {}.{}; of its erofs, {}",
        version.0,
        version.1,
        short.join(", ")
    ))
}

/// The EROFS image of `tree`, compressed with `compression` at `level`
/// (0 for the compressor's default), in physical clusters of up to
/// `cluster` bytes (0 or unset for the tool's default, one block).
///
/// `dictsize` is the lzma dictionary in bytes (0 or unset for the tool's
/// own choice): the kernel holds one of this size while the image is
/// mounted. `fragments`, `ztailpacking` and `dedupe` are the tool's ways of
/// packing small files and tails more tightly, each off unless set. (Its
/// `all-fragments` is not offered: reading a tar, as this step has it do
/// for the owners and modes, erofs-utils 1.8.10 writes every file's data
/// as zeros with it, and reports success.)
///
/// What is made is read back before it is kept: the image is unpacked with
/// the same tools' `fsck.erofs` and every file compared with the tree's. An
/// image that does not hold the tree fails the step, whatever the tool
/// said; one that does is kept with the step's second output, `read-back`,
/// a line that says how many were compared.
///
/// `tools` is the image's build's host tools, when it has an OS: the
/// `mkfs.erofs` in them makes the image, and `mkfs_erofs` (a path on this
/// machine) is used only for an image with no OS build.
///
/// `kernel` is the image's kernel build tree, when it has one: its final
/// `.config` must be able to read what is made ([`kernel_reads`]), or the
/// step fails before anything is made. Nothing is changed in the kernel.
#[node(
    id = "gaia.fs.erofs",
    inputs(
        store,
        mkfs_erofs,
        tools,
        tree,
        compression,
        level,
        cluster,
        dictsize,
        fragments,
        ztailpacking,
        dedupe,
        kernel
    ),
    outputs(image)
)]
#[allow(clippy::too_many_arguments)]
pub fn fs_erofs(
    store: &str,
    mkfs_erofs: Option<&String>,
    tools: Option<&Tree>,
    tree: Tree,
    compression: &str,
    level: u32,
    cluster: Option<&i64>,
    dictsize: Option<&i64>,
    fragments: Option<&bool>,
    ztailpacking: Option<&bool>,
    dedupe: Option<&bool>,
    kernel: Option<&Tree>,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
    // The image's own build's mkfs.erofs when it has one, never this
    // machine's: what makes the image is pinned with it.
    let program = gaia_step::image_tool(
        &store,
        tools.and_then(|tree| tree.object.object_ref()).as_ref(),
        mkfs_erofs.map(String::as_str),
        "mkfs.erofs",
        "BR2_PACKAGE_HOST_EROFS_UTILS=y",
    )
    .map_err(fail)?;
    let program = program.display().to_string();
    let mkfs_erofs: &str = &program;
    if !COMPRESSORS.contains(&compression) {
        return Err(fail(format!(
            "compression {compression:?} is not one of {}",
            COMPRESSORS.join(", ")
        )));
    }
    let cluster = cluster.copied().unwrap_or(0);
    if cluster < 0 || cluster % 4096 != 0 {
        return Err(fail(format!(
            "cluster {cluster} is not a number of 4096-byte blocks"
        )));
    }
    if cluster != 0 && compression == "none" {
        return Err(fail(format!(
            "cluster {cluster} is set and compression is none: clusters are of compressed data"
        )));
    }
    let dictsize = dictsize.copied().unwrap_or(0);
    if dictsize < 0 || (dictsize != 0 && compression != "lzma") {
        return Err(fail(format!(
            "dictsize {dictsize} is for lzma, above 0; the compression is {compression}"
        )));
    }
    // The extended options asked for, as mkfs.erofs names them.
    let extended: Vec<&str> = [
        (fragments, "fragments"),
        (ztailpacking, "ztailpacking"),
        (dedupe, "dedupe"),
    ]
    .into_iter()
    .filter(|(on, _)| on.copied().unwrap_or(false))
    .map(|(_, name)| name)
    .collect();
    if !extended.is_empty() && compression == "none" {
        return Err(fail(format!(
            "{} is set and compression is none: these are ways of packing compressed data",
            extended.join(", ")
        )));
    }
    // Checked on every run, found in the store or not: the kernel is not in
    // the key, so a kernel that lost the option fails here all the same.
    if let Some(kernel) = kernel
        && let Some(build) = kernel.object.path_in(&store)
        && let Ok(makefile) = std::fs::read_to_string(build.join("Makefile"))
        && let Some(version) = kernel_version(&makefile)
    {
        let mut asked: Vec<&str> = vec![compression];
        if cluster > 4096 {
            asked.push("big clusters");
        }
        asked.extend(&extended);
        kernel_new_enough(version, &asked).map_err(fail)?;
    }
    if let Some(kernel) = kernel {
        let build = kernel
            .object
            .path_in(&store)
            .ok_or_else(|| fail("the kernel build tree is not in the store".into()))?;
        // A build with no kernel has an empty tree: nothing to hold it to.
        if let Ok(config) = std::fs::read_to_string(build.join(".config")) {
            kernel_reads(&config, compression).map_err(fail)?;
        }
    }
    let owned =
        owned_digest(&tree).ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let dir = tree
        .object
        .path_in(&store)
        .ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let version = once_per_process(Path::new(mkfs_erofs), || {
        let out = std::process::Command::new(mkfs_erofs)
            .arg("--version")
            .env_clear()
            .output()?;
        Ok([out.stdout, out.stderr].concat())
    })
    .map_err(|e| fail(format!("{mkfs_erofs} --version: {e}")))?;
    // 2: what is made is read back, and the step says so in an output of
    // its own. No image made before that is taken for one that was.
    let mut key = KeyBuilder::new(NAME, 2)
        .config("mkfs.erofs", &version)
        .config("compression", compression)
        .config("level", level.to_string())
        .input("tree", owned)
        .libs(crate::LIBS);
    // An image with the default clusters has the key it had; and so for
    // each thing asked for beyond the defaults.
    if cluster != 0 {
        key = key.config("cluster", cluster.to_string());
    }
    if dictsize != 0 {
        key = key.config("dictsize", dictsize.to_string());
    }
    if !extended.is_empty() {
        key = key.config("extended", extended.join(","));
    }
    let key = key.finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("image", Path::new("image.erofs")),
            ("read-back", Path::new("read-back.txt")),
        ],
        |work| {
            let tar_path = work.dir().join("tree.tar");
            write_tar(&dir, &tree.entries, &tar_path).map_err(|e| fail(format!("tar: {e}")))?;
            let mut mkfs = Tool::new(mkfs_erofs)
                .args(["--quiet", "--tar=f", "-T0", "-x-1"])
                .arg("-U")
                .arg(uuid_of(&owned));
            if compression != "none" {
                let mut z = format!("-z{compression}");
                if level > 0 {
                    z.push_str(&format!(",level={level}"));
                }
                if dictsize != 0 {
                    z.push_str(&format!(",dictsize={dictsize}"));
                }
                mkfs = mkfs.arg(z);
            }
            if cluster != 0 {
                mkfs = mkfs.arg(format!("-C{cluster}"));
            }
            if !extended.is_empty() {
                mkfs = mkfs.arg(format!("-E{}", extended.join(",")));
            }
            let image = work.dir().join("image.erofs");
            work.run(&mkfs.arg(&image).arg(&tar_path))?;
            std::fs::remove_file(&tar_path).map_err(|e| fail(e.to_string()))?;
            // Read back with the same tools: the tool's exit status is not
            // proof that the image holds the tree.
            let fsck = Path::new(mkfs_erofs).with_file_name("fsck.erofs");
            if !fsck.is_file() {
                return Err(fail(format!(
                    "no fsck.erofs beside {mkfs_erofs}: what was made cannot be read back"
                )));
            }
            let check = work.dir().join("check");
            work.run(
                &Tool::new(&fsck)
                    .arg(format!("--extract={}", check.display()))
                    .arg(&image),
            )?;
            let (compared, differing) =
                differing(&dir, &check).map_err(|e| fail(format!("read back: {e}")))?;
            remove_tree(&check);
            if !differing.is_empty() {
                return Err(fail(format!(
                    "the image made does not hold the tree: {} of its files differ when read back ({}{})",
                    differing.len(),
                    differing[..differing.len().min(3)].join(", "),
                    if differing.len() > 3 { ", ..." } else { "" }
                )));
            }
            // The record that it was: what a size is read beside.
            std::fs::write(
                work.dir().join("read-back.txt"),
                format!("{compared} files and links read back with fsck.erofs: all as the tree has them\n"),
            )
            .map_err(|e| fail(e.to_string()))?;
            Ok(())
        },
    )?;
    let object = made.outputs["image"];
    let bytes = std::fs::metadata(store.path(&object))
        .map_err(|e| fail(e.to_string()))?
        .len();
    Ok(Image {
        kind: ImageKind::Filesystem,
        object: object.into(),
        size: Size(bytes),
        partitions: Vec::new(),
    })
}

/// How many regular files and symlinks `tree` has, and those of them that
/// `unpacked` does not hold with the same bytes (or the same target).
fn differing(tree: &Path, unpacked: &Path) -> std::io::Result<(usize, Vec<String>)> {
    let mut paths = BTreeMap::new();
    walk(tree, "", &mut paths)?;
    let mut differ = Vec::new();
    let mut compared = 0;
    for (path, full) in paths {
        let meta = std::fs::symlink_metadata(&full)?;
        let other = unpacked.join(&path);
        let same = if meta.file_type().is_symlink() {
            std::fs::read_link(&other)
                .is_ok_and(|to| std::fs::read_link(&full).is_ok_and(|want| to == want))
        } else if meta.is_file() {
            std::fs::read(&other)
                .is_ok_and(|got| std::fs::read(&full).is_ok_and(|want| got == want))
        } else {
            continue;
        };
        compared += 1;
        if !same {
            differ.push(path);
        }
    }
    Ok((compared, differ))
}

/// Removes a tree fsck.erofs unpacked, whose directories may not be
/// writable.
fn remove_tree(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fn open(dir: &Path) {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                open(&path);
            }
        }
    }
    open(dir);
    let _ = std::fs::remove_dir_all(dir);
}

/// The tree as a tar: paths sorted, times 0, numeric owners with no
/// names, owners and modes from `entries` or root and the stored mode.
fn write_tar(root: &Path, entries: &[FileEntry], out: &Path) -> std::io::Result<()> {
    let by_path: BTreeMap<&str, &FileEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut paths = BTreeMap::new();
    walk(root, "", &mut paths)?;
    let mut builder = tar::Builder::new(std::io::BufWriter::new(std::fs::File::create(out)?));
    builder.mode(tar::HeaderMode::Deterministic);
    for (path, full) in paths {
        let meta = std::fs::symlink_metadata(&full)?;
        let (uid, gid, mode) = match by_path.get(path.as_str()) {
            Some(e) => (e.uid, e.gid, e.mode & 0o7777),
            None => (0, 0, meta.mode() & 0o7777),
        };
        let mut header = tar::Header::new_gnu();
        header.set_uid(u64::from(uid));
        header.set_gid(u64::from(gid));
        header.set_mtime(0);
        header.set_mode(mode);
        let kind = meta.file_type();
        if kind.is_symlink() {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            builder.append_link(&mut header, &path, std::fs::read_link(&full)?)?;
        } else if kind.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            builder.append_data(&mut header, format!("{path}/"), std::io::empty())?;
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            builder.append_data(&mut header, &path, std::fs::File::open(&full)?)?;
        }
    }
    builder
        .into_inner()?
        .into_inner()
        .map_err(|e| e.into_error())?
        .sync_all()
}

fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = entry.file_type()?.is_dir();
        out.insert(path.clone(), entry.path());
        if is_dir {
            walk(&entry.path(), &path, out)?;
        }
    }
    Ok(())
}

/// A UUID from the tree's digest, version 4 and variant bits set.
fn uuid_of(digest: &Digest) -> String {
    let hex = digest.hex();
    let mut b: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FS_EROFS_NODE];
