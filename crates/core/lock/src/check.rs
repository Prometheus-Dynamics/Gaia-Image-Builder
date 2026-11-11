//! Comparing a lock with what the descriptions ask for.

use crate::{Entry, Lock, redact};
use std::fmt;

/// What a description asks for: a source by name, before it is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    Git { url: String, reference: String },
    Download { url: String },
    Image { reference: String },
    Tree { path: String },
}

impl Want {
    /// Whether `entry` was recorded for this.
    fn matches(&self, entry: &Entry) -> bool {
        match (self, entry) {
            (
                Self::Git { url, reference },
                Entry::Git {
                    url: u,
                    reference: r,
                    ..
                },
            ) => redact(url) == *u && reference == r,
            (Self::Download { url }, Entry::Download { url: u, .. }) => redact(url) == *u,
            (Self::Image { reference }, Entry::Image { reference: r, .. }) => reference == r,
            (Self::Tree { path }, Entry::Tree { path: p, .. }) => path == p,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The lock does not match the descriptions. Each list is sorted.
    Mismatch {
        /// Sources the descriptions use that the lock lacks.
        missing: Vec<String>,
        /// Lock entries no description uses.
        unused: Vec<String>,
        /// Entries recorded for another URL, ref, reference or path than the
        /// description now names.
        changed: Vec<String>,
    },
    Parse(String),
    Io {
        path: String,
        reason: String,
    },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch {
                missing,
                unused,
                changed,
            } => {
                f.write_str("the lock does not match the descriptions; run `gaia lock`")?;
                for (what, names) in [
                    ("not in the lock", missing),
                    ("in the lock but used by nothing", unused),
                    ("locked for something else", changed),
                ] {
                    if !names.is_empty() {
                        write!(f, "\n  {what}: {}", names.join(", "))?;
                    }
                }
                Ok(())
            }
            Self::Parse(reason) => write!(f, "the lock does not read: {reason}"),
            Self::Io { path, reason } => write!(f, "{path}: {reason}"),
        }
    }
}

impl std::error::Error for LockError {}

impl Lock {
    /// Checks the lock against every source the descriptions use, by name.
    /// Every name that is missing, unused or changed is reported at once.
    pub fn check<'a>(
        &self,
        wants: impl IntoIterator<Item = (&'a str, &'a Want)>,
    ) -> Result<(), LockError> {
        let mut missing = Vec::new();
        let mut changed = Vec::new();
        let mut used = std::collections::BTreeSet::new();
        for (name, want) in wants {
            used.insert(name);
            match self.entries.get(name) {
                None => missing.push(name.to_string()),
                Some(entry) if !want.matches(entry) => changed.push(name.to_string()),
                Some(_) => {}
            }
        }
        let unused: Vec<String> = self
            .entries
            .keys()
            .filter(|name| !used.contains(name.as_str()))
            .cloned()
            .collect();
        missing.sort();
        missing.dedup();
        changed.sort();
        changed.dedup();
        if missing.is_empty() && unused.is_empty() && changed.is_empty() {
            Ok(())
        } else {
            Err(LockError::Mismatch {
                missing,
                unused,
                changed,
            })
        }
    }
}
