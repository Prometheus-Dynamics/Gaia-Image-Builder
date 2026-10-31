//! Stable fingerprints for everything Gaia persists and compares across runs
//! (reuse state, provider state digests, cache directory names).
//!
//! A [`Fingerprint`] is SHA-256 over an explicit encoding: the scheme version,
//! a domain naming what is fingerprinted, then typed, length-prefixed values.
//! Nothing here depends on `std::hash` (whose algorithms may change between
//! Rust releases) or on `Debug` text. Structured values go through
//! [`canonical_json`]: serde JSON with object keys sorted, so the text only
//! changes when a value does.
//!
//! Bumping [`FINGERPRINT_SCHEME`] changes every fingerprint at once; the reuse
//! state records the scheme it was written under, so a bump re-evaluates every
//! operation once instead of comparing values of different schemes.

use gaia_process::hex;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Version of the fingerprint encoding. `1` was `DefaultHasher` over `Debug`
/// text (never recorded); `2` is this module.
pub const FINGERPRINT_SCHEME: u32 = 2;

/// SHA-256 over typed, length-prefixed values. Each value is tagged with its
/// type, so `str("ab")` and `str("a").str("b")` differ.
#[derive(Clone)]
pub struct Fingerprint(Sha256);

impl Fingerprint {
    /// Starts a fingerprint of `domain` (what is fingerprinted, such as
    /// `"operation"` or `"dir-digest"`) under [`FINGERPRINT_SCHEME`].
    pub fn new(domain: &str) -> Self {
        let mut fingerprint = Self(Sha256::new());
        fingerprint
            .raw(b"gaia-fingerprint")
            .u64(u64::from(FINGERPRINT_SCHEME))
            .str(domain);
        fingerprint
    }

    fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update(bytes);
        self
    }

    fn tagged(&mut self, tag: u8, bytes: &[u8]) -> &mut Self {
        self.0.update([tag]);
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
        self
    }

    pub fn str(&mut self, value: &str) -> &mut Self {
        self.tagged(b's', value.as_bytes())
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.tagged(b'b', value)
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.tagged(b'u', &value.to_le_bytes())
    }

    pub fn u128(&mut self, value: u128) -> &mut Self {
        self.tagged(b'w', &value.to_le_bytes())
    }

    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.tagged(b't', &[u8::from(value)])
    }

    /// A value that may be absent; absent is encoded distinctly from any
    /// present value, including the empty string.
    pub fn optional_str(&mut self, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.tagged(b'?', &[1]).str(value),
            None => self.tagged(b'?', &[0]),
        }
    }

    /// A named text input.
    pub fn field(&mut self, name: &str, value: &str) -> &mut Self {
        self.tagged(b'n', name.as_bytes()).str(value)
    }

    /// A named structured input, as its [`canonical_json`].
    pub fn value<T: Serialize + ?Sized>(&mut self, name: &str, value: &T) -> &mut Self {
        self.field(name, &canonical_json(value))
    }

    pub fn finish(self) -> FingerprintDigest {
        FingerprintDigest(self.0.finalize().into())
    }
}

/// A finished [`Fingerprint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FingerprintDigest([u8; 32]);

impl FingerprintDigest {
    /// The full digest as 64 hex characters.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// The first 64 bits as 16 hex characters, for digests stored in state
    /// files and directory names.
    pub fn short_hex(&self) -> String {
        format!("{:016x}", self.as_u64())
    }

    /// The first 64 bits, big-endian, for fingerprints stored as numbers.
    pub fn as_u64(&self) -> u64 {
        let mut first = [0_u8; 8];
        first.copy_from_slice(&self.0[..8]);
        u64::from_be_bytes(first)
    }
}

/// Whether `value` is its type's default: for `skip_serializing_if` on
/// options that are only encoded when set.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// For fields that default to `true`, so specs written before the field
/// existed keep their fingerprint.
pub(crate) fn is_true(value: &bool) -> bool {
    *value
}

/// SHA-256 hex of `text`.
pub fn sha256_hex(text: &str) -> String {
    gaia_process::sha256_hex_bytes(text.as_bytes())
}

/// `value` as JSON with every object's keys sorted and no whitespace, so
/// the text does not depend on field order, map iteration order or the
/// `serde_json` features another crate enables. A value serde cannot encode
/// yields a fixed `unserializable` text naming the error.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(value) => {
            let mut out = String::new();
            write_canonical(&value, &mut out);
            out
        }
        Err(error) => format!("unserializable:{error}"),
    }
}

/// `value` as a JSON value, for splitting a struct into named fingerprint
/// inputs. `Null` when serde cannot encode it.
pub fn json_value<T: Serialize + ?Sized>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// The canonical text of a JSON value (see [`canonical_json`]).
pub fn canonical_json_value(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
