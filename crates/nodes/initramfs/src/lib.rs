//! `gaia.initramfs`: a stored tree as a reproducible newc cpio archive, the
//! format the kernel unpacks as its initramfs.
//!
//! The archive holds exactly the tree's paths and nothing the tree does not
//! declare: no implied parents, no device nodes, no build-host paths. Records
//! are ordered by path component, so a directory always comes before what is
//! in it. Every timestamp is 0, every inode number is its record's position,
//! and each path's uid, gid and mode come from the tree's entries (root and
//! the stored mode for a path not listed). Two hosts give the same bytes.
//!
//! Compression is not a knob here. The archive is the uncompressed output;
//! `gaia.compress.gzip`, `.xz` or `.zstd` takes it as an artifact and gives
//! `initramfs+gzip` and so on, which the kernel unpacks as it does any
//! compressed initramfs.
//!
//! The key is the tree's [`owned_digest`], so an owner or mode change remakes
//! the archive.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_tree::owned_digest;
use gaia_types::{Artifact, ArtifactKind, FileEntry, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.initramfs";
const TRAILER: &str = "TRAILER!!!";
/// The newc header: a 6-byte magic and 13 eight-digit hex fields.
const HEADER_BYTES: usize = 110;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// The tree as a newc archive, with the owners and modes its entries give.
#[node(id = "gaia.initramfs", inputs(store, tree), outputs(archive))]
pub fn initramfs(store: &str, tree: Tree, cx: &Ctx) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let owned = owned_digest(&tree).ok_or_else(|| body("a tree that is not in the store"))?;
    let root = tree
        .object
        .path_in(&store)
        .ok_or_else(|| body("a tree that is not in the store"))?;
    let key = KeyBuilder::new(NAME, 1)
        .config("format", "newc")
        .input("tree", owned)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("archive", Path::new("initramfs.cpio"))],
        |work| {
            let out = work.dir().join("initramfs.cpio");
            write_archive(&root, &tree.entries, &out).map_err(body)
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("initramfs"),
        object: made.outputs["archive"].into(),
    })
}

fn body(why: impl std::fmt::Display) -> StepError {
    StepError::body(NAME, why)
}

/// One record of the archive, planned from metadata before anything is written.
struct Record {
    name: String,
    mode: u32,
    uid: u32,
    gid: u32,
    body: Body,
}

enum Body {
    Empty,
    Link(Vec<u8>),
    File { path: PathBuf, size: u32 },
}

impl Body {
    fn len(&self) -> u64 {
        match self {
            Self::Empty => 0,
            Self::Link(target) => target.len() as u64,
            Self::File { size, .. } => u64::from(*size),
        }
    }
}

/// Writes the tree at `root` to `out`, owners and modes from `entries`.
fn write_archive(root: &Path, entries: &[FileEntry], out: &Path) -> Result<(), String> {
    let records = plan(root, entries)?;
    let file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut writer = BufWriter::new(file);
    for (index, record) in records.iter().enumerate() {
        write_record(&mut writer, index as u32 + 1, record)?;
    }
    let trailer = Record {
        name: TRAILER.to_string(),
        mode: 0,
        uid: 0,
        gid: 0,
        body: Body::Empty,
    };
    write_record(&mut writer, 0, &trailer)?;
    let file = writer
        .into_inner()
        .map_err(|e| format!("{}: {}", out.display(), e.error()))?;
    file.sync_all()
        .map_err(|e| format!("{}: {e}", out.display()))
}

/// Every path under `root`, with the record it becomes. An entry that names a
/// path the tree lacks is an error, not a silent gap.
fn plan(root: &Path, entries: &[FileEntry]) -> Result<Vec<Record>, String> {
    let mut found = Vec::new();
    walk(root, "", &mut found)?;
    let present: BTreeSet<&str> = found.iter().map(|(name, _, _)| name.as_str()).collect();
    let mut by_path: BTreeMap<&str, &FileEntry> = BTreeMap::new();
    for entry in entries {
        let path = entry.path.trim_start_matches('/').trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        if !present.contains(path) {
            return Err(format!("an entry names {path}, which is not in the tree"));
        }
        if by_path.insert(path, entry).is_some() {
            return Err(format!("two entries name {path}"));
        }
    }
    // Component order: a parent sorts before everything under it.
    found.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
    let mut records = Vec::with_capacity(found.len());
    for (name, path, meta) in found {
        let entry = by_path.get(name.as_str());
        let (uid, gid) = entry.map_or((0, 0), |e| (e.uid, e.gid));
        let stored = meta.permissions().mode() & 0o7777;
        let file_type = meta.file_type();
        let (mode, body) = if file_type.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|e| format!("{name}: {e}"))?;
            (
                S_IFLNK | 0o777,
                Body::Link(target.as_os_str().as_bytes().to_vec()),
            )
        } else if file_type.is_dir() {
            (
                S_IFDIR | entry.map_or(stored, |e| e.mode & 0o7777),
                Body::Empty,
            )
        } else if file_type.is_file() {
            let size = u32::try_from(meta.len())
                .map_err(|_| format!("{name} is larger than a newc archive can hold"))?;
            (
                S_IFREG | entry.map_or(stored, |e| e.mode & 0o7777),
                Body::File { path, size },
            )
        } else {
            return Err(format!("{name} is not a file, directory or symlink"));
        };
        records.push(Record {
            name,
            mode,
            uid,
            gid,
            body,
        });
    }
    Ok(records)
}

fn walk(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf, std::fs::Metadata)>,
) -> Result<(), String> {
    for item in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let item = item.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|n| format!("{}: not UTF-8", Path::new(&n).display()))?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = item.path();
        let meta = path
            .symlink_metadata()
            .map_err(|e| format!("{relative}: {e}"))?;
        let is_dir = meta.file_type().is_dir();
        out.push((relative.clone(), path.clone(), meta));
        if is_dir {
            walk(&path, &relative, out)?;
        }
    }
    Ok(())
}

/// Writes one record: the header and name, padded to four, then its data,
/// padded to four.
fn write_record<W: Write>(out: &mut W, ino: u32, record: &Record) -> Result<(), String> {
    let name_size = record.name.len() + 1;
    let data = record.body.len();
    let data = u32::try_from(data)
        .map_err(|_| format!("{} is larger than a newc archive can hold", record.name))?;
    write!(
        out,
        "070701{ino:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{data:08x}{:08x}{:08x}{:08x}{:08x}{name_size:08x}{:08x}",
        record.mode,
        record.uid,
        record.gid,
        1u32, // nlink: every record is its own inode
        0u32, // mtime
        0u32, // dev major
        0u32, // dev minor
        0u32, // rdev major
        0u32, // rdev minor
        0u32, // check
    )
    .map_err(|e| format!("cpio header: {e}"))?;
    out.write_all(record.name.as_bytes())
        .and_then(|()| out.write_all(&[0]))
        .map_err(|e| format!("cpio name: {e}"))?;
    pad(out, HEADER_BYTES + name_size)?;
    match &record.body {
        Body::Empty => {}
        Body::Link(target) => {
            out.write_all(target)
                .map_err(|e| format!("{}: {e}", record.name))?;
            pad(out, target.len())?;
        }
        Body::File { path, size } => {
            let mut source = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let copied =
                std::io::copy(&mut source, out).map_err(|e| format!("{}: {e}", path.display()))?;
            if copied != u64::from(*size) {
                return Err(format!("{} changed while it was archived", path.display()));
            }
            pad(out, *size as usize)?;
        }
    }
    Ok(())
}

fn pad<W: Write>(out: &mut W, written: usize) -> Result<(), String> {
    let padding = (4 - written % 4) % 4;
    out.write_all(&[0; 3][..padding])
        .map_err(|e| format!("cpio padding: {e}"))
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&INITRAMFS_NODE];
