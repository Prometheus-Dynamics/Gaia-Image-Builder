//! Gaia's content store: the whole of what Gaia keeps between runs.
//!
//! - **Content by digest.** A file or a directory tree is stored whole under
//!   its digest ([`digest`]), and found again by its [`ObjectRef`].
//! - **Step entries.** A step's key maps to the references of the outputs
//!   that step made ([`StepEntry`]). An entry is written only after every
//!   output it names is in the store.
//!
//! A step writes into a [`Scratch`] directory. When it succeeds, its outputs
//! are moved into the store, each with one rename, so an object is either
//! whole or absent. When it fails, or the scratch is dropped, the directory is
//! removed. A process that dies mid-commit leaves only a scratch directory and
//! perhaps some objects no entry names: never an entry that names something
//! missing. [`Store::retain`] clears both, on request.
//!
//! Delete any part of the store and the next run remakes exactly that part.
//!
//! Layout under the root:
//!
//! ```text
//! objects/file/<2 hex>/<64 hex>     a file, by the digest of its bytes
//! objects/tree/<2 hex>/<64 hex>/    a tree, by its tree digest
//! entries/<2 hex>/<64 hex>.json     a step entry, by step key
//! scratch/<pid>-<n>/                steps at work
//! trash/<pid>-<n>-<nanos>/          what was dropped, awaiting removal
//! ```
//!
//! **Subvolumes.** On btrfs, a store created by [`Store::open`] is a btrfs
//! subvolume, not a plain directory. Everything in one btrfs subvolume shares
//! one file system tree and its locks, so a store that shares a subvolume
//! with other work trees waits on them for every metadata operation. A
//! subvolume has a tree of its own: the store's scratch writes, commits and
//! removals contend only with each other. Its scratch directories are plain
//! directories inside it, so a commit is a rename into `objects/`, which is
//! free. A store on another file system, or a store that already exists, is
//! a plain directory, and nothing else changes. See `fsutil::make_root`.
//!
//! **Trash.** Removing a tree of millions of small files takes an hour on a
//! busy disk, and blocks the steps around it. So a dropped scratch directory,
//! or an object or entry that [`Store::retain`] removes, is renamed into
//! `trash/` (instant, same subvolume), and [`Store::retain`] empties `trash/`
//! at the end of its run, when it is run alone. Deleting a btrfs subvolume
//! needs privilege, unless the mount has the `user_subvol_rm_allowed` option,
//! so a subvolume per scratch directory would not make its removal cheaper:
//! the rename into `trash/` is what makes it instant. The option is for the
//! machine's owner to decide; the store does not need it.

mod digest;
mod entry;
mod fsutil;
mod object;
mod placement;
mod retain;
mod scratch;
mod verified;
mod verify;

pub use digest::{Digest, file_digest, tree_digest};
pub use entry::{StepEntry, StepKey};
pub use fsutil::{copy_file, used_bytes};
pub use object::{Kind, ObjectRef};
pub use placement::{
    MemoryHold, Mode, RESERVE_BYTES, Room, Settings, held_memory, hold_memory, parse_mem_available,
    parse_size, ram_root,
};
pub use retain::{Keep, RetainReport};
pub use scratch::Scratch;
pub use verify::{Problem, VerifyReport};

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// A content store rooted at one directory. Any number of processes may
/// commit into the same store at once; [`Store::retain`] must run alone.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// How [`Store::lookup`] and [`Store::check`] decide an object is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The object's path exists and is of its kind. Cheap.
    Present,
    /// The object's content must match its digest. It is read in full the
    /// first time; after that it is accepted while its size, mode and
    /// modification times are the ones recorded then, and read again when
    /// any has changed.
    Digest,
}

impl Store {
    /// Opens the store at `root`, creating it when missing: as a btrfs
    /// subvolume when its parent is on btrfs (see the module docs), and its
    /// directories when they are missing.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fsutil::make_root(&root)?;
        for dir in [
            "objects/file",
            "objects/tree",
            "entries",
            "scratch",
            "trash",
        ] {
            let path = root.join(dir);
            fs::create_dir_all(&path).map_err(|e| Error::io("create", &path, e))?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `object` is, or would be, in the store.
    pub fn path(&self, object: &ObjectRef) -> PathBuf {
        let hex = object.digest.hex();
        self.root
            .join("objects")
            .join(object.kind.dir_name())
            .join(&hex[..2])
            .join(hex)
    }

    /// Whether `object` is in the store, checked as `check` says.
    pub fn check(&self, object: &ObjectRef, check: Check) -> bool {
        let path = self.path(object);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            return false;
        };
        let kind_matches = match object.kind {
            Kind::File => metadata.is_file(),
            Kind::Tree => metadata.is_dir(),
        };
        if !kind_matches {
            return false;
        }
        match check {
            Check::Present => true,
            // Read in full once, then trusted while nothing has touched it
            // (see `verified`).
            Check::Digest => {
                if self.still_verified(object) {
                    return true;
                }
                let matches = object.kind.digest(&path).ok() == Some(object.digest);
                if matches {
                    self.record_verified(object);
                }
                matches
            }
        }
    }

    /// A new scratch directory for one step's outputs, on the store's disk.
    pub fn scratch(&self) -> Result<Scratch<'_>> {
        Scratch::new(self)
    }

    /// A new scratch directory for a step that expects to write `expected`
    /// bytes. Whether it is in RAM is decided by `GAIA_WORK_DIR` and
    /// `GAIA_RAM_MAX` (see [`placement`]); otherwise it is on the disk, as
    /// [`Store::scratch`] makes it.
    pub fn scratch_for(&self, expected: u64) -> Result<Scratch<'_>> {
        let settings = Settings::from_env()?;
        let shm = std::path::Path::new(placement::SHM);
        // Read only for the automatic choice: a quota that cannot be read
        // stops it, and a chosen mode needs no reading.
        let room = match settings.mode {
            placement::Mode::Auto => Room::read(shm)?,
            placement::Mode::Disk | placement::Mode::Ram => Room::default(),
        };
        self.scratch_with(expected, &settings, room, shm, &placement::BUDGET)
    }

    /// [`Store::scratch_for`] with what it reads from the environment and the
    /// machine given, so the choice can be tested.
    pub(crate) fn scratch_with(
        &self,
        expected: u64,
        settings: &Settings,
        room: Room,
        shm: &Path,
        budget: &'static placement::RamBudget,
    ) -> Result<Scratch<'_>> {
        let wanted = match settings.mode {
            Mode::Disk => false,
            Mode::Ram => true,
            Mode::Auto => placement::auto_fits(room, expected),
        };
        if !wanted {
            return self.scratch();
        }
        placement::sweep_once(shm);
        let root = placement::ram_root(self.root(), shm);
        match budget.claim(expected, settings.ram_max) {
            Some(claim) => Scratch::in_ram(self, &root, claim),
            None if settings.mode == Mode::Ram => Err(Error::work(format!(
                "a RAM work directory of {expected} bytes would pass GAIA_RAM_MAX={} \
                 ({} bytes are in use)",
                settings.ram_max,
                budget.in_use()
            ))),
            None => self.scratch(),
        }
    }

    /// Copies the file at `path` into the store.
    pub fn put_file(&self, path: &Path) -> Result<ObjectRef> {
        let scratch = self.scratch()?;
        let copy = scratch.path().join("file");
        fsutil::copy_file(path, &copy).map_err(|e| Error::io("copy", path, e))?;
        scratch.commit_one(Path::new("file"))
    }

    /// Copies the tree at `path` into the store: directories, regular files
    /// with their permission bits, and symlinks as links.
    pub fn put_tree(&self, path: &Path) -> Result<ObjectRef> {
        let scratch = self.scratch()?;
        let copy = scratch.path().join("tree");
        fsutil::copy_tree(path, &copy)?;
        scratch.commit_one(Path::new("tree"))
    }

    /// The step entry recorded for `key`, if any.
    pub fn entry(&self, key: &StepKey) -> Result<Option<StepEntry>> {
        entry::read(&self.entry_path(key))
    }

    /// The outputs recorded for `key` when every one of them is in the store,
    /// checked as `check` says. `None` means the step has to run.
    pub fn lookup(&self, key: &StepKey, check: Check) -> Result<Option<StepEntry>> {
        Ok(self.entry(key)?.filter(|entry| {
            entry
                .outputs
                .values()
                .all(|object| self.check(object, check))
        }))
    }

    /// Records `entry` for `key`. Every object it names must already be in
    /// the store. The entry appears whole, by rename.
    pub fn record(&self, key: &StepKey, entry: &StepEntry) -> Result<()> {
        if let Some((name, object)) = entry
            .outputs
            .iter()
            .find(|(_, object)| !self.check(object, Check::Present))
        {
            return Err(Error::Missing {
                output: name.clone(),
                object: *object,
            });
        }
        entry::write(&self.entry_path(key), entry)
    }

    /// The one right to make `key`'s step in this store, waited for while
    /// another process or thread holds it. Two makers of one key would each
    /// do the whole work, and where a step's output is not the same bytes
    /// twice, the later record would replace the earlier and orphan every
    /// step keyed on the earlier output. The holder looks the key up again
    /// once it has the right: the other maker may have recorded it.
    ///
    /// The right is a lock on a file under `making/`; it ends when the
    /// value is dropped, and when its process dies.
    pub fn making(&self, key: &StepKey) -> Result<Making> {
        let dir = self.root.join("making");
        fs::create_dir_all(&dir).map_err(|e| Error::io("create", &dir, e))?;
        let path = dir.join(format!("{}.lock", key.0.hex()));
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io("open", &path, e))?;
        file.lock().map_err(|e| Error::io("lock", &path, e))?;
        Ok(Making { _file: file })
    }

    fn entry_path(&self, key: &StepKey) -> PathBuf {
        let hex = key.0.hex();
        self.root
            .join("entries")
            .join(&hex[..2])
            .join(format!("{hex}.json"))
    }
}

/// The right to make one key's step; see [`Store::making`].
#[derive(Debug)]
pub struct Making {
    _file: fs::File,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Io {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A device, socket or pipe, which a store cannot hold.
    Unsupported(PathBuf),
    /// A step's output is not where the step said, or is not a file or tree.
    BadOutput { output: PathBuf, reason: String },
    /// An entry names an object that is not in the store.
    Missing { output: String, object: ObjectRef },
    /// An entry file that does not parse.
    BadEntry { path: PathBuf, reason: String },
    /// A work directory that cannot be placed as asked: a setting that does
    /// not parse, or a RAM work directory that would pass the cap.
    Work { reason: String },
}

impl Error {
    pub(crate) fn io(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn work(reason: String) -> Self {
        Self::Work { reason }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
            Self::Unsupported(path) => write!(
                f,
                "{} is a device, socket or pipe, which the store cannot hold",
                path.display()
            ),
            Self::BadOutput { output, reason } => {
                write!(f, "output {}: {reason}", output.display())
            }
            Self::Missing { output, object } => {
                write!(
                    f,
                    "output {output} names {object}, which is not in the store"
                )
            }
            Self::BadEntry { path, reason } => {
                write!(f, "step entry {}: {reason}", path.display())
            }
            Self::Work { reason } => write!(f, "work directory: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
