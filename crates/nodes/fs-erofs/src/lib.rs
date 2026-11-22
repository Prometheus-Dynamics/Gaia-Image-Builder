//! `gaia.fs.erofs`: a stored tree as an EROFS image.
//!
//! The tree is written as a tar stream, sorted by path, every time 0, each
//! path's uid, gid and mode from the tree's entries (root and the stored
//! mode for a path not listed), and `mkfs.erofs --tar=f` builds the image
//! from it. So owners reach the image without root, and the builder's own
//! uid never does. The stream's first entry is the image's root directory,
//! `./`: root's, mode 0755, unless the tree lists its root (path `` or `.`).
//! Without it mkfs.erofs gives the root inode the builder's uid and gid and
//! mode 0777 (erofs-utils 1.9.4), and the image's `/` is world-writable. The UUID comes from the tree's [`owned_digest`], the
//! build time is 0 and extended attributes are off, so the same tree gives
//! the same bytes on every host. The block size is 4096 bytes, named
//! (`-b4096`), not the build machine's page size, which is mkfs.erofs's
//! default: a host with 16 KiB pages would make an image a 4 KiB-page
//! kernel cannot mount. The superblock read back must say 4096.
//!
//! Compression is declared: `none`, or one `mkfs.erofs` was built with
//! (`lz4`, `lz4hc`, `lzma`, `deflate`, `zstd`), with an optional level.
//! `mkfs.erofs` is the build machine's when it meets the floor, else the
//! image's Buildroot host tool (see [`tool`]). Its full `--version` text is
//! in the key, with the tree's `owned_digest`, so an owner change remakes the
//! image and another erofs-utils makes another key. The bytes depend on the
//! erofs-utils that made them, and the key records which one.
//!
//! No setuid or setgid bit reaches the image unless asked for: every one is
//! cleared except on the paths in `keep_setid` (absolute, as the image has
//! them). A build system may set such a bit on a package's file with no way
//! to take it back, so the rule is here, for every file. A listed path that
//! the tree does not have, or that carries no such bit, fails the step:
//! nothing listed is ignored. The step's `setid` output says each bit
//! cleared and each kept, with path and mode.

/// The libraries this node links, pinned in its Cargo.toml. Its step key names
/// them, and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[("tar", "0.4.46")];

pub mod superblock;
pub mod tool;

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool, once_per_process};
use gaia_store::{Digest, StepKey};
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
/// `mkfs_erofs` is the program that makes the image when it is given (not
/// empty): the build machine's, which must meet the floor of [`tool`]. Empty
/// or none, the image's `tools` make it with their `mkfs.erofs`, which the
/// plan put there (Buildroot's host erofs-utils) when the machine's tool is
/// not used.
///
/// `keep_setid`: the paths whose setuid and setgid bits are kept,
/// separated by whitespace; every other such bit is cleared (see the
/// crate's documentation).
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
        keep_setid,
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
    keep_setid: Option<&String>,
    kernel: Option<&Tree>,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
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
    // The build machine's tool when one is given, checked against what this
    // image asks of it; else the image's own tools' (the plan gave no
    // machine tool because its floor was not met, or the setting forbids it).
    let asked = tool::Asked {
        compression,
        extended: &extended,
    };
    let program = match mkfs_erofs.map(String::as_str).filter(|p| !p.is_empty()) {
        Some(machine) => {
            tool::check(machine, &asked).map_err(fail)?;
            machine.to_string()
        }
        None => gaia_step::image_tool(
            &store,
            tools.and_then(|tree| tree.object.object_ref()).as_ref(),
            None,
            "mkfs.erofs",
            tool::BUILDROOT_LINE,
        )
        .map_err(fail)?
        .display()
        .to_string(),
    };
    let mkfs_erofs: &str = &program;
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
    let keep: Vec<String> = keep_setid
        .map(|text| text.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    for path in &keep {
        if !path.starts_with('/') || path.split('/').any(|part| part == "..") || path.len() < 2 {
            return Err(fail(format!(
                "keep_setid {path:?} is not an absolute path in the image"
            )));
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
    let key = image_key(
        &version,
        &ImageOptions {
            compression,
            level,
            cluster,
            dictsize,
            extended: &extended,
            keep: &keep,
        },
        &owned,
    );
    // The worker count is the same for a given host, and changes no byte of
    // the image, so it is not in the key.
    // Asked of the tool itself: a tool built without threads has the version
    // of one built with them, and refuses the flag.
    let help = std::process::Command::new(mkfs_erofs)
        .arg("--help")
        .env_clear()
        .output()
        .map(|out| [out.stdout, out.stderr].concat())
        .map_err(|e| fail(format!("{mkfs_erofs} --help: {e}")))?;
    let workers = workers_flag(&String::from_utf8_lossy(&help), worker_count());
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("image", Path::new("image.erofs")),
            ("read-back", Path::new("read-back.txt")),
            ("setid", Path::new("setid.txt")),
        ],
        |work| {
            let tar_path = work.dir().join("tree.tar");
            let setid = write_tar(&dir, &tree.entries, &keep, &tar_path)
                .map_err(|e| fail(format!("tar: {e}")))?
                .map_err(fail)?;
            std::fs::write(work.dir().join("setid.txt"), setid)
                .map_err(|e| fail(e.to_string()))?;
            let image = work.dir().join("image.erofs");
            let args = mkfs_options(
                &uuid_of(&owned),
                &ImageOptions {
                    compression,
                    level,
                    cluster,
                    dictsize,
                    extended: &extended,
                    keep: &keep,
                },
                workers.clone(),
            );
            work.run(
                &Tool::new(mkfs_erofs)
                    .args(args)
                    .arg(&image)
                    .arg(&tar_path),
            )?;
            std::fs::remove_file(&tar_path).map_err(|e| fail(e.to_string()))?;
            // The superblock says what the tool made: a compressor or option
            // it ignored fails the step, whatever its exit status.
            let head = {
                use std::io::Read;
                let mut head = Vec::new();
                std::fs::File::open(&image)
                    .and_then(|file| file.take(4096).read_to_end(&mut head))
                    .map_err(|e| fail(format!("read the image's superblock: {e}")))?;
                head
            };
            let superblock = superblock::read(&head)
                .and_then(|sb| superblock::check(&sb, compression, &extended))
                .map_err(|e| fail(format!("the image made is not as asked: {e}")))?;
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
                format!("{compared} files and links read back with fsck.erofs: all as the tree has them\n{superblock}\n"),
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

/// What an image is made with, apart from the tree and the tool: the
/// compression and its settings, the packing options, and the paths whose
/// setuid and setgid bits are kept (for the key; the tar carries them).
#[derive(Debug)]
pub struct ImageOptions<'a> {
    pub compression: &'a str,
    pub level: u32,
    /// Bytes of a physical cluster; 0 for the tool's default.
    pub cluster: i64,
    /// The lzma dictionary in bytes; 0 for the tool's own choice.
    pub dictsize: i64,
    /// `fragments`, `ztailpacking` and `dedupe` as mkfs.erofs names them.
    pub extended: &'a [&'a str],
    pub keep: &'a [String],
}

/// The step's key: the tool's `--version` output, the image's options and
/// the tree's owner digest. Each option is in it only when it is set, so an
/// image made with the defaults keeps the key it had. The worker count is
/// not an option: it changes how fast the image is made, not what it holds.
pub fn image_key(version: &[u8], options: &ImageOptions<'_>, owned: &Digest) -> StepKey {
    // 2: what is made is read back, and the step says so in an output of
    // its own. No image made before that is taken for one that was.
    // 3: setuid and setgid bits are cleared but for those kept.
    // 4: the superblock is read back, and the compression, fragments and
    // ztailpacking asked for are checked against it. No image made before
    // that is taken for one that was.
    // 5: the block size is 4096, named, and checked in the superblock.
    // 6: the image's root directory is root's and 0755 (before, the
    // builder's uid and gid and 0777). No image made before is taken.
    let mut key = KeyBuilder::new(NAME, 6)
        .config("mkfs.erofs", version)
        .config("compression", options.compression)
        .config("level", options.level.to_string())
        .input("tree", *owned)
        .libs(crate::LIBS);
    if options.cluster != 0 {
        key = key.config("cluster", options.cluster.to_string());
    }
    if options.dictsize != 0 {
        key = key.config("dictsize", options.dictsize.to_string());
    }
    if !options.extended.is_empty() {
        key = key.config("extended", options.extended.join(","));
    }
    if !options.keep.is_empty() {
        key = key.config("keep_setid", options.keep.join("\n"));
    }
    key.finish()
}

/// The arguments of `mkfs.erofs` that make an image from a tar, all but
/// the image's path and the tar's: the quiet, tar, deterministic-time,
/// no-xattr and block-size flags, the UUID, the compressor with its level and dictionary,
/// the cluster, the packing options and `workers` (see [`workers_flag`]).
pub fn mkfs_options(
    uuid: &str,
    options: &ImageOptions<'_>,
    workers: Option<String>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--quiet".into(),
        "--tar=f".into(),
        "-T0".into(),
        "-x-1".into(),
        format!("-b{}", superblock::BLOCK_SIZE),
        "-U".into(),
        uuid.into(),
    ];
    if options.compression != "none" {
        let mut z = format!("-z{}", options.compression);
        if options.level > 0 {
            z.push_str(&format!(",level={}", options.level));
        }
        if options.dictsize != 0 {
            z.push_str(&format!(",dictsize={}", options.dictsize));
        }
        args.push(z);
    }
    if options.cluster != 0 {
        args.push(format!("-C{}", options.cluster));
    }
    if !options.extended.is_empty() {
        args.push(format!("-E{}", options.extended.join(",")));
    }
    args.extend(workers);
    args
}

/// `--workers=N` when the tool whose `--help` is `help` lists the flag, and
/// `workers` is above 0; none otherwise, so a tool without it is run as it
/// was. The help text decides, not the version: erofs-utils built without
/// threads has no `--workers` at any version. The count is for the CPUs this
/// process may use: how many mkfs.erofs runs at once is up to the host, not
/// to the image.
pub fn workers_flag(help: &str, workers: u32) -> Option<String> {
    let takes = help
        .lines()
        .any(|line| line.trim_start().starts_with("--workers"));
    (takes && workers > 0).then(|| format!("--workers={workers}"))
}

/// The CPUs this process may run on: the affinity mask, so a `taskset` cap
/// holds, and 1 when that is not known.
fn worker_count() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1))
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

/// The setuid and setgid bits.
const SETID: u32 = 0o6000;

/// The image's root directory when the tree does not list it: root's, 0755.
const ROOT_DIR: (u32, u32, u32) = (0, 0, 0o755);

/// The tree as a tar: paths sorted, times 0, numeric owners with no
/// names, owners and modes from `entries` or root and the stored mode, and
/// no setuid or setgid bit but on the paths in `keep`. Returns the report of
/// the bits cleared and kept, or why a path in `keep` cannot be kept.
fn write_tar(
    root: &Path,
    entries: &[FileEntry],
    keep: &[String],
    out: &Path,
) -> std::io::Result<Result<String, String>> {
    let by_path: BTreeMap<&str, &FileEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut paths = BTreeMap::new();
    walk(root, "", &mut paths)?;
    let mode_of = |path: &str, full: &Path| -> std::io::Result<u32> {
        Ok(match by_path.get(path) {
            Some(e) => e.mode & 0o7777,
            None => std::fs::symlink_metadata(full)?.mode() & 0o7777,
        })
    };
    for path in keep {
        let relative = path.trim_start_matches('/');
        let Some(full) = paths.get(relative) else {
            return Ok(Err(format!("keep_setid {path}: the tree has no such file")));
        };
        let meta = std::fs::symlink_metadata(full)?;
        if meta.file_type().is_symlink() || mode_of(relative, full)? & SETID == 0 {
            return Ok(Err(format!(
                "keep_setid {path}: it carries no setuid or setgid bit, so there is nothing to keep"
            )));
        }
    }
    let mut report = String::new();
    let mut builder = tar::Builder::new(std::io::BufWriter::new(std::fs::File::create(out)?));
    builder.mode(tar::HeaderMode::Deterministic);
    // The root directory first: mkfs.erofs takes the root inode's owner and
    // mode from this entry, and from the builder when there is none.
    let (uid, gid, mode) = match by_path.get("").or_else(|| by_path.get(".")) {
        Some(e) => (e.uid, e.gid, e.mode & 0o7777 & !SETID),
        None => ROOT_DIR,
    };
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_uid(u64::from(uid));
    header.set_gid(u64::from(gid));
    header.set_mtime(0);
    header.set_mode(mode);
    header.set_size(0);
    builder.append_data(&mut header, "./", std::io::empty())?;
    for (path, full) in paths {
        let meta = std::fs::symlink_metadata(&full)?;
        let (uid, gid, mut mode) = match by_path.get(path.as_str()) {
            Some(e) => (e.uid, e.gid, e.mode & 0o7777),
            None => (0, 0, meta.mode() & 0o7777),
        };
        if mode & SETID != 0 && !meta.file_type().is_symlink() {
            if keep.iter().any(|k| k.trim_start_matches('/') == path) {
                report.push_str(&format!("kept    /{path} {mode:04o}\n"));
            } else {
                report.push_str(&format!(
                    "cleared /{path} {mode:04o} -> {:04o}\n",
                    mode & !SETID
                ));
                mode &= !SETID;
            }
        }
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
        .sync_all()?;
    if report.is_empty() {
        report.push_str("no setuid or setgid bits\n");
    }
    Ok(Ok(report))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The report: each bit cleared and each kept, by path and mode.
    #[test]
    fn the_report_names_each_bit_cleared_and_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("bin")).expect("mkdir");
        for name in ["busybox", "su", "plain"] {
            std::fs::write(root.join("bin").join(name), name).expect("write");
        }
        let entry = |path: &str, mode| FileEntry {
            path: path.into(),
            mode,
            uid: 0,
            gid: 0,
        };
        let entries = [
            entry("bin/busybox", 0o4755),
            entry("bin/su", 0o6750),
            entry("bin/plain", 0o755),
        ];
        let tar = dir.path().join("t.tar");
        let report = write_tar(&root, &entries, &["/bin/su".into()], &tar)
            .expect("io")
            .expect("kept");
        assert_eq!(
            report,
            "cleared /bin/busybox 4755 -> 0755\nkept    /bin/su 6750\n"
        );
        let modes: BTreeMap<String, u32> =
            tar::Archive::new(std::fs::File::open(&tar).expect("open"))
                .entries()
                .expect("entries")
                .map(|e| {
                    let e = e.expect("entry");
                    (
                        e.path().expect("path").display().to_string(),
                        e.header().mode().expect("mode"),
                    )
                })
                .collect();
        assert_eq!(modes["bin/busybox"], 0o755);
        assert_eq!(modes["bin/su"], 0o6750);
        assert_eq!(modes["bin/plain"], 0o755);
        assert_eq!(modes["./"], 0o755, "{modes:?}");
        let none = write_tar(&root, &entries[2..], &[], &tar)
            .expect("io")
            .expect("ok");
        assert_eq!(none, "no setuid or setgid bits\n");
    }

    /// The stream's first entry is the root directory, root's and 0755, or
    /// as the tree lists it: never the stored directory's owner or mode.
    #[test]
    fn the_root_directory_comes_first_and_is_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("etc")).expect("mkdir");
        std::fs::set_permissions(
            &root,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o777),
        )
        .expect("chmod");
        let tar = dir.path().join("t.tar");
        let first = |entries: &[FileEntry]| {
            write_tar(&root, entries, &[], &tar)
                .expect("io")
                .expect("ok");
            let mut archive = tar::Archive::new(std::fs::File::open(&tar).expect("open"));
            let mut all = archive.entries().expect("entries");
            let e = all.next().expect("an entry").expect("entry");
            let h = e.header();
            (
                e.path().expect("path").display().to_string(),
                h.entry_type(),
                h.uid().expect("uid"),
                h.gid().expect("gid"),
                h.mode().expect("mode"),
            )
        };
        assert_eq!(
            first(&[]),
            ("./".into(), tar::EntryType::Directory, 0, 0, 0o755)
        );
        let listed = FileEntry {
            path: "".into(),
            mode: 0o40750,
            uid: 5,
            gid: 6,
        };
        assert_eq!(
            first(&[listed]),
            ("./".into(), tar::EntryType::Directory, 5, 6, 0o750)
        );
    }

    fn options<'a>(compression: &'a str, level: u32, extended: &'a [&'a str]) -> ImageOptions<'a> {
        ImageOptions {
            compression,
            level,
            cluster: 0,
            dictsize: 0,
            extended,
            keep: &[],
        }
    }

    /// The arguments before the image's and the tar's paths, as they are
    /// run for `options`.
    fn args(options: &ImageOptions<'_>, workers: Option<&str>) -> Vec<String> {
        mkfs_options(
            "6f1c2a3b-4d5e-4f60-8a71-b2c3d4e5f607",
            options,
            workers.map(str::to_string),
        )
    }

    /// The compressor argument of each compression, with and without a level.
    #[test]
    fn each_compression_is_passed_as_mkfs_erofs_names_it() {
        let flags = |compression: &str, level: u32| {
            args(&options(compression, level, &[]), None)
                .into_iter()
                .filter(|a| a.starts_with("-z"))
                .collect::<Vec<_>>()
        };
        assert_eq!(flags("none", 0), Vec::<String>::new());
        assert_eq!(flags("lz4", 0), ["-zlz4"]);
        assert_eq!(flags("lz4hc", 0), ["-zlz4hc"]);
        assert_eq!(flags("lz4hc", 9), ["-zlz4hc,level=9"]);
        assert_eq!(flags("lzma", 0), ["-zlzma"]);
        assert_eq!(flags("deflate", 6), ["-zdeflate,level=6"]);
        assert_eq!(flags("zstd", 3), ["-zzstd,level=3"]);
    }

    /// The fixed flags and the UUID come first, and nothing else is added
    /// for the defaults.
    #[test]
    fn the_defaults_are_the_fixed_flags_and_the_uuid() {
        assert_eq!(
            args(&options("none", 0, &[]), None),
            [
                "--quiet",
                "--tar=f",
                "-T0",
                "-x-1",
                "-b4096",
                "-U",
                "6f1c2a3b-4d5e-4f60-8a71-b2c3d4e5f607",
            ]
        );
    }

    /// The dictionary, cluster and packing options each come as their flag.
    #[test]
    fn the_settings_beyond_the_compressor_are_their_flags() {
        let extended = ["fragments", "dedupe"];
        let mut lzma = options("lzma", 0, &extended);
        lzma.dictsize = 8_388_608;
        lzma.cluster = 16_384;
        let out = args(&lzma, None);
        assert!(
            out.contains(&"-zlzma,dictsize=8388608".to_string()),
            "{out:?}"
        );
        assert!(out.contains(&"-C16384".to_string()), "{out:?}");
        assert!(out.contains(&"-Efragments,dedupe".to_string()), "{out:?}");
    }

    /// Workers are the last argument, and only when given.
    #[test]
    fn workers_are_the_last_argument() {
        let out = args(&options("lz4hc", 0, &[]), Some("--workers=26"));
        assert_eq!(out.last().map(String::as_str), Some("--workers=26"));
        assert_eq!(out.iter().filter(|a| a.starts_with("--workers")).count(), 1);
        assert!(
            !args(&options("lz4hc", 0, &[]), None)
                .iter()
                .any(|a| a.starts_with("--workers"))
        );
    }

    /// The flag is passed to a tool whose help lists it, for a count above
    /// 0; a 1.8.10 built without threads lists none and gets none.
    #[test]
    fn the_workers_flag_needs_a_tool_that_lists_it() {
        let threads = "Usage: mkfs.erofs [OPTIONS] FILE SOURCE(s)\n --uid-offset=#  add offset\n --workers=#            set the number of worker threads to # (default: 32)\n";
        assert_eq!(workers_flag(threads, 26).as_deref(), Some("--workers=26"));
        let none = "Usage: mkfs.erofs [OPTIONS] FILE SOURCE(s)\n --uid-offset=#  add offset\n";
        assert_eq!(workers_flag(none, 4), None, "no --workers in the help");
        assert_eq!(workers_flag(threads, 0), None, "no workers, no flag");
        assert_eq!(workers_flag("see the --workers note", 4), None);
    }

    /// The key changes with the compression and its settings, and is the
    /// same for the same ones. A worker count is not an input of it.
    #[test]
    fn the_key_changes_with_the_compression_not_the_workers() {
        let owned = Digest::from_bytes([7; 32]);
        let version = b"mkfs.erofs (erofs-utils) 1.9.4\n";
        let key = |compression: &str| image_key(version, &options(compression, 0, &[]), &owned);
        let lzma = key("lzma");
        assert_eq!(lzma, key("lzma"), "the same options, the same key");
        assert_ne!(lzma, key("lz4"));
        assert_ne!(key("lz4"), key("lz4hc"));
        assert_ne!(key("lz4hc"), key("none"));
        let mut other_level = options("lz4hc", 9, &[]);
        assert_ne!(key("lz4hc"), image_key(version, &other_level, &owned));
        other_level.level = 0;
        assert_eq!(key("lz4hc"), image_key(version, &other_level, &owned));
        assert_ne!(
            key("lzma"),
            image_key(
                b"mkfs.erofs (erofs-utils) 1.8.10\n",
                &options("lzma", 0, &[]),
                &owned
            ),
            "a different tool is a different image"
        );
    }
}
