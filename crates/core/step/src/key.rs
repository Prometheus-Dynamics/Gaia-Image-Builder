//! Step keys.

use gaia_store::{Digest, StepKey};
use sha2::{Digest as _, Sha256};

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

    /// A format or encoder library the node links, by the version Cargo
    /// resolved for it. A new version changes the key, so a stateless cache
    /// never returns an output this code would not make. A test checks the
    /// version against Cargo.lock (see [`crate::cargo_lock`]).
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
        StepKey(Digest::from_bytes(hasher.finalize().into()))
    }
}

fn field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
