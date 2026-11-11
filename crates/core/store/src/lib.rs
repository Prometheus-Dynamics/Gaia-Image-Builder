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
//! ```

mod digest;
mod entry;
mod fsutil;
mod object;
mod retain;
mod scratch;

pub use digest::{Digest, file_digest, tree_digest};
pub use entry::{StepEntry, StepKey};
pub use object::{Kind, ObjectRef};
pub use retain::{Keep, RetainReport};
pub use scratch::Scratch;

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
    /// The object's content is read again and must match its digest.
    Digest,
}

impl Store {
    /// Opens the store at `root`, creating its directories when missing.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        for dir in ["objects/file", "objects/tree", "entries", "scratch"] {
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
        kind_matches
            && match check {
                Check::Present => true,
                Check::Digest => object.kind.digest(&path).ok() == Some(object.digest),
            }
    }

    /// A new scratch directory for one step's outputs.
    pub fn scratch(&self) -> Result<Scratch<'_>> {
        Scratch::new(self)
    }

    /// Copies the file at `path` into the store.
    pub fn put_file(&self, path: &Path) -> Result<ObjectRef> {
        let scratch = self.scratch()?;
        let copy = scratch.path().join("file");
        fs::copy(path, &copy).map_err(|e| Error::io("copy", path, e))?;
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

    fn entry_path(&self, key: &StepKey) -> PathBuf {
        let hex = key.0.hex();
        self.root
            .join("entries")
            .join(&hex[..2])
            .join(format!("{hex}.json"))
    }
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
}

impl Error {
    pub(crate) fn io(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
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
