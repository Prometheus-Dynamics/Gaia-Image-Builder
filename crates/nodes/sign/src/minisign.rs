//! The minisign formats: public keys, unencrypted secret keys and
//! signatures (prehashed Ed25519 over BLAKE2b-512, with a signed trusted
//! comment), so `minisign -V` checks what this signs and the other way
//! round.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::Zeroizing;

const PREHASHED: &[u8; 2] = b"ED";
const LEGACY: &[u8; 2] = b"Ed";

/// A public key: its 8-byte id and the Ed25519 key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub id: [u8; 8],
    key: VerifyingKey,
}

impl PublicKey {
    /// A public key file's text, or its base64 line alone.
    pub fn parse(text: &str) -> Result<Self, String> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or("an empty public key")?;
        let bytes = STANDARD
            .decode(line)
            .map_err(|_| "a public key that is not base64")?;
        if bytes.len() != 42 || &bytes[..2] != LEGACY {
            return Err("not a minisign Ed25519 public key".into());
        }
        let id = bytes[2..10].try_into().map_err(|_| "a short key id")?;
        let key = VerifyingKey::from_bytes(bytes[10..42].try_into().map_err(|_| "a short key")?)
            .map_err(|_| "a public key that is not a curve point")?;
        Ok(Self { id, key })
    }

    /// The base64 line.
    pub fn to_line(&self) -> String {
        let mut bytes = LEGACY.to_vec();
        bytes.extend_from_slice(&self.id);
        bytes.extend_from_slice(self.key.as_bytes());
        STANDARD.encode(bytes)
    }

    /// The id as minisign prints it.
    pub fn id_hex(&self) -> String {
        self.id.iter().rev().map(|b| format!("{b:02X}")).collect()
    }
}

/// A secret key, held only in memory and wiped when dropped.
pub struct SecretKey {
    id: [u8; 8],
    key: SigningKey,
}

impl std::fmt::Debug for SecretKey {
    /// Shows the key's id only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretKey")
            .field("id", &self.public().id_hex())
            .finish_non_exhaustive()
    }
}

impl SecretKey {
    /// An unencrypted minisign secret key file's text (`minisign -G -W`).
    /// Errors never include key bytes.
    pub fn parse(text: &str) -> Result<Self, String> {
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
        // sig alg, kdf alg, checksum alg, salt, opslimit, memlimit, then
        // id, the 64-byte key and a checksum.
        if bytes.len() != 158 || &bytes[..2] != LEGACY || &bytes[4..6] != b"B2" {
            return Err("not a minisign Ed25519 secret key".into());
        }
        if bytes[2..4] != [0, 0] {
            return Err("a password-protected secret key; make one with `minisign -G -W`".into());
        }
        let keynum = &bytes[54..158];
        // minisign 0.12 writes no checksum for an unencrypted key (all
        // zero) and does not check one; older versions write one. A key
        // without one is still checked below: its public half must be the
        // one its seed derives.
        let checksum = &keynum[72..104];
        if checksum.iter().any(|&b| b != 0) {
            let mut hasher = blake2::Blake2b::<blake2::digest::consts::U32>::new();
            hasher.update(LEGACY);
            hasher.update(&keynum[..72]);
            if hasher.finalize().as_slice() != checksum {
                return Err("a secret key whose checksum does not match".into());
            }
        }
        let id: [u8; 8] = keynum[..8].try_into().map_err(|_| "a short key id")?;
        let pair: Zeroizing<[u8; 64]> =
            Zeroizing::new(keynum[8..72].try_into().map_err(|_| "a short key")?);
        let key = SigningKey::from_keypair_bytes(&pair)
            .map_err(|_| "a secret key whose halves do not match".to_string())?;
        Ok(Self { id, key })
    }

    /// A key from a 32-byte seed, for tests.
    pub fn from_seed(seed: &[u8; 32], id: [u8; 8]) -> Self {
        Self {
            id,
            key: SigningKey::from_bytes(seed),
        }
    }

    /// Its unencrypted secret key file, for tests.
    pub fn to_file(&self) -> Zeroizing<String> {
        let mut keynum = Zeroizing::new(self.id.to_vec());
        keynum.extend_from_slice(&self.key.to_keypair_bytes());
        let mut hasher = blake2::Blake2b::<blake2::digest::consts::U32>::new();
        hasher.update(LEGACY);
        hasher.update(&keynum[..]);
        keynum.extend_from_slice(&hasher.finalize());
        let mut bytes = Zeroizing::new(LEGACY.to_vec());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(b"B2");
        bytes.extend_from_slice(&[0; 48]);
        bytes.extend_from_slice(&keynum);
        Zeroizing::new(format!(
            "untrusted comment: minisign encrypted secret key\n{}\n",
            STANDARD.encode(&bytes[..])
        ))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey {
            id: self.id,
            key: self.key.verifying_key(),
        }
    }

    /// The signature file for `data`, with `comment` as its trusted
    /// comment.
    pub fn sign(&self, data: &[u8], comment: &str) -> String {
        let signature = self.key.sign(&Blake2b512::digest(data));
        let mut first = PREHASHED.to_vec();
        first.extend_from_slice(&self.id);
        first.extend_from_slice(&signature.to_bytes());
        let mut global = signature.to_bytes().to_vec();
        global.extend_from_slice(comment.as_bytes());
        let global = self.key.sign(&global);
        format!(
            "untrusted comment: signature from gaia secret key\n{}\ntrusted comment: {comment}\n{}\n",
            STANDARD.encode(first),
            STANDARD.encode(global.to_bytes())
        )
    }
}

/// Checks `signature` (a signature file's text) over `data` against
/// `trusted`. Gives the key that signed and the trusted comment.
pub fn verify<'a>(
    data: &[u8],
    signature: &str,
    trusted: &'a [PublicKey],
) -> Result<(&'a PublicKey, String), String> {
    let mut lines = signature.lines().map(str::trim_end);
    let _untrusted = lines.next().ok_or("an empty signature")?;
    let first = STANDARD
        .decode(lines.next().ok_or("a signature with no signature line")?)
        .map_err(|_| "a signature line that is not base64")?;
    let comment = lines
        .next()
        .and_then(|l| l.strip_prefix("trusted comment: "))
        .ok_or("a signature with no trusted comment")?
        .to_string();
    let global = STANDARD
        .decode(
            lines
                .next()
                .ok_or("a signature with no comment signature")?,
        )
        .map_err(|_| "a comment signature that is not base64")?;
    if first.len() != 74 || global.len() != 64 {
        return Err("not a minisign signature".into());
    }
    let id: [u8; 8] = first[2..10].try_into().map_err(|_| "a short key id")?;
    let key = trusted.iter().find(|k| k.id == id).ok_or_else(|| {
        let hex: String = id.iter().rev().map(|b| format!("{b:02X}")).collect();
        format!("signed by key {hex}, which is not trusted")
    })?;
    let sig = Signature::from_slice(&first[10..74]).map_err(|_| "a malformed signature")?;
    let signed = match &first[..2] {
        b"ED" => key.key.verify(&Blake2b512::digest(data), &sig),
        b"Ed" => key.key.verify(data, &sig),
        _ => return Err("an unknown signature algorithm".into()),
    };
    signed.map_err(|_| format!("the signature by key {} does not match", key.id_hex()))?;
    let mut over = first[10..74].to_vec();
    over.extend_from_slice(comment.as_bytes());
    let global = Signature::from_slice(&global).map_err(|_| "a malformed comment signature")?;
    key.key
        .verify(&over, &global)
        .map_err(|_| "the trusted comment's signature does not match".to_string())?;
    Ok((key, comment))
}
