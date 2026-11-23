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
//! Three settings are for a filesystem that grows on the device (a data
//! partition): `inode_ratio` (`-i`, bytes per inode; 0 gives the inode count
//! the tree needs, `-N`), `features` (`-O`, a comma list such as
//! `meta_bg,^resize_inode`; empty for mke2fs's defaults) and `journal_size`
//! (`-J size=`, in MiB; 0 is 1/64 of the image, 4 to 128 MiB).
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
/// journal (`journal_size` MiB, or 1/64 of the image, 4 to 128 MiB, when it
/// is 0). `inode_ratio` and `features` are mke2fs's `-i` and `-O` (see the
/// module docs).
#[node(
    id = "gaia.fs.ext4",
    inputs(
        store,
        mke2fs,
        debugfs,
        tree,
        size,
        margin,
        label,
        journal,
        inode_ratio,
        features,
        journal_size
    ),
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
    inode_ratio: u64,
    features: &str,
    journal_size: u64,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
    let options = Options::new(journal, inode_ratio, features, journal_size).map_err(fail)?;
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
    let key = KeyBuilder::new(NAME, 2)
        .config("mke2fs", version(mke2fs)?)
        .config("debugfs", version(debugfs)?)
        .config("size", size.to_string())
        .config("margin", margin.to_string())
        .config("label", label)
        .config("journal", if journal { "yes" } else { "no" })
        .config("inode ratio", inode_ratio.to_string())
        .config("features", features)
        .config("journal size", journal_size.to_string())
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
                auto_bytes(&paths, margin, &options)
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
            if let Some(ratio) = options.inode_ratio {
                let inodes = bytes / ratio;
                let needed = inode_count(paths.len());
                if inodes < needed {
                    return Err(fail(format!(
                        "{bytes} bytes at one inode per {ratio} bytes is {inodes} inodes; the tree needs {needed}"
                    )));
                }
            }
            let mut mkfs = Tool::new(mke2fs)
                .env("E2FSPROGS_FAKE_TIME", FAKE_TIME)
                .args([
                    "-q", "-F", "-t", "ext4", "-b", "4096", "-I", "256", "-m", "0",
                ])
                .args(options.mke2fs_args(paths.len(), bytes))
                .arg("-U")
                .arg(&uuid)
                .arg("-E")
                .arg(format!("hash_seed={uuid},root_owner=0:0"));
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
            let values = work.dir().join("xattrs");
            std::fs::create_dir(&values).map_err(|e| fail(e.to_string()))?;
            std::fs::write(&script, normalize_script(&paths, &tree.entries, &values)?)
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
            std::fs::remove_file(&script).map_err(|e| fail(e.to_string()))?;
            std::fs::remove_dir_all(&values).map_err(|e| fail(e.to_string()))
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

/// The settings mke2fs takes beyond the fixed ones, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    journal: bool,
    /// `-i`: bytes per inode, or `None` for the tree's count (`-N`).
    inode_ratio: Option<u64>,
    /// `-O` words, the journal's `^has_journal` among them when it has none.
    features: Vec<String>,
    /// `-J size=` in MiB, or `None` for 1/64 of the image.
    journal_mib: Option<u64>,
}

impl Options {
    fn new(
        journal: bool,
        inode_ratio: u64,
        features: &str,
        journal_size: u64,
    ) -> Result<Options, String> {
        // mke2fs's own bounds: 1 KiB to 64 MiB, and not below a block.
        if inode_ratio != 0 && !(BLOCK..=64 * MIB).contains(&inode_ratio) {
            return Err(format!(
                "inode_ratio {inode_ratio} is outside {BLOCK} to {} bytes per inode (0 sizes it to the tree)",
                64 * MIB
            ));
        }
        let mut words = Vec::new();
        for word in features.split(',').filter(|w| !w.is_empty()) {
            let name = word.strip_prefix('^').unwrap_or(word);
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(format!(
                    "feature {word:?} is not an ext4 feature name (letters, digits and _, `^` to turn one off)"
                ));
            }
            if name == "has_journal" {
                return Err(format!(
                    "feature {word:?}: the journal is the `journal` setting"
                ));
            }
            words.push(word.to_string());
        }
        if !journal {
            words.push("^has_journal".into());
        }
        if journal_size != 0 && !journal {
            return Err(format!(
                "journal_size {journal_size} MiB for a filesystem with no journal"
            ));
        }
        if journal_size != 0 && !(1..=10240).contains(&journal_size) {
            return Err(format!(
                "journal_size {journal_size} MiB is outside mke2fs's 1 to 10240 MiB"
            ));
        }
        Ok(Options {
            journal,
            inode_ratio: (inode_ratio != 0).then_some(inode_ratio),
            features: words,
            journal_mib: (journal_size != 0).then_some(journal_size),
        })
    }

    /// The journal's bytes in an image of `image` bytes: none without one.
    fn journal_bytes(&self, image: u64) -> u64 {
        match (self.journal, self.journal_mib) {
            (false, _) => 0,
            (true, Some(mib)) => mib * MIB,
            (true, None) => journal_bytes(image),
        }
    }

    /// mke2fs's inode, feature and journal words for a tree of `paths`
    /// entries in `bytes`.
    fn mke2fs_args(&self, paths: usize, bytes: u64) -> Vec<String> {
        let mut args = match self.inode_ratio {
            Some(ratio) => vec!["-i".to_string(), ratio.to_string()],
            None => vec!["-N".to_string(), inode_count(paths).to_string()],
        };
        if !self.features.is_empty() {
            args.push("-O".into());
            args.push(self.features.join(","));
        }
        if self.journal {
            args.push("-J".into());
            args.push(format!("size={}", self.journal_bytes(bytes) / MIB));
        }
        args
    }
}

fn inode_count(paths: usize) -> u64 {
    paths as u64 * 2 + 64
}

fn journal_bytes(image: u64) -> u64 {
    (image / 64).clamp(4 * MIB, 128 * MIB) / MIB * MIB
}

/// The content, inode tables, overhead and journal, plus `margin`, in MiB.
/// With an inode ratio, the inode tables grow with the image: one inode of
/// `INODE_BYTES` per `ratio` bytes of it.
fn auto_bytes(paths: &BTreeMap<String, Found>, margin: u64, options: &Options) -> u64 {
    let content: u64 = BLOCK
        + paths
            .values()
            .map(|f| f.len.map_or(BLOCK, |len| len.div_ceil(BLOCK) * BLOCK))
            .sum::<u64>();
    let base = match options.inode_ratio {
        None => content + inode_count(paths.len()) * INODE_BYTES + OVERHEAD + margin,
        Some(ratio) => {
            let rest = content + OVERHEAD + margin;
            (rest * ratio).div_ceil(ratio - INODE_BYTES)
        }
    };
    (base + options.journal_bytes(base)).div_ceil(MIB) * MIB
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
/// (root and the stored mode otherwise), every time 0; then an `ea_set` line
/// for each extended attribute an entry gives, its value written to a file
/// in `values` (debugfs reads it with `-f`).
fn normalize_script(
    paths: &BTreeMap<String, Found>,
    entries: &[FileEntry],
    values: &Path,
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
    // The attributes, after the owners: the first loop's closure holds the
    // script until its last line.
    let mut count = 0_usize;
    for path in paths.keys() {
        let Some(entry) = by_path.get(path.as_str()) else {
            continue;
        };
        for (name, value) in &entry.xattrs {
            if name.is_empty() || name.contains(['"', '\\', '\n', '\r', '\0', ' ']) {
                return Err(StepError::body(
                    NAME,
                    format!("{path}: the attribute {name:?} cannot be named to debugfs"),
                ));
            }
            let file = values.join(count.to_string());
            std::fs::write(&file, value)
                .map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?;
            script.push_str(&format!(
                "ea_set -f \"{}\" \"/{path}\" {name}\n",
                file.display()
            ));
            count += 1;
        }
    }
    Ok(script)
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FS_EXT4_NODE];
