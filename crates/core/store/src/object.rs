//! References to stored objects.

use crate::{Digest, Result, file_digest, tree_digest};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// What an object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    File,
    Tree,
}

impl Kind {
    pub(crate) fn dir_name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Tree => "tree",
        }
    }

    pub(crate) fn digest(self, path: &Path) -> Result<Digest> {
        match self {
            Self::File => file_digest(path),
            Self::Tree => tree_digest(path),
        }
    }
}

/// A stored file or tree. Written as `file:sha256:<hex>` or
/// `tree:sha256:<hex>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectRef {
    pub kind: Kind,
    pub digest: Digest,
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.dir_name(), self.digest)
    }
}

impl fmt::Debug for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for ObjectRef {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (kind, digest) = text
            .split_once(':')
            .ok_or_else(|| format!("not an object reference: {text}"))?;
        let kind = match kind {
            "file" => Kind::File,
            "tree" => Kind::Tree,
            other => return Err(format!("unknown object kind {other} in {text}")),
        };
        let digest = Digest::parse(digest).ok_or_else(|| format!("bad digest in {text}"))?;
        Ok(Self { kind, digest })
    }
}

impl Serialize for ObjectRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ObjectRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
