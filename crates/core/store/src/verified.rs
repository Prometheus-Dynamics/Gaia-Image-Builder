//! A record that an object's content was read and matched its digest, so a
//! later lookup does not read it all again.
//!
//! Hashing a large tree (a kernel's source) takes minutes; a lookup that
//! does it every time is not a lookup. When [`Check::Digest`](crate::Check)
//! has hashed an object and it matched, the store records the object's
//! *state*: a hash over every entry's path, type, size, mode and
//! modification time, read with `stat` alone. The next check takes the
//! state again, which reads no file, and accepts the object when the state
//! is the one recorded. Any other state means the object was touched since,
//! and it is hashed in full again.
//!
//! What this trusts: that a file's content does not change without its size
//! or modification time changing. A write through the filesystem moves the
//! time; damage beneath the filesystem does not, and is not seen until the
//! record is gone. The store's objects are written once and never edited,
//! so this is the check that fits them. Removing `verified/` makes every
//! object hash again.

use crate::{ObjectRef, Store};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const STATE_ENCODING: &[u8] = b"gaia-state\0v1\0";

/// The state of what is at `path`: a hash over each entry's relative path,
/// type, size, mode and modification time, sorted by path. No file is read.
pub(crate) fn state(path: &Path) -> io::Result<String> {
    let mut entries: Vec<(Vec<u8>, [u8; 33])> = Vec::new();
    walk(path, &mut Vec::new(), &mut entries)?;
    entries.sort();
    let mut hasher = Sha256::new();
    hasher.update(STATE_ENCODING);
    for (name, fields) in &entries {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name);
        hasher.update(fields);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn walk(path: &Path, name: &mut Vec<u8>, out: &mut Vec<(Vec<u8>, [u8; 33])>) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let kind = if meta.is_dir() {
        b'd'
    } else if meta.file_type().is_symlink() {
        b'l'
    } else {
        b'f'
    };
    let mut fields = [0u8; 33];
    fields[0] = kind;
    // A directory's own size and time move when an entry is added or
    // removed, and its entries are listed anyway: its mode is what counts.
    let (size, sec, nsec) = if kind == b'd' {
        (0, 0, 0)
    } else {
        (meta.size(), meta.mtime(), meta.mtime_nsec())
    };
    fields[1..9].copy_from_slice(&size.to_le_bytes());
    fields[9..17].copy_from_slice(&sec.to_le_bytes());
    fields[17..25].copy_from_slice(&nsec.to_le_bytes());
    fields[25..33].copy_from_slice(&u64::from(meta.mode()).to_le_bytes());
    out.push((name.clone(), fields));
    if kind == b'd' {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let len = name.len();
            if len > 0 {
                name.push(b'/');
            }
            name.extend_from_slice(entry.file_name().as_bytes());
            walk(&entry.path(), name, out)?;
            name.truncate(len);
        }
    }
    Ok(())
}

impl Store {
    fn verified_path(&self, object: &ObjectRef) -> PathBuf {
        let hex = object.digest.hex();
        self.root()
            .join("verified")
            .join(object.kind.dir_name())
            .join(&hex[..2])
            .join(hex)
    }

    /// Whether `object` is in the state recorded when its digest last
    /// matched. False with no record, or when anything was touched since.
    pub(crate) fn still_verified(&self, object: &ObjectRef) -> bool {
        let Ok(recorded) = fs::read_to_string(self.verified_path(object)) else {
            return false;
        };
        state(&self.path(object)).is_ok_and(|now| now == recorded.trim())
    }

    /// Records `object`'s state, after its digest matched. A record that
    /// cannot be written costs only the next lookup its time.
    pub(crate) fn record_verified(&self, object: &ObjectRef) {
        let Ok(now) = state(&self.path(object)) else {
            return;
        };
        let path = self.verified_path(object);
        let Some(dir) = path.parent() else { return };
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = dir.join(format!(".{}.{}", object.digest.hex(), std::process::id()));
        if fs::write(&temp, now).is_ok() && fs::rename(&temp, &path).is_err() {
            let _ = fs::remove_file(&temp);
        }
    }
}
