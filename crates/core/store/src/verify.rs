//! `gaia store verify`: every object read in full and compared with its
//! digest. Lookups do not do this (see `verified`: a lookup accepts an object
//! whose size, mode and modification time are unchanged, even if its content
//! was rewritten). This is the check that does.

use crate::retain::children_two_deep;
use crate::{Digest, Kind, ObjectRef, Result, Store, file_digest, fsutil, tree_digest};
use std::fs;
use std::path::{Path, PathBuf};

/// What [`Store::verify`] found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Objects read in full.
    pub checked: usize,
    /// Objects whose content matches their name.
    pub verified: usize,
    /// Objects that do not match, and entries in an object directory that are
    /// not an object.
    pub problems: Vec<Problem>,
    /// Problems removed, with `fix`.
    pub removed: usize,
}

/// One object, or entry in an object directory, that is not what its name
/// says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub reason: String,
}

impl Store {
    /// Reads every object in the store and compares its content with its
    /// name. An object that matches gets its record written (see `verified`),
    /// so the next lookup of it reads it no more than before. One that does
    /// not is reported with its path. With `fix`, it is removed with its
    /// record, so the next build makes it again.
    ///
    /// Run it alone, as [`Store::retain`]: a step committing meanwhile could
    /// be reading an object this removes.
    pub fn verify(&self, fix: bool) -> Result<VerifyReport> {
        let mut report = VerifyReport::default();
        for kind in [Kind::File, Kind::Tree] {
            let dir = self.root().join("objects").join(kind.dir_name());
            for (name, path) in children_two_deep(&dir)? {
                let object = Digest::parse(&name).map(|digest| ObjectRef { kind, digest });
                let outcome = match object {
                    Some(object) => {
                        report.checked += 1;
                        self.verify_one(&object, &path)
                    }
                    None => Err(format!("not an object name in {}", kind.dir_name())),
                };
                match outcome {
                    Ok(()) => report.verified += 1,
                    Err(reason) => {
                        if fix {
                            fsutil::remove_tree(&path)?;
                            if let Some(object) = object {
                                self.remove_record(&object);
                            }
                            report.removed += 1;
                        }
                        report.problems.push(Problem { path, reason });
                    }
                }
            }
        }
        Ok(report)
    }

    /// Reads one object, and records it as verified when its content matches.
    fn verify_one(&self, object: &ObjectRef, path: &Path) -> std::result::Result<(), String> {
        let metadata = fs::symlink_metadata(path).map_err(|e| format!("cannot stat: {e}"))?;
        let actual = match object.kind {
            Kind::File => {
                if !metadata.is_file() {
                    return Err("not a regular file".into());
                }
                file_digest(path)
            }
            Kind::Tree => {
                if !metadata.is_dir() {
                    return Err("not a directory".into());
                }
                tree_digest(path)
            }
        }
        .map_err(|e| e.to_string())?;
        if actual != object.digest {
            return Err(format!("the content is {actual}"));
        }
        self.record_verified(object);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Check, StepKey};
    use std::fs::{File, FileTimes};

    /// Rewrites `path` with `bytes` of the same length, and puts its
    /// modification time back: what a size-and-mtime check cannot see.
    fn rewrite_unseen(path: &Path, bytes: &[u8]) {
        let before = fs::metadata(path).expect("meta").modified().expect("mtime");
        assert_eq!(fs::metadata(path).expect("meta").len(), bytes.len() as u64);
        fs::write(path, bytes).expect("write");
        File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(before))
            .expect("mtime back");
    }

    fn store_with_file(dir: &Path, text: &str) -> (Store, ObjectRef) {
        let store = Store::open(dir.join("store")).expect("open");
        let src = dir.join("src");
        fs::write(&src, text).expect("write");
        let object = ObjectRef {
            kind: Kind::File,
            digest: file_digest(&src).expect("digest"),
        };
        assert_eq!(store.put_file(&src).expect("put"), object);
        (store, object)
    }

    fn store_with_tree(dir: &Path) -> (Store, ObjectRef) {
        let store = Store::open(dir.join("store")).expect("open");
        let src = dir.join("src");
        fs::create_dir_all(src.join("sub")).expect("dir");
        fs::write(src.join("sub/f"), b"content").expect("write");
        fs::write(src.join("g"), b"other").expect("write");
        let object = store.put_tree(&src).expect("put");
        (store, object)
    }

    #[test]
    fn a_clean_store_verifies_and_every_object_gets_its_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, file) = store_with_file(dir.path(), "one");
        let tree = store_with_tree_into(&store, dir.path());
        let report = store.verify(false).expect("verify");
        assert_eq!((report.checked, report.verified), (2, 2), "{report:?}");
        assert!(report.problems.is_empty());
        assert!(store.verified_path(&file).is_file(), "file record");
        assert!(store.verified_path(&tree).is_file(), "tree record");
    }

    fn store_with_tree_into(store: &Store, dir: &Path) -> ObjectRef {
        let src = dir.join("tree-src");
        fs::create_dir_all(src.join("sub")).expect("dir");
        fs::write(src.join("sub/f"), b"content").expect("write");
        store.put_tree(&src).expect("put")
    }

    #[test]
    fn a_same_size_edit_with_its_mtime_restored_is_missed_by_lookup_and_caught_by_verify() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, object) = store_with_file(dir.path(), "original");
        assert!(store.check(&object, Check::Digest));
        let path = store.path(&object);
        rewrite_unseen(&path, b"tampered");
        // The blind spot the state check has: nothing it reads has changed.
        assert!(store.check(&object, Check::Digest), "a lookup accepts it");
        let report = store.verify(false).expect("verify");
        assert_eq!(report.verified, 0);
        assert_eq!(report.problems.len(), 1, "{report:?}");
        assert!(
            report.problems[0]
                .reason
                .starts_with("the content is sha256:")
        );
        assert_eq!(report.problems[0].path, path);
        assert!(path.is_file(), "reported, not removed");
    }

    #[test]
    fn one_edited_deep_file_in_a_tree_is_caught() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, tree) = store_with_tree(dir.path());
        assert!(store.check(&tree, Check::Digest));
        rewrite_unseen(&store.path(&tree).join("sub/f"), b"CONTENT");
        assert!(store.check(&tree, Check::Digest), "a lookup accepts it");
        let report = store.verify(false).expect("verify");
        assert_eq!((report.checked, report.verified), (1, 0), "{report:?}");
        assert_eq!(report.problems.len(), 1, "{report:?}");
        assert!(report.problems[0].path.ends_with(tree.digest.hex()));
    }

    #[test]
    fn a_name_that_is_no_digest_is_reported_and_fix_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_with_file(dir.path(), "one");
        let stray = store.root().join("objects/file/00/not-a-digest");
        fs::create_dir_all(stray.parent().expect("parent")).expect("dir");
        fs::write(&stray, b"x").expect("write");
        let report = store.verify(false).expect("verify");
        assert_eq!(report.problems.len(), 1, "{report:?}");
        assert_eq!(report.problems[0].path, stray);
        assert_eq!(report.problems[0].reason, "not an object name in file");
        assert!(stray.exists(), "reported, not removed");
        let fixed = store.verify(true).expect("verify and fix");
        assert_eq!(fixed.removed, 1);
        assert!(!stray.exists());
    }

    #[test]
    fn fix_removes_the_object_and_its_record_and_a_lookup_misses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("open");
        let key = StepKey(crate::Digest::of_bytes(b"step"));
        let scratch = store.scratch().expect("scratch");
        fs::write(scratch.path().join("bin"), b"binary").expect("write");
        let entry = scratch
            .commit_step(&key, &[("bin", Path::new("bin"))])
            .expect("commit");
        let object = entry.outputs["bin"];
        assert!(store.lookup(&key, Check::Digest).expect("lookup").is_some());
        rewrite_unseen(&store.path(&object), b"BINARX");
        let report = store.verify(true).expect("verify and fix");
        assert_eq!(
            (report.problems.len(), report.removed),
            (1, 1),
            "{report:?}"
        );
        assert!(!store.path(&object).exists(), "removed");
        assert!(!store.verified_path(&object).exists(), "its record too");
        assert!(store.lookup(&key, Check::Digest).expect("lookup").is_none());
        assert!(store.verify(false).expect("verify").problems.is_empty());
    }

    #[test]
    fn retention_removes_the_record_with_the_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, object) = store_with_file(dir.path(), "gone");
        assert!(store.check(&object, Check::Digest));
        assert!(store.verified_path(&object).is_file());
        store.retain(&crate::Keep::default(), true).expect("retain");
        assert!(!store.path(&object).exists());
        assert!(!store.verified_path(&object).exists());
    }

    #[test]
    fn verify_writes_the_record_of_an_object_that_lacks_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, object) = store_with_file(dir.path(), "content");
        fs::remove_dir_all(dir.path().join("store/verified")).expect("rm");
        let report = store.verify(false).expect("verify");
        assert_eq!(report.verified, 1);
        assert!(store.verified_path(&object).is_file(), "written again");
    }
}
