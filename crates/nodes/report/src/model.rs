//! The report's value: what the image is made of, what it was built from,
//! and how it differs from the report it is compared with.

use serde::{Deserialize, Serialize};

/// Written into every report.
pub const FORMAT: &str = "gaia.report";
/// Bumped when the report's fields change.
pub const VERSION: u32 = 1;

/// The report of one image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub format: String,
    pub version: u32,
    /// The image's name in the product.
    pub image: String,
    /// The image archive, by store reference (`file:sha256:...`).
    pub image_object: String,
    pub image_bytes: u64,
    /// The sources each owner was built from, from the lock.
    pub sources: Vec<SourceRecord>,
    /// One line per owner.
    pub components: Vec<ComponentRecord>,
    /// Every file (and symlink) in the image, sorted by path.
    pub files: Vec<FileRecord>,
    /// The sum of the files' bytes.
    pub total_bytes: u64,
    /// The report this one is compared with, when there was one.
    pub previous: Option<String>,
    pub delta: Option<Delta>,
}

/// One locked source of one owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub owner: String,
    /// The source's name in the component, `src` for `hello-a.src`.
    pub source: String,
    /// `git`, `download`, `image` or `tree`.
    pub kind: String,
    /// The URL, image reference or path, masked.
    pub locator: String,
    /// The commit or digest it resolved to.
    pub resolved: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub owner: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Relative to the image's root, `/`-separated.
    pub path: String,
    /// The components that claim the path, in layer order. The last one's
    /// bytes and mode are the file's.
    pub owners: Vec<String>,
    /// `file` or `symlink`.
    pub kind: String,
    /// Octal, as an image gets it.
    pub mode: String,
    /// Bytes of a file; 0 for a symlink.
    pub bytes: u64,
}

/// What changed against the previous report. Byte changes are signed:
/// after minus before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    pub image_change: i64,
    pub total_change: i64,
    pub components: Vec<OwnerChange>,
    pub sources: Vec<SourceChange>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<FileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerChange {
    pub owner: String,
    pub before: u64,
    pub after: u64,
}

/// A locked source whose resolved commit or digest, or whose presence, changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChange {
    /// `owner.source`.
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub before_mode: String,
    pub after_mode: String,
    pub before_owners: Vec<String>,
    pub after_owners: Vec<String>,
}
