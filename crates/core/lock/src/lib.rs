//! The lock: one file per product, naming every source the build uses and
//! what it resolved to.
//!
//! - a git source, by its commit;
//! - a download, by the digest of its bytes;
//! - a container image, by its manifest digest;
//! - a tree (a local directory or an unpacked archive), by its tree digest.
//!
//! The text is sorted by name and written the same way every time, so
//! reading a lock and writing it again gives the same bytes. Only `gaia lock`
//! writes it.
//!
//! [`Lock::check`] compares a lock with what the descriptions ask for. A
//! source the lock lacks, a lock entry no description uses, and an entry
//! recorded for another URL, ref or reference each fail, by name.
//!
//! ```toml
//! # Written by `gaia lock`. Do not edit.
//! version = 1
//!
//! [source."kernel"]
//! kind = "git"
//! url = "https://example.org/linux.git"
//! ref = "v7.3.3"
//! commit = "0123456789abcdef0123456789abcdef01234567"
//! ```

mod check;
mod text;

pub use check::{LockError, Want};

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// The lock format's version.
pub const VERSION: u32 = 1;

/// What one source resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A git repository at a ref, resolved to a commit (40 or 64 hex digits).
    Git {
        url: String,
        reference: String,
        commit: String,
    },
    /// A file fetched from a URL, by the digest of its bytes.
    Download { url: String, digest: String },
    /// A container image, by the digest of its manifest.
    Image { reference: String, digest: String },
    /// A directory or an unpacked archive, by its tree digest.
    Tree { path: String, digest: String },
}

impl Entry {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Git { .. } => "git",
            Self::Download { .. } => "download",
            Self::Image { .. } => "image",
            Self::Tree { .. } => "tree",
        }
    }

    /// The commit or digest the source resolved to.
    pub fn resolved(&self) -> &str {
        match self {
            Self::Git { commit, .. } => commit,
            Self::Download { digest, .. }
            | Self::Image { digest, .. }
            | Self::Tree { digest, .. } => digest,
        }
    }
}

/// A product's lock: source name to what it resolved to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    pub entries: BTreeMap<String, Entry>,
}

impl Lock {
    /// Reads a lock's text. Anything not written by [`Lock::render`] in form
    /// (an unknown field, a bad commit or digest, another version) is an
    /// error.
    pub fn parse(text: &str) -> Result<Self, LockError> {
        text::parse(text)
    }

    /// The lock's text: sorted, the same bytes for the same entries.
    pub fn render(&self) -> String {
        text::render(self)
    }

    pub fn read(path: &Path) -> Result<Self, LockError> {
        let text = fs::read_to_string(path).map_err(|error| LockError::Io {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::parse(&text)
    }

    /// Writes the lock to `path` whole: a temporary file, synced, renamed
    /// into place.
    pub fn write(&self, path: &Path) -> Result<(), LockError> {
        let io = |error: std::io::Error| LockError::Io {
            path: path.display().to_string(),
            reason: error.to_string(),
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let temporary = path.with_file_name(format!(".{}.tmp", name.unwrap_or_default()));
        let mut file = fs::File::create(&temporary).map_err(io)?;
        file.write_all(self.render().as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        fs::rename(&temporary, path).map_err(io)
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    /// Records `entry` for `name`. A URL's user and password are never
    /// recorded.
    pub fn insert(&mut self, name: &str, entry: Entry) {
        let entry = match entry {
            Entry::Git {
                url,
                reference,
                commit,
            } => Entry::Git {
                url: redact(&url),
                reference,
                commit,
            },
            Entry::Download { url, digest } => Entry::Download {
                url: redact(&url),
                digest,
            },
            other => other,
        };
        self.entries.insert(name.to_string(), entry);
    }
}

/// `url` without a `user:password@` part.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}
