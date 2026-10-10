//! What running an image operation would do, computed without changing
//! anything (`gaia preview`). Providers fill these in from the same decision
//! code their runs use; the app renders them and derives the exit code.

/// The clean a run would make of its output tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewCleanKind {
    /// Nothing is cleaned or rebuilt.
    Nothing,
    /// Only target finalization runs again.
    Finalize,
    /// Some packages are uninstalled and rebuilt.
    Packages,
    /// The whole output tree is cleaned.
    Full,
}

impl PreviewCleanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Finalize => "finalize",
            Self::Packages => "packages",
            Self::Full => "full",
        }
    }
}

/// What a path would be removed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewDeletionKind {
    /// A whole tree directory (a full clean, a RAM tree switch).
    Tree,
    /// Installed files and build directories of packages rebuilt or
    /// uninstalled.
    Package,
    /// A package cache entry evicted to stay within its size.
    Cache,
    /// Leftovers of an earlier clean, purged in the background.
    Trash,
}

impl PreviewDeletionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Package => "package",
            Self::Cache => "cache",
            Self::Trash => "trash",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewDeletion {
    pub kind: PreviewDeletionKind,
    pub path: String,
    pub reason: String,
}

/// A heading and the lines under it, in the order the report shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSection {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePreview {
    pub provider_id: String,
    pub sections: Vec<PreviewSection>,
    pub clean: PreviewCleanKind,
    /// Why the clean is what it is (empty for `Nothing`).
    pub clean_reasons: Vec<String>,
    /// Packages that would be rebuilt (uninstalled and built again).
    pub rebuilt_packages: Vec<String>,
    /// Packages that would only be uninstalled.
    pub uninstalled_packages: Vec<String>,
    /// Every path a run would remove or move aside.
    pub deletions: Vec<PreviewDeletion>,
    /// Set when the run would stop before building, and why.
    pub blocked: Option<String>,
    /// The one-line summary of this image operation, without the `preview:`
    /// prefix.
    pub verdict: String,
}

impl ImagePreview {
    /// Deletions other than the purge of earlier leftovers.
    pub fn deletions_outside_trash(&self) -> usize {
        self.deletions
            .iter()
            .filter(|deletion| deletion.kind != PreviewDeletionKind::Trash)
            .count()
    }

    /// Whether a run would clean the whole output tree or delete something
    /// other than leftovers of an earlier clean.
    pub fn trips_fail_on_clean(&self) -> bool {
        self.clean == PreviewCleanKind::Full || self.deletions_outside_trash() > 0
    }
}
