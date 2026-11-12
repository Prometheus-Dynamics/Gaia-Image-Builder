//! `gaia.os.buildroot.downloads`: the download directory Buildroot reads,
//! filled from the lock.
//!
//! Buildroot finds a package's source at `<download dir>/<package>/<file>`
//! and checks it against the hash in the package's `.hash` file. So a
//! locked download is laid out at `<package>/<file name of its URL>`, and its
//! digest is the lock's. Each file is copied from the store, where its
//! download step put it, and read back and checked before it is named in
//! the directory. A file already there with the right digest is kept, so a
//! second fill copies nothing.
//!
//! The directory is a host path the description names, shared by the
//! products that use it, as ccache's is. It is not a store object: a store
//! object is never written to, and Buildroot may write to its download
//! directory. The store is still the source of every file, and each file is
//! checked against its digest every time.

use daedalus::prelude::*;
use gaia_lock::Entry;
use gaia_step::{StepError, Store};
use gaia_store::{Digest, Kind, file_digest};
use gaia_types::Artifact;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

const NAME: &str = "gaia.os.buildroot.downloads";

/// One file the download directory holds: where it goes, what it must be,
/// and the store object that holds it.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:buildroot-download")]
pub struct LockedFile {
    /// Under the download directory: `<package>/<file name>`.
    pub path: String,
    /// The lock's digest, `sha256:<hex>`.
    pub digest: String,
    /// The file in the store.
    pub artifact: Artifact,
}

/// What a fill did: the directory, and how many files were copied in (the
/// rest were already there with the right digest).
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:buildroot-downloads")]
pub struct Filled {
    pub dir: String,
    pub copied: u32,
}

/// The locked file for `entry`, a download, laid out for the Buildroot
/// package directory `package` (the package's name, without `host-`).
pub fn locked_file(package: &str, entry: &Entry, artifact: Artifact) -> Result<LockedFile, String> {
    let Entry::Download { url, digest } = entry else {
        return Err(format!(
            "{package}: the lock entry is a {}, not a download",
            entry.kind()
        ));
    };
    if package.is_empty() || package.contains(['/', '\\']) || package.starts_with('.') {
        return Err(format!("{package:?} is not a package directory name"));
    }
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let file = without_query.rsplit('/').next().unwrap_or("");
    if file.is_empty() {
        return Err(format!("{package}: {url} names no file"));
    }
    Ok(LockedFile {
        path: format!("{package}/{file}"),
        digest: digest.clone(),
        artifact,
    })
}

/// Fills the download directory `dir` (an absolute path) from `files`.
#[node(
    id = "gaia.os.buildroot.downloads",
    inputs(store, dir, files),
    outputs(filled)
)]
pub fn os_buildroot_downloads(
    store: &str,
    dir: &str,
    files: Vec<LockedFile>,
) -> Result<Filled, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    if !Path::new(dir).is_absolute() {
        return Err(fail(format!("download directory {dir:?} is not absolute")));
    }
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let copied = fill(&store, Path::new(dir), &files).map_err(fail)?;
    Ok(Filled {
        dir: dir.to_string(),
        copied,
    })
}

/// Makes `root` hold each of `files` at its path, copying from `store`.
/// Returns how many were copied. Every file is checked against its digest
/// after the copy, and a mismatch fails and names both digests.
pub fn fill(store: &Store, root: &Path, files: &[LockedFile]) -> Result<u32, String> {
    let mut copied = 0;
    for file in files {
        let relative = plain_relative(&file.path)?;
        let want = Digest::parse(&file.digest)
            .ok_or_else(|| format!("{}: {:?} is not a sha256 digest", file.path, file.digest))?;
        let object = file
            .artifact
            .object
            .object_ref()
            .ok_or_else(|| format!("{}: the artifact is not in the store", file.path))?;
        if object.kind != Kind::File {
            return Err(format!("{}: the artifact is not a file", file.path));
        }
        let target = root.join(&relative);
        if target.is_file() && file_digest(&target).map_err(|e| e.to_string())? == want {
            continue;
        }
        let parent = target
            .parent()
            .ok_or_else(|| format!("{}: no parent directory", file.path))?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let temporary = temporary_beside(&target);
        fs::copy(store.path(&object), &temporary)
            .map_err(|e| format!("{}: copy from the store: {e}", file.path))?;
        let got = file_digest(&temporary).map_err(|e| e.to_string())?;
        if got != want {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "{}: digest mismatch: the lock says {want}, the store file is {got}",
                file.path
            ));
        }
        fs::rename(&temporary, &target).map_err(|e| format!("{}: {e}", target.display()))?;
        copied += 1;
    }
    Ok(copied)
}

/// `path` as a relative path made of plain names only: no `..`, no root.
fn plain_relative(path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if plain {
        Ok(relative.to_path_buf())
    } else {
        Err(format!(
            "{path:?} is not a plain path under the download directory"
        ))
    }
}

fn temporary_beside(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_store::Store;
    use gaia_types::{ArtifactKind, StoreRef};

    fn put(store: &Store, bytes: &[u8]) -> (Artifact, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("source");
        fs::write(&path, bytes).expect("write");
        let object = store.put_file(&path).expect("into the store");
        let digest = format!("{}", object.digest);
        let artifact = Artifact {
            kind: ArtifactKind::parse("archive"),
            object: StoreRef::from(object),
        };
        (artifact, digest)
    }

    fn locked(path: &str, digest: &str, artifact: &Artifact) -> LockedFile {
        LockedFile {
            path: path.into(),
            digest: digest.into(),
            artifact: artifact.clone(),
        }
    }

    #[test]
    fn a_download_is_laid_out_by_its_url_file_name() {
        let entry = Entry::Download {
            url: "https://www.busybox.net/downloads/busybox-1.37.0.tar.bz2?x=1".into(),
            digest: "sha256:00".into(),
        };
        let artifact = Artifact {
            kind: ArtifactKind::parse("archive"),
            object: StoreRef::from(gaia_store::ObjectRef {
                kind: Kind::File,
                digest: Digest::of_bytes(b"x"),
            }),
        };
        let file = locked_file("busybox", &entry, artifact).expect("laid out");
        assert_eq!(file.path, "busybox/busybox-1.37.0.tar.bz2");
        let git = Entry::Git {
            url: "https://example.org/x.git".into(),
            reference: "main".into(),
            commit: "0".repeat(40),
        };
        let artifact = Artifact {
            kind: ArtifactKind::parse("archive"),
            object: StoreRef::from(gaia_store::ObjectRef {
                kind: Kind::File,
                digest: Digest::of_bytes(b"x"),
            }),
        };
        let why = locked_file("x", &git, artifact).expect_err("not a download");
        assert!(why.contains("not a download"), "{why}");
    }

    #[test]
    fn a_fill_copies_each_file_once_and_keeps_the_ones_already_right() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let (artifact, digest) = put(&store, b"busybox source");
        let files = [locked("busybox/busybox.tar", &digest, &artifact)];
        let root = dir.path().join("dl");
        assert_eq!(fill(&store, &root, &files).expect("first"), 1);
        assert_eq!(
            fs::read(root.join("busybox/busybox.tar")).expect("read"),
            b"busybox source"
        );
        assert_eq!(fill(&store, &root, &files).expect("second"), 0);
    }

    #[test]
    fn a_damaged_file_in_the_directory_is_copied_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let (artifact, digest) = put(&store, b"good bytes");
        let files = [locked("p/f", &digest, &artifact)];
        let root = dir.path().join("dl");
        fs::create_dir_all(root.join("p")).expect("mkdir");
        fs::write(root.join("p/f"), b"bad bytes").expect("damage");
        assert_eq!(fill(&store, &root, &files).expect("fill"), 1);
        assert_eq!(fs::read(root.join("p/f")).expect("read"), b"good bytes");
    }

    #[test]
    fn a_store_file_that_does_not_match_the_lock_fails_naming_both_digests() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let (artifact, _) = put(&store, b"something else");
        let want = format!("{}", Digest::of_bytes(b"the locked bytes"));
        let why = fill(
            &store,
            &dir.path().join("dl"),
            &[locked("p/f", &want, &artifact)],
        )
        .expect_err("mismatch");
        assert!(why.contains("digest mismatch"), "{why}");
        assert!(why.contains(&want), "{why}");
        assert!(!dir.path().join("dl/p/f").exists());
    }

    #[test]
    fn a_path_that_leaves_the_directory_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let (artifact, digest) = put(&store, b"x");
        for path in ["../escape", "/abs/path", "p/../../x", ""] {
            let why = fill(
                &store,
                &dir.path().join("dl"),
                &[locked(path, &digest, &artifact)],
            )
            .expect_err(path);
            assert!(why.contains("not a plain path"), "{why}");
        }
    }
}
