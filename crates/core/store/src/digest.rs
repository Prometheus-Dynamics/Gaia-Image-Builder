//! Content digests of files and directory trees.
//!
//! A file's digest is the SHA-256 of its bytes, the value `sha256sum` prints.
//! A tree's digest is the SHA-256 of a listing of every entry under the root,
//! sorted by path: its kind, its path relative to the root, a directory's or
//! file's permission bits, a file's content digest and a link's target. The
//! listing never holds a timestamp, an owner, an inode or where the tree sits,
//! so the same tree digests the same in any directory on any machine. The
//! root directory's own mode is left out: it belongs to wherever the tree was
//! made, not to the tree.

use crate::{Error, Result};
use sha2::{Digest as _, Sha256};
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Bumped when the tree listing's encoding changes, which changes every tree
/// digest.
const TREE_ENCODING: &[u8] = b"gaia-tree\0v1\0";

const READ_BUFFER: usize = 64 * 1024;

/// A SHA-256 digest. Written as `sha256:<64 hex digits>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

impl Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64 lowercase hex digits, without the `sha256:` prefix.
    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        out
    }

    /// Reads `sha256:<hex>` or bare `<hex>`.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix("sha256:").unwrap_or(text);
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in hex.as_bytes().chunks(2).enumerate() {
            bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Some(Self(bytes))
    }

    /// The digest of `bytes`.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The digest of a regular file's bytes, read in place.
pub fn file_digest(path: &Path) -> Result<Digest> {
    let mut file = File::open(path).map_err(|e| Error::io("read", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_BUFFER];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| Error::io("read", path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Digest(hasher.finalize().into()))
}

/// The digest of the directory tree at `root` (see the module docs). A
/// device, socket or pipe in the tree is an error: it cannot be stored.
pub fn tree_digest(root: &Path) -> Result<Digest> {
    let mut entries = Vec::new();
    walk(root, &mut Vec::new(), &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    hasher.update(TREE_ENCODING);
    for entry in &entries {
        let field = |hasher: &mut Sha256, bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        match &entry.kind {
            Entry::Dir { mode } => {
                hasher.update(b"d");
                field(&mut hasher, &entry.path);
                hasher.update(mode.to_le_bytes());
            }
            Entry::File { mode, digest } => {
                hasher.update(b"f");
                field(&mut hasher, &entry.path);
                hasher.update(mode.to_le_bytes());
                hasher.update(digest.as_bytes());
            }
            Entry::Link { target } => {
                hasher.update(b"l");
                field(&mut hasher, &entry.path);
                field(&mut hasher, target);
            }
        }
    }
    Ok(Digest(hasher.finalize().into()))
}

struct Listed {
    /// Relative path, components joined by `/`.
    path: Vec<u8>,
    kind: Entry,
}

enum Entry {
    Dir { mode: u32 },
    File { mode: u32, digest: Digest },
    Link { target: Vec<u8> },
}

fn walk(dir: &Path, prefix: &mut Vec<u8>, out: &mut Vec<Listed>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| Error::io("list", dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("list", dir, e))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| Error::io("stat", &path, e))?;
        let start = prefix.len();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(entry.file_name().as_bytes());
        let mode = metadata.permissions().mode() & 0o7777;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            out.push(Listed {
                path: prefix.clone(),
                kind: Entry::Dir { mode },
            });
            walk(&path, prefix, out)?;
        } else if file_type.is_file() {
            out.push(Listed {
                path: prefix.clone(),
                kind: Entry::File {
                    mode,
                    digest: file_digest(&path)?,
                },
            });
        } else if file_type.is_symlink() {
            let target = fs::read_link(&path).map_err(|e| Error::io("read link", &path, e))?;
            out.push(Listed {
                path: prefix.clone(),
                kind: Entry::Link {
                    target: target.as_os_str().as_bytes().to_vec(),
                },
            });
        } else {
            return Err(Error::Unsupported(path));
        }
        prefix.truncate(start);
    }
    Ok(())
}
