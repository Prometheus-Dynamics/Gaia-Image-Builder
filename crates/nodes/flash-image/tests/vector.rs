//! The contract's test vector (W1-actions.md, "Signed message"): the message
//! bytes, the signature, the key id and the 108-byte record must be the
//! vector's, byte for byte. The key is a test key: the seed is 32 bytes of 0x07.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use gaia_flash_image::signing;

const SEED: [u8; 32] = [7; 32];
const BOARD: &str = "flash-test";
const FORMAT: &str = "disk";
const SIZE: u64 = 16384;
const SHA256: &str = "f30a82fe97de9b784dd7b5735958b28dbfa95f94c0ba6d4834b441ab78b65c58";
const MESSAGE: &str = "6c656d6e6f732d66697273742d666c61736800010000000a000000666c6173682d74657374040000006469736b0040000000000000f30a82fe97de9b784dd7b5735958b28dbfa95f94c0ba6d4834b441ab78b65c58";
const PUBLIC: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";
const SIGNATURE: &str = "bd894b37df1a05915c0c6baa7606d57c8cc2a6aa10a01451a3e4f67b8edfd517df6794fbcdeeaadfd7f46e099cea00be896e99445a8942f5e7592ec31c1b6c09";
const KEY_ID: &str = "fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889";
const RECORD: &str = "4c4d4e535349473101000000fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889bd894b37df1a05915c0c6baa7606d57c8cc2a6aa10a01451a3e4f67b8edfd517df6794fbcdeeaadfd7f46e099cea00be896e99445a8942f5e7592ec31c1b6c09";

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn sha256() -> [u8; 32] {
    unhex(SHA256).try_into().expect("32 bytes")
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&SEED)
}

fn message() -> Vec<u8> {
    signing::message(BOARD, FORMAT, SIZE, &sha256()).expect("message")
}

#[test]
fn the_message_bytes_are_the_vectors() {
    assert_eq!(message(), unhex(MESSAGE));
}

#[test]
fn the_public_key_is_the_vectors() {
    assert_eq!(
        key().verifying_key().as_bytes(),
        &<[u8; 32]>::try_from(unhex(PUBLIC)).expect("32")
    );
}

#[test]
fn the_signature_is_the_vectors_and_verifies() {
    let signature = key().sign(&message());
    assert_eq!(signature.to_bytes().to_vec(), unhex(SIGNATURE));
    let public: VerifyingKey = key().verifying_key();
    public
        .verify_strict(&message(), &signature)
        .expect("verifies");
}

#[test]
fn the_key_id_is_the_vectors() {
    let public = key().verifying_key();
    assert_eq!(signing::key_id(public.as_bytes()).to_vec(), unhex(KEY_ID));
}

#[test]
fn the_record_is_the_vectors_byte_for_byte() {
    let signature: [u8; 64] = unhex(SIGNATURE).try_into().expect("64 bytes");
    let key_id: [u8; 32] = unhex(KEY_ID).try_into().expect("32 bytes");
    let record = signing::record(&key_id, &signature);
    assert_eq!(record.len(), 108);
    assert_eq!(record.to_vec(), unhex(RECORD));
}

#[test]
fn a_minisign_secret_key_file_gives_the_vectors_key() {
    let file = gaia_sign::minisign::SecretKey::from_seed(&SEED, [1, 2, 3, 4, 5, 6, 7, 8]).to_file();
    let parsed = signing::secret_key(&file).expect("decodes");
    assert_eq!(
        parsed.verifying_key().as_bytes(),
        key().verifying_key().as_bytes()
    );
}

#[test]
fn a_secret_key_that_is_not_minisign_is_refused_without_its_text() {
    let error = signing::secret_key("untrusted comment: x\nQUFBQQ==\n").expect_err("refused");
    assert!(!error.contains("QUFB"), "{error}");
}
