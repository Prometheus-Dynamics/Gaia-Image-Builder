//! Step keys.

use gaia_store::{Digest, StepKey};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// Bumped when the key encoding changes, which changes every key.
const KEY_ENCODING: &[u8] = b"gaia-step-key\0v1\0";

/// Builds a step's key from its node kind, node version, config and input
/// digests. Config entries and inputs are named and sorted by name, so the
/// order they are given in does not matter; every value is length-prefixed,
/// so no two different sets of parts encode the same.
#[derive(Debug, Clone)]
pub struct KeyBuilder {
    kind: String,
    version: u32,
    config: Vec<(String, Vec<u8>)>,
    inputs: Vec<(String, Digest)>,
}

impl KeyBuilder {
    /// `kind` is the node id; `version` is bumped when the node's body
    /// changes what it makes from the same inputs.
    pub fn new(kind: &str, version: u32) -> Self {
        Self {
            kind: kind.to_string(),
            version,
            config: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// A config value, as bytes that change exactly when the value does.
    pub fn config(mut self, name: &str, value: impl AsRef<[u8]>) -> Self {
        self.config
            .push((name.to_string(), value.as_ref().to_vec()));
        self
    }

    /// An input, by the digest of its content.
    pub fn input(mut self, name: &str, digest: Digest) -> Self {
        self.inputs.push((name.to_string(), digest));
        self
    }

    /// A format or encoder library the node links, by its version: the one
    /// its Cargo.toml pins (see [`crate::pins`]), or the one the library reports
    /// at run time. A new version changes the key, so a stateless cache never
    /// returns an output this code would not make.
    pub fn lib(self, name: &str, version: &str) -> Self {
        self.config(&format!("lib {name}"), version)
    }

    /// Every library in `libs`, as [`KeyBuilder::lib`].
    pub fn libs(self, libs: &[(&str, &str)]) -> Self {
        libs.iter()
            .fold(self, |key, (name, version)| key.lib(name, version))
    }

    pub fn finish(mut self) -> StepKey {
        self.config.sort();
        self.inputs.sort();
        let shape = self.shape();
        let inputs: Vec<Digest> = self.inputs.iter().map(|(_, digest)| *digest).collect();
        let mut hasher = Sha256::new();
        hasher.update(KEY_ENCODING);
        field(&mut hasher, self.kind.as_bytes());
        hasher.update(self.version.to_le_bytes());
        hasher.update((self.config.len() as u64).to_le_bytes());
        for (name, value) in &self.config {
            field(&mut hasher, name.as_bytes());
            field(&mut hasher, value);
        }
        hasher.update((self.inputs.len() as u64).to_le_bytes());
        for (name, digest) in &self.inputs {
            field(&mut hasher, name.as_bytes());
            hasher.update(digest.as_bytes());
        }
        let key = StepKey(Digest::from_bytes(hasher.finalize().into()));
        SHAPES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, Shape { shape, inputs });
        key
    }

    /// The step's shape: its kind and the names of its config entries and
    /// inputs, without their values. Two runs of one step over changed
    /// content have the same shape; the kernel's and the root filesystem's
    /// Buildroot builds, which have fragments of their own, do not.
    fn shape(&self) -> Digest {
        let mut hasher = Sha256::new();
        hasher.update(SHAPE_ENCODING);
        field(&mut hasher, self.kind.as_bytes());
        let mut config: Vec<&str> = self.config.iter().map(|(name, _)| name.as_str()).collect();
        config.dedup();
        hasher.update((config.len() as u64).to_le_bytes());
        for name in config {
            field(&mut hasher, name.as_bytes());
        }
        let mut inputs: Vec<&str> = self.inputs.iter().map(|(name, _)| name.as_str()).collect();
        inputs.dedup();
        hasher.update((inputs.len() as u64).to_le_bytes());
        for name in inputs {
            field(&mut hasher, name.as_bytes());
        }
        Digest::from_bytes(hasher.finalize().into())
    }
}

/// Bumped when the shape encoding changes, which leaves no step a history.
const SHAPE_ENCODING: &[u8] = b"gaia-step-shape\0v1\0";

/// What a finished key was made of, beyond the key, for the step record:
/// its shape and its inputs' digests. Neither is part of any key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) shape: Digest,
    pub(crate) inputs: Vec<Digest>,
}

/// The shape of every key this process finished, so a step's record can
/// carry it without the step passing it along: the step is given the key
/// alone.
static SHAPES: Mutex<BTreeMap<StepKey, Shape>> = Mutex::new(BTreeMap::new());

/// The shape and inputs `key` was finished with in this process.
pub(crate) fn shape_of(key: &StepKey) -> Option<Shape> {
    SHAPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(key)
        .cloned()
}

fn field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
