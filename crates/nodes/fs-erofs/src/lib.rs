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

/// The EROFS image of `tree`, compressed with `compression` at `level`
/// (0 for the compressor's default), in physical clusters of up to
/// `cluster` bytes (0 or unset for the tool's default, one block).
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
    inputs(store, mkfs_erofs, tools, tree, compression, level, cluster, kernel),
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
    // Checked on every run, found in the store or not: the kernel is not in
    // the key, so a kernel that lost the option fails here all the same.
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
    let mut key = KeyBuilder::new(NAME, 1)
        .config("mkfs.erofs", &version)
        .config("compression", compression)
        .config("level", level.to_string())
        .input("tree", owned)
        .libs(crate::LIBS);
    // An image with the default clusters has the key it had.
    if cluster != 0 {
        key = key.config("cluster", cluster.to_string());
    }
    let key = key.finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("image", Path::new("image.erofs"))],
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
                mkfs = mkfs.arg(z);
            }
            if cluster != 0 {
                mkfs = mkfs.arg(format!("-C{cluster}"));
            }
            work.run(&mkfs.arg(work.dir().join("image.erofs")).arg(&tar_path))?;
            std::fs::remove_file(&tar_path).map_err(|e| fail(e.to_string()))
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
