//! Signing artifacts and checking signatures, in minisign's format.
//!
//! - **`gaia.sign`**: signs a stored artifact with the key the product
//!   names: its public key, and the environment variable that holds the
//!   secret key file's path (or the file's text). The secret key is read in
//!   memory only, checked against the named public key, and wiped after.
//!   It never reaches the store, a step key, a log or an error. The
//!   signature's trusted comment is the artifact's digest, so the same
//!   artifact and key give the same signature, and a remade step finds it.
//! - **`gaia.verify`**: checks a signature over an artifact against a list
//!   of trusted public keys, and gives the artifact back only when it
//!   holds.
//!
//! Signatures are prehashed Ed25519 (BLAKE2b-512), the format `minisign
//! -S` writes and `minisign -V` checks. Secret keys must be unencrypted
//! (`minisign -G -W`): key material comes from the environment, which
//! already keeps it out of the build.

pub mod minisign;

/// The libraries this node links, by the version Cargo resolved. Its step key
/// names them, and `tests/libs.rs` checks each version against Cargo.lock.
pub const LIBS: &[(&str, &str)] = &[
    ("ed25519-dalek", "2.2.0"),
    ("blake2", "0.10.6"),
    ("base64", "0.22.1"),
];

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind};
use minisign::{PublicKey, SecretKey};
use std::path::Path;
use zeroize::Zeroizing;

const SIGN: &str = "gaia.sign";
const VERIFY: &str = "gaia.verify";

/// The signature of `artifact` by the key `public_key` names, whose secret
/// key the environment variable `key_env` holds.
#[node(
    id = "gaia.sign",
    inputs(store, artifact, public_key, key_env),
    outputs(signature)
)]
pub fn sign(
    store: &str,
    artifact: Artifact,
    public_key: &str,
    key_env: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(SIGN))?;
    let public = PublicKey::parse(public_key).map_err(|e| StepError::body(SIGN, e))?;
    let object = artifact
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(SIGN, "an artifact that is not in the store"))?;
    let comment = format!("{} {}", artifact.kind, object.digest);
    let key = KeyBuilder::new(SIGN, 1)
        .config("public key", public.to_line())
        .config("comment", &comment)
        .input("artifact", object.digest)
        .libs(LIBS)
        .finish();
    let made = Step::new(&store, cx, SIGN, key).look_up_or_make(
        &[("signature", Path::new("signature.minisig"))],
        |work| {
            let secret = secret_key(key_env)?;
            if secret.public() != public {
                return Err(StepError::body(
                    SIGN,
                    format!(
                        "{key_env} holds key {}, not the product's key {}",
                        secret.public().id_hex(),
                        public.id_hex()
                    ),
                ));
            }
            let data = std::fs::read(store.path(&object)).map_err(|e| StepError::body(SIGN, e))?;
            std::fs::write(
                work.dir().join("signature.minisig"),
                secret.sign(&data, &comment),
            )
            .map_err(|e| StepError::body(SIGN, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("signature"),
        object: made.outputs["signature"].into(),
    })
}

/// The secret key the environment variable `name` holds: a key file's path,
/// or the key file's text.
fn secret_key(name: &str) -> Result<SecretKey, StepError> {
    let value = Zeroizing::new(std::env::var(name).map_err(|_| {
        StepError::body(SIGN, format!("{name} is not set; it names the signing key"))
    })?);
    let text = if value.trim_start().starts_with("untrusted comment:") {
        value
    } else {
        Zeroizing::new(std::fs::read_to_string(value.trim()).map_err(|e| {
            StepError::body(SIGN, format!("the key file {name} names: {}", e.kind()))
        })?)
    };
    SecretKey::parse(&text).map_err(|e| StepError::body(SIGN, format!("{name}: {e}")))
}

/// `artifact`, once `signature` over it checks against one of `trusted`
/// (public key files' text, or their base64 lines).
#[node(
    id = "gaia.verify",
    inputs(store, artifact, signature, trusted),
    outputs(artifact)
)]
pub fn verify(
    store: &str,
    artifact: Artifact,
    signature: Artifact,
    trusted: Vec<String>,
    _cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(VERIFY))?;
    let keys: Vec<PublicKey> = trusted
        .iter()
        .map(|k| PublicKey::parse(k))
        .collect::<Result<_, _>>()
        .map_err(|e| StepError::body(VERIFY, format!("a trusted key: {e}")))?;
    let read = |a: &Artifact| {
        let path = a
            .object
            .path_in(&store)
            .ok_or_else(|| StepError::body(VERIFY, "an artifact that is not in the store"))?;
        std::fs::read(path).map_err(|e| StepError::body(VERIFY, e))
    };
    let data = read(&artifact)?;
    let text = String::from_utf8(read(&signature)?)
        .map_err(|_| StepError::body(VERIFY, "a signature that is not text"))?;
    minisign::verify(&data, &text, &keys).map_err(|e| StepError::body(VERIFY, e))?;
    Ok(artifact)
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 2] = [&SIGN_NODE, &VERIFY_NODE];
