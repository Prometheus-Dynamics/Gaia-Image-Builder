//! The first-flash signature: the message the board rebuilds, the 108-byte
//! record that carries the signature, and the secret key's decoding.
//!
//! The message is the bytes Ed25519 signs (not a prehash): a domain tag, the
//! format version, the board id, the raw image's kind, its size and its
//! SHA-256. The board rebuilds it from its own id and the manifest's size
//! and hash, so none of the message's fields is trusted from the sender.
//! The record is `LMNSSIG1`, the algorithm (1, Ed25519), three zero bytes,
//! the key id (SHA-256 of the public key) and the 64-byte signature.
//!
//! The secret key is minisign's unencrypted secret key format, the same one
//! `gaia.sign` reads: `minisign -G -W` makes it. Only its 32-byte seed and
//! public half are used here.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest as _};
use ed25519_dalek::SigningKey;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

/// The domain tag at the start of the message, with its NUL.
pub const DOMAIN: &[u8] = b"lemnos-first-flash\0";
/// The message's version field.
pub const VERSION: u32 = 1;
/// The record's magic.
pub const MAGIC: &[u8; 8] = b"LMNSSIG1";
/// The record's algorithm byte: Ed25519.
pub const ALGORITHM_ED25519: u8 = 1;
/// The record's length: magic, algorithm and zeros, key id, signature.
pub const RECORD_LEN: usize = 108;

/// The first two bytes of a minisign key (`Ed`).
const LEGACY: &[u8; 2] = b"Ed";

/// The bytes Ed25519 signs for a first-flash image: `board` and `format`
/// (the raw image's kind, `disk`), the raw image's `size` in bytes and its
/// `sha256`. Refused when a string is longer than a u32 length.
pub fn message(board: &str, format: &str, size: u64, sha256: &[u8; 32]) -> Result<Vec<u8>, String> {
    let mut out =
        Vec::with_capacity(DOMAIN.len() + 4 + 4 + board.len() + 4 + format.len() + 8 + 32);
    out.extend_from_slice(DOMAIN);
    out.extend_from_slice(&VERSION.to_le_bytes());
    push_string(&mut out, board)?;
    push_string(&mut out, format)?;
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(sha256);
    Ok(out)
}

fn push_string(out: &mut Vec<u8>, text: &str) -> Result<(), String> {
    let len = u32::try_from(text.len()).map_err(|_| "a string too long for the message")?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

/// The key id: the SHA-256 of the 32-byte public key.
pub fn key_id(public: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(public).into()
}

/// The 108-byte signature record.
pub fn record(key_id: &[u8; 32], signature: &[u8; 64]) -> [u8; RECORD_LEN] {
    let mut out = [0_u8; RECORD_LEN];
    out[..8].copy_from_slice(MAGIC);
    out[8] = ALGORITHM_ED25519;
    out[12..44].copy_from_slice(key_id);
    out[44..108].copy_from_slice(signature);
    out
}

/// The Ed25519 key an unencrypted minisign secret key file holds. Errors
/// never include key bytes.
pub fn secret_key(text: &str) -> Result<SigningKey, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .ok_or("an empty secret key")?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(line)
            .map_err(|_| "a secret key that is not base64".to_string())?,
    );
    // sig alg, kdf alg, checksum alg, salt, opslimit, memlimit, then the
    // key id, the 64-byte keypair and a checksum.
    if bytes.len() != 158 || &bytes[..2] != LEGACY || &bytes[4..6] != b"B2" {
        return Err("not a minisign Ed25519 secret key".into());
    }
    if bytes[2..4] != [0, 0] {
        return Err("a password-protected secret key; make one with `minisign -G -W`".into());
    }
    let keynum = &bytes[54..158];
    let checksum = &keynum[72..104];
    if checksum.iter().any(|&b| b != 0) {
        let mut hasher = Blake2b::<U32>::new();
        hasher.update(LEGACY);
        hasher.update(&keynum[..72]);
        if hasher.finalize().as_slice() != checksum {
            return Err("a secret key whose checksum does not match".into());
        }
    }
    let pair = Zeroizing::new(
        <[u8; 64]>::try_from(&keynum[8..72]).map_err(|_| "a short secret key".to_string())?,
    );
    SigningKey::from_keypair_bytes(&pair)
        .map_err(|_| "a secret key whose halves do not match".to_string())
}
