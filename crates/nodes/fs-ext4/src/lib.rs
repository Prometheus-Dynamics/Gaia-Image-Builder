//! `gaia.fs.ext4`: a stored tree as an ext4 image.
//!
//! `mke2fs -d` copies the tree in, then `debugfs` sets every inode's owner
//! and mode from the tree's entries (root and the stored mode for a path
//! not listed) and every timestamp to 0, so no root is needed and the
//! builder's own uid never reaches the image. The UUID and the directory
//! hash seed come from the tree's [`owned_digest`], and mke2fs's clock is
//! fixed, so the same tree gives the same bytes on every host.
//!
//! The size is either fixed, or the content (each file and directory
//! rounded up to a block), the inode tables, the metadata overhead and the
//! journal, plus a declared margin, rounded up to 1 MiB.
//!
//! `mke2fs` and `debugfs` are pinned by path; their versions are in the key.
//! The key is the tree's `owned_digest`, so an owner change remakes it.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Stream, Tool};
use gaia_store::Digest;
use gaia_types::owned_digest;
use gaia_types::{FileEntry, Image, ImageKind, Size, Tree};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

const NAME: &str = "gaia.fs.ext4";
const BLOCK: u64 = 4096;
const INODE_BYTES: u64 = 256;
const MIB: u64 = 1024 * 1024;
/// Group descriptors, bitmaps and the like.
const OVERHEAD: u64 = 8 * MIB;
/// `E2FSPROGS_FAKE_TIME` takes 0 as unset; the epoch plus one stands in,
/// and every inode is then set to 0.
const FAKE_TIME: &str = "1";

/// The ext4 image of `tree`: `size` bytes, or sized from the content plus
/// `margin` bytes when `size` is 0. `label` may be empty; `journal` keeps a
/// journal (1/64 of the image, 4 to 128 MiB).
#[node(
    id = "gaia.fs.ext4",
    inputs(store, mke2fs, debugfs, tree, size, margin, label, journal),
    outputs(image)
)]
#[allow(clippy::too_many_arguments)]
pub fn fs_ext4(
    store: &str,
    mke2fs: &str,
    debugfs: &str,
    tree: Tree,
    size: u64,
    margin: u64,
    label: &str,
    journal: bool,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
    let owned =
        owned_digest(&tree).ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let dir = tree
        .object
        .path_in(&store)
        .ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let version = |program: &str| {
        std::process::Command::new(program)
            .arg("-V")
            .env_clear()
            .output()
            .map_err(|e| fail(format!("{program} -V: {e}")))
            .map(|o| [o.stdout, o.stderr].concat())
    };
    let key = KeyBuilder::new(NAME, 1)
        .config("mke2fs", version(mke2fs)?)
        .config("debugfs", version(debugfs)?)
        .config("size", size.to_string())
        .config("margin", margin.to_string())
        .config("label", label)
        .config("journal", if journal { "yes" } else { "no" })
        .input("tree", owned)
        .finish();
    let paths = walk(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("image", Path::new("image.ext4"))],
        |work| {
            let image = work.dir().join("image.ext4");
            let bytes = if size > 0 {
                size
            } else {
                auto_bytes(&paths, margin)
            };
            if bytes % BLOCK != 0 {
                return Err(fail(format!(
                    "{bytes} bytes is not a whole number of 4 KiB blocks"
                )));
            }
            let file = std::fs::File::create(&image).map_err(|e| fail(e.to_string()))?;
            file.set_len(bytes).map_err(|e| fail(e.to_string()))?;
            drop(file);
            let uuid = uuid_of(&owned);
            let mut mkfs = Tool::new(mke2fs)
                .env("E2FSPROGS_FAKE_TIME", FAKE_TIME)
                .args([
                    "-q", "-F", "-t", "ext4", "-b", "4096", "-I", "256", "-m", "0",
                ])
                .arg("-N")
                .arg(inode_count(paths.len()).to_string())
                .arg("-U")
                .arg(&uuid)
                .arg("-E")
                .arg(format!("hash_seed={uuid},root_owner=0:0"));
            mkfs = if journal {
                mkfs.arg("-J")
                    .arg(format!("size={}", journal_bytes(bytes) / MIB))
            } else {
                mkfs.args(["-O", "^has_journal"])
            };
            if !label.is_empty() {
                mkfs = mkfs.arg("-L").arg(label);
            }
            work.run(
                &mkfs
                    .arg("-d")
                    .arg(&dir)
                    .arg(&image)
                    .arg((bytes / BLOCK).to_string()),
            )?;

            let script = work.dir().join("normalize.debugfs");
            std::fs::write(&script, normalize_script(&paths, &tree.entries)?)
                .map_err(|e| fail(e.to_string()))?;
            let errors = Arc::new(Mutex::new(Vec::new()));
            let sink = errors.clone();
            work.run(
                &Tool::new(debugfs)
                    .env("E2FSPROGS_FAKE_TIME", FAKE_TIME)
                    .arg("-w")
                    .arg("-f")
                    .arg(&script)
                    .arg(&image)
                    .on_line(move |stream, line| {
                        // debugfs exits 0 after a failed command; its
                        // banner is the only line expected on stderr.
                        if stream == Stream::Stderr
                            && !line.trim().is_empty()
                            && !line.starts_with("debugfs ")
                        {
                            sink.lock().expect("lock").push(line.to_string());
                        }
                    }),
            )?;
            let errors = errors.lock().expect("lock");
            if !errors.is_empty() {
                return Err(fail(format!("debugfs: {}", errors.join("; "))));
            }
            std::fs::remove_file(&script).map_err(|e| fail(e.to_string()))
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

/// One path of the tree: its file type bits, its stored mode, and its
/// size when it is a file.
struct Found {
    kind: u32,
    mode: u32,
    len: Option<u64>,
}

fn walk(root: &Path) -> std::io::Result<BTreeMap<String, Found>> {
    fn go(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Found>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let meta = entry.metadata()?;
            let kind = meta.mode() & 0o170000;
            let found = Found {
                kind,
                mode: meta.mode() & 0o7777,
                len: meta.is_file().then_some(meta.len()),
            };
            out.insert(path.clone(), found);
            if meta.is_dir() {
                go(&entry.path(), &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    go(root, "", &mut out)?;
    Ok(out)
}

fn inode_count(paths: usize) -> u64 {
    paths as u64 * 2 + 64
}

fn journal_bytes(image: u64) -> u64 {
    (image / 64).clamp(4 * MIB, 128 * MIB) / MIB * MIB
}

/// The content, inode tables, overhead and journal, plus `margin`, in MiB.
fn auto_bytes(paths: &BTreeMap<String, Found>, margin: u64) -> u64 {
    let content: u64 = BLOCK
        + paths
            .values()
            .map(|f| f.len.map_or(BLOCK, |len| len.div_ceil(BLOCK) * BLOCK))
            .sum::<u64>();
    let base = content + inode_count(paths.len()) * INODE_BYTES + OVERHEAD + margin;
    (base + journal_bytes(base)).div_ceil(MIB) * MIB
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

/// `set_inode_field` lines: every inode's owner and mode from the entries
/// (root and the stored mode otherwise), every time 0.
fn normalize_script(
    paths: &BTreeMap<String, Found>,
    entries: &[FileEntry],
) -> Result<String, StepError> {
    let by_path: BTreeMap<&str, &FileEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut script = String::new();
    let mut set = |path: &str, uid: u32, gid: u32, mode: Option<u32>| {
        let mut fields = vec![("uid", uid.to_string()), ("gid", gid.to_string())];
        if let Some(mode) = mode {
            fields.push(("mode", format!("0{mode:o}")));
        }
        for time in ["atime", "mtime", "ctime", "crtime"] {
            fields.push((time, "0".into()));
        }
        for (field, value) in fields {
            script.push_str(&format!("set_inode_field \"/{path}\" {field} {value}\n"));
        }
    };
    set("", 0, 0, None);
    set("lost+found", 0, 0, None);
    for (path, found) in paths {
        if path.contains(['"', '\\', '\n', '\r', '\0']) {
            return Err(StepError::body(
                NAME,
                format!("{path:?}: a quote, backslash or control character debugfs cannot name"),
            ));
        }
        let (uid, gid, perm) = match by_path.get(path.as_str()) {
            Some(e) => (e.uid, e.gid, e.mode & 0o7777),
            None => (0, 0, found.mode),
        };
        // A symlink's mode is fixed; its owner is still set.
        let mode = (found.kind != 0o120000).then_some(found.kind | perm);
        set(path, uid, gid, mode);
    }
    Ok(script)
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FS_EXT4_NODE];
