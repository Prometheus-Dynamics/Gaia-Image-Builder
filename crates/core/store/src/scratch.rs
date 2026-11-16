//! Scratch directories: where a step writes before its outputs are
//! committed.

use crate::placement::RamClaim;
use crate::{Error, Kind, ObjectRef, Result, StepEntry, StepKey, Store, fsutil};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// One step's output directory. Committing moves the named outputs into the
/// store; dropping it without a commit removes everything in it.
///
/// A disk scratch directory lives inside the store (in its subvolume, when
/// it has one), so committing an output is a rename. A RAM work directory
/// (see [`crate::Store::scratch_for`]) holds its claim on the RAM budget
/// until it is dropped, and the outputs committed from it are copied to the
/// store, since the store's disk is another file system.
#[derive(Debug)]
pub struct Scratch<'s> {
    store: &'s Store,
    dir: PathBuf,
    claim: Option<RamClaim>,
}

impl<'s> Scratch<'s> {
    pub(crate) fn new(store: &'s Store) -> Result<Self> {
        let dir = store.root().join("scratch").join(fsutil::unique_name());
        fs::create_dir(&dir).map_err(|e| Error::io("create", &dir, e))?;
        Ok(Self {
            store,
            dir,
            claim: None,
        })
    }

    /// A scratch directory under `root` (in RAM), counted by `claim`.
    pub(crate) fn in_ram(store: &'s Store, root: &Path, claim: RamClaim) -> Result<Self> {
        fs::create_dir_all(root).map_err(|e| Error::io("create", root, e))?;
        let dir = root.join(fsutil::unique_name());
        fs::create_dir(&dir).map_err(|e| Error::io("create", &dir, e))?;
        Ok(Self {
            store,
            dir,
            claim: Some(claim),
        })
    }

    /// Whether this directory is in RAM.
    pub fn in_memory(&self) -> bool {
        self.claim.is_some()
    }

    /// The directory the step writes into.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Moves each named output (a path relative to the scratch directory, a
    /// regular file or a directory) into the store, and returns their
    /// references. The scratch directory is removed either way.
    pub fn commit(self, outputs: &[(&str, &Path)]) -> Result<BTreeMap<String, ObjectRef>> {
        self.commit_with(outputs, fsutil::rename)
    }

    /// [`Scratch::commit`], with the rename a move into the store starts with
    /// given (see `fsutil::place`).
    fn commit_with(
        self,
        outputs: &[(&str, &Path)],
        rename: fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<BTreeMap<String, ObjectRef>> {
        let mut found = Vec::with_capacity(outputs.len());
        for (name, relative) in outputs {
            let path = self.output_path(relative)?;
            let overlapping = found.iter().find(|(_, other, _): &&(&str, PathBuf, Kind)| {
                other.starts_with(&path) || path.starts_with(other)
            });
            if let Some((other, _, _)) = overlapping {
                return Err(Error::BadOutput {
                    output: relative.to_path_buf(),
                    reason: format!("overlaps output {other}"),
                });
            }
            let metadata = fs::symlink_metadata(&path).map_err(|e| Error::BadOutput {
                output: relative.to_path_buf(),
                reason: e.to_string(),
            })?;
            let kind = if metadata.is_file() {
                Kind::File
            } else if metadata.is_dir() {
                Kind::Tree
            } else {
                return Err(Error::BadOutput {
                    output: relative.to_path_buf(),
                    reason: "not a regular file or a directory".into(),
                });
            };
            found.push((*name, path, kind));
        }
        let mut refs = BTreeMap::new();
        for (name, path, kind) in found {
            let object = ObjectRef {
                kind,
                digest: kind.digest(&path)?,
            };
            // What was hashed a moment ago is what the store now holds, so
            // the first lookup need not read it all again. An object that
            // was already there was not read here, and gets no record.
            if fsutil::place(&path, &self.store.path(&object), rename)? {
                self.store.record_verified(&object);
            }
            refs.insert(name.to_string(), object);
        }
        Ok(refs)
    }

    /// Commits the outputs, then records them as `key`'s entry: the entry is
    /// written only once every output is in the store.
    pub fn commit_step(self, key: &StepKey, outputs: &[(&str, &Path)]) -> Result<StepEntry> {
        let store = self.store;
        let entry = StepEntry {
            outputs: self.commit(outputs)?,
        };
        store.record(key, &entry)?;
        Ok(entry)
    }

    pub(crate) fn commit_one(self, relative: &Path) -> Result<ObjectRef> {
        let refs = self.commit(&[("out", relative)])?;
        refs.into_values().next().ok_or_else(|| Error::BadOutput {
            output: relative.to_path_buf(),
            reason: "nothing committed".into(),
        })
    }

    /// Removes the scratch directory and everything in it.
    pub fn discard(self) {}

    fn output_path(&self, relative: &Path) -> Result<PathBuf> {
        let plain = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if !plain || relative.as_os_str().is_empty() {
            return Err(Error::BadOutput {
                output: relative.to_path_buf(),
                reason: "must be a plain path inside the step's output directory".into(),
            });
        }
        Ok(self.dir.join(relative))
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        // Nothing is kept in RAM after the step: a failed step's outputs go
        // with it, and its log is written elsewhere. A disk scratch is moved
        // to the store's trash instead: removing a tree on the store's disk
        // while other steps run would block them (see the module docs of
        // `lib`). The trash is emptied by `Store::retain`.
        let _ = if self.in_memory() {
            fsutil::remove_tree(&self.dir)
        } else {
            fsutil::bury(&self.dir, &self.store.root().join("trash"))
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{self, Mode, RamBudget, Room, Settings};
    use crate::{Check, Digest, Store, tree_digest};
    use std::os::unix::fs::PermissionsExt;

    const GIB: u64 = 1 << 30;
    /// What a rename across file systems fails with (EXDEV).
    const EXDEV: i32 = 18;

    fn cross_device(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(EXDEV))
    }

    fn budget() -> &'static RamBudget {
        Box::leak(Box::new(RamBudget::new()))
    }

    fn settings(mode: Mode, ram_max: u64) -> Settings {
        Settings { mode, ram_max }
    }

    fn roomy() -> Room {
        Room {
            mem_available: Some(100 * GIB),
            shm_free: Some(100 * GIB),
        }
    }

    /// A store at `base/store` and a RAM root under `base/shm`.
    fn store_in(base: &Path) -> (Store, PathBuf) {
        let store = Store::open(base.join("store")).expect("store");
        (store, base.join("shm"))
    }

    /// A small output tree with a read-only directory, an executable, and a
    /// link, as a build leaves them.
    fn output_tree(dir: &Path) {
        fs::create_dir_all(dir.join("bin")).expect("dirs");
        fs::write(dir.join("bin/tool"), b"#!/bin/sh\necho hi\n").expect("write");
        fs::set_permissions(dir.join("bin/tool"), fs::Permissions::from_mode(0o755))
            .expect("chmod");
        fs::create_dir_all(dir.join("ro")).expect("dirs");
        fs::write(dir.join("ro/data"), b"data").expect("write");
        std::os::unix::fs::symlink("../bin/tool", dir.join("bin/link")).expect("link");
        fs::set_permissions(dir.join("ro"), fs::Permissions::from_mode(0o555)).expect("chmod");
    }

    fn ram_scratch<'s>(store: &'s Store, shm: &Path, budget: &'static RamBudget) -> Scratch<'s> {
        let claim = budget.claim(GIB, 10 * GIB).expect("claim");
        let root = placement::ram_root(store.root(), shm);
        Scratch::in_ram(store, &root, claim).expect("ram scratch")
    }

    #[test]
    fn a_cross_device_commit_copies_the_tree_and_keeps_its_digest() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let scratch = ram_scratch(&store, &shm, budget);
        let source = scratch.path().join("out");
        output_tree(&source);
        let expected = tree_digest(&source).expect("digest");
        let made = scratch
            .commit_with(&[("out", Path::new("out"))], cross_device)
            .expect("commit");
        let object = &made["out"];
        assert_eq!(object.digest, expected);
        assert!(store.check(object, Check::Digest), "the copy is whole");
        assert_eq!(
            fs::read(store.path(object).join("bin/tool")).expect("read"),
            b"#!/bin/sh\necho hi\n"
        );
        assert!(store.path(object).join("bin/link").is_symlink());
        // No temporary name is left beside the object.
        let leftovers: Vec<_> = fs::read_dir(store.path(object).parent().expect("parent"))
            .expect("list")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "no temporary copies are left");
    }

    #[test]
    fn a_cross_device_file_commit_is_copied_too() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let scratch = ram_scratch(&store, &shm, budget);
        fs::write(scratch.path().join("image"), b"image bytes").expect("write");
        let made = scratch
            .commit_with(&[("image", Path::new("image"))], cross_device)
            .expect("commit");
        assert!(store.check(&made["image"], Check::Digest));
        assert_eq!(
            fs::read(store.path(&made["image"])).expect("read"),
            b"image bytes"
        );
    }

    #[test]
    fn a_ram_scratch_is_gone_after_a_successful_commit() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let scratch = ram_scratch(&store, &shm, budget);
        let dir = scratch.path().to_path_buf();
        output_tree(&dir.join("out"));
        assert_eq!(budget.in_use(), GIB);
        let entry = scratch
            .commit_step(
                &StepKey(Digest::of_bytes(b"ram")),
                &[("out", Path::new("out"))],
            )
            .expect("commit");
        assert!(entry.outputs.contains_key("out"));
        assert!(!dir.exists(), "the RAM work directory is removed");
        assert_eq!(budget.in_use(), 0, "its claim is returned");
    }

    #[test]
    fn a_ram_scratch_is_gone_after_a_failed_step() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let scratch = ram_scratch(&store, &shm, budget);
        let dir = scratch.path().to_path_buf();
        output_tree(&dir.join("out"));
        scratch.discard();
        assert!(!dir.exists(), "a dropped RAM work directory is removed");
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn a_ram_scratch_is_gone_after_a_refused_commit() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let scratch = ram_scratch(&store, &shm, budget);
        let dir = scratch.path().to_path_buf();
        output_tree(&dir.join("out"));
        let refused = scratch.commit_with(&[("out", Path::new("missing"))], cross_device);
        assert!(refused.is_err());
        assert!(!dir.exists(), "a refused commit removes the work directory");
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn enough_room_puts_an_auto_step_in_ram() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let scratch = store
            .scratch_with(
                4 * GIB,
                &settings(Mode::Auto, 60 * GIB),
                roomy(),
                &shm,
                budget(),
            )
            .expect("scratch");
        assert!(scratch.in_memory());
        assert!(scratch.path().starts_with(&shm));
    }

    #[test]
    fn too_little_memory_puts_an_auto_step_on_the_disk() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let tight = Room {
            mem_available: Some(20 * GIB),
            shm_free: Some(100 * GIB),
        };
        let scratch = store
            .scratch_with(
                8 * GIB,
                &settings(Mode::Auto, 60 * GIB),
                tight,
                &shm,
                budget(),
            )
            .expect("scratch");
        assert!(!scratch.in_memory());
        assert!(scratch.path().starts_with(store.root()));
    }

    #[test]
    fn disk_mode_is_always_the_disk() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let scratch = store
            .scratch_with(
                GIB,
                &settings(Mode::Disk, 60 * GIB),
                roomy(),
                &shm,
                budget(),
            )
            .expect("scratch");
        assert!(!scratch.in_memory());
        assert!(scratch.path().starts_with(store.root()));
    }

    #[test]
    fn ram_mode_is_ram_even_when_memory_looks_short() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let tight = Room {
            mem_available: Some(GIB),
            shm_free: Some(GIB),
        };
        let scratch = store
            .scratch_with(
                8 * GIB,
                &settings(Mode::Ram, 60 * GIB),
                tight,
                &shm,
                budget(),
            )
            .expect("scratch");
        assert!(scratch.in_memory());
    }

    #[test]
    fn concurrent_ram_steps_count_against_the_cap() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let cap = settings(Mode::Auto, 10 * GIB);
        let first = store
            .scratch_with(6 * GIB, &cap, roomy(), &shm, budget)
            .expect("first");
        assert!(first.in_memory());
        // 6 + 6 is over the 10 GiB cap: the second step goes to the disk.
        let second = store
            .scratch_with(6 * GIB, &cap, roomy(), &shm, budget)
            .expect("second");
        assert!(!second.in_memory());
        // Forced RAM over the cap is an error, not a silent disk.
        let forced = settings(Mode::Ram, 10 * GIB);
        let refused = store.scratch_with(6 * GIB, &forced, roomy(), &shm, budget);
        assert!(matches!(refused, Err(Error::Work { .. })));
        drop(first);
        let third = store
            .scratch_with(6 * GIB, &cap, roomy(), &shm, budget)
            .expect("third");
        assert!(third.in_memory(), "the first step's claim went back");
    }

    #[test]
    fn a_step_over_the_cap_takes_the_disk() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, shm) = store_in(base.path());
        let budget = budget();
        let big = store
            .scratch_with(
                12 * GIB,
                &settings(Mode::Auto, 10 * GIB),
                roomy(),
                &shm,
                budget,
            )
            .expect("scratch");
        assert!(!big.in_memory(), "a step over the cap never takes RAM");
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn a_dropped_disk_scratch_goes_to_the_trash() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_in(base.path());
        let scratch = store.scratch().expect("scratch");
        let dir = scratch.path().to_path_buf();
        output_tree(&dir.join("out"));
        scratch.discard();
        assert!(!dir.exists(), "the dropped scratch is gone from its place");
        let trash = store.root().join("trash");
        assert_eq!(fs::read_dir(&trash).expect("trash").count(), 1);
        store.retain(&crate::Keep::default(), true).expect("retain");
        assert_eq!(fs::read_dir(&trash).expect("trash").count(), 0);
    }

    #[test]
    fn a_commit_of_an_object_already_there_leaves_its_copy_to_the_drop() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_in(base.path());
        let first = store.scratch().expect("scratch");
        output_tree(&first.path().join("out"));
        first.commit(&[("out", Path::new("out"))]).expect("first");
        let second = store.scratch().expect("scratch");
        let dir = second.path().to_path_buf();
        output_tree(&dir.join("out"));
        let made = second.commit(&[("out", Path::new("out"))]).expect("second");
        assert!(store.check(&made["out"], Check::Digest));
        assert!(!dir.exists(), "the drop removes what the commit left");
    }

    #[test]
    fn a_new_store_is_a_subvolume_where_its_parent_is_btrfs() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_in(base.path());
        assert_eq!(
            fsutil::is_subvolume(store.root()),
            fsutil::on_btrfs(base.path())
        );
        // Everything a step needs is there either way.
        let scratch = store.scratch().expect("scratch");
        output_tree(&scratch.path().join("out"));
        let made = scratch
            .commit(&[("out", Path::new("out"))])
            .expect("commit");
        assert!(store.check(&made["out"], Check::Digest));
    }

    #[test]
    fn a_store_on_a_file_system_without_subvolumes_is_a_directory() {
        // /dev/shm is tmpfs: never btrfs.
        let Ok(shm) = tempfile::Builder::new().tempdir_in("/dev/shm") else {
            return;
        };
        let store = Store::open(shm.path().join("store")).expect("store");
        assert!(store.root().is_dir());
        assert!(!fsutil::is_subvolume(store.root()));
        let scratch = store.scratch().expect("scratch");
        output_tree(&scratch.path().join("out"));
        assert!(scratch.commit(&[("out", Path::new("out"))]).is_ok());
    }

    #[test]
    fn a_dead_process_scratch_is_swept_to_the_trash_and_emptied() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_in(base.path());
        // A pid no process has (above the kernel's pid limit).
        let dead = store.root().join("scratch").join("999999999-0-1");
        output_tree(&dead.join("out"));
        let dry = store.retain(&crate::Keep::default(), false).expect("dry");
        assert!(dead.exists(), "a dry run removes nothing");
        assert_eq!(dry.leftovers_removed, 1);
        let applied = store.retain(&crate::Keep::default(), true).expect("apply");
        assert!(!dead.exists());
        assert_eq!(applied.leftovers_removed, 1, "counted once, not twice");
        let trash = store.root().join("trash");
        assert_eq!(fs::read_dir(&trash).expect("trash").count(), 0);
    }

    #[test]
    fn a_disk_scratch_is_still_removed_after_commit() {
        let base = tempfile::tempdir().expect("tempdir");
        let (store, _) = store_in(base.path());
        let scratch = store.scratch().expect("scratch");
        let dir = scratch.path().to_path_buf();
        output_tree(&dir.join("out"));
        scratch
            .commit(&[("out", Path::new("out"))])
            .expect("commit");
        assert!(!dir.exists());
    }
}
