//! Sign, verify, tamper, and look for key bytes everywhere.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use gaia_sign::minisign::SecretKey;
use gaia_sign::{sign, verify};
use gaia_step::Ctx;
use gaia_store::Store;
use gaia_types::{Artifact, ArtifactKind};
use std::path::{Path, PathBuf};
use std::process::Command;

const SEED: [u8; 32] = [7; 32];
const OTHER_SEED: [u8; 32] = [9; 32];

fn key(seed: &[u8; 32], id: u8) -> SecretKey {
    SecretKey::from_seed(seed, [id, 1, 2, 3, 4, 5, 6, 7])
}

fn public_file(key: &SecretKey) -> String {
    format!(
        "untrusted comment: minisign public key {}\n{}\n",
        key.public().id_hex(),
        key.public().to_line()
    )
}

fn stored(store: &Path, name: &str, bytes: &[u8]) -> Artifact {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write");
    let store = Store::open(store).expect("store");
    Artifact {
        kind: ArtifactKind::Archive,
        object: store.put_file(&path).expect("put").into(),
    }
}

/// Each test runs in its own process, so the key variable is set there.
fn in_child(test: &str, key_text: &str) -> bool {
    if std::env::var("GAIA_SIGN_CHILD").is_ok() {
        return false;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let key_path = dir.path().join("secret.key");
    std::fs::write(&key_path, key_text).expect("write");
    let status = Command::new(std::env::current_exe().expect("exe"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env("GAIA_SIGN_CHILD", "1")
        .env("GAIA_TEST_SIGNING_KEY", &key_path)
        .status()
        .expect("child");
    assert!(status.success(), "{test} failed in its child");
    true
}

#[test]
fn sign_then_verify_and_a_tampered_byte_fails() {
    let secret = key(&SEED, 1);
    if in_child(
        "sign_then_verify_and_a_tampered_byte_fails",
        &secret.to_file(),
    ) {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store");
    let s = store.display().to_string();
    let image = stored(&store, "image.img.xz", b"an image, compressed");
    let public = public_file(&secret);
    let signature = sign(
        &s,
        image.clone(),
        &public,
        "GAIA_TEST_SIGNING_KEY",
        &Ctx::new(),
    )
    .expect("signs");
    let store_handle = Store::open(&store).expect("store");
    let sig_path = signature.object.path_in(&store_handle).expect("in store");
    println!("{}", std::fs::read_to_string(&sig_path).expect("read"));

    // Verified by the trusted list that has its key.
    let other = public_file(&key(&OTHER_SEED, 2));
    let checked = verify(
        &s,
        image.clone(),
        signature.clone(),
        vec![other.clone(), public.clone()],
        &Ctx::new(),
    )
    .expect("verifies");
    assert_eq!(checked, image);

    // One byte changed: the signature no longer holds.
    let tampered = stored(&store, "tampered.img.xz", b"an image, compressee");
    let error = verify(
        &s,
        tampered,
        signature.clone(),
        vec![public.clone()],
        &Ctx::new(),
    )
    .expect_err("tampered");
    println!("tampered: {error}");
    assert!(error.to_string().contains("does not match"));

    // A list without its key does not trust it.
    let error = verify(
        &s,
        image.clone(),
        signature.clone(),
        vec![other],
        &Ctx::new(),
    )
    .expect_err("untrusted");
    println!("untrusted: {error}");
    assert!(error.to_string().contains("not trusted"));

    // The same artifact and key: the same signature, found, not remade.
    let again = sign(
        &s,
        image.clone(),
        &public,
        "GAIA_TEST_SIGNING_KEY",
        &Ctx::new(),
    )
    .expect("again");
    assert_eq!(again, signature);

    // A key that is not the product's is refused.
    let wrong = public_file(&key(&OTHER_SEED, 2));
    let error = sign(
        &s,
        image.clone(),
        &wrong,
        "GAIA_TEST_SIGNING_KEY",
        &Ctx::new(),
    )
    .expect_err("wrong key");
    println!("wrong key: {error}");
    assert!(error.to_string().contains("not the product's key"));

    // The host's minisign agrees, both ways, when it is installed.
    if let Some(minisign) = ["/usr/bin/minisign", "/usr/local/bin/minisign"]
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
    {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pub"), &public).expect("write");
        let data = image.object.path_in(&store_handle).expect("in");
        let out = Command::new(minisign)
            .args(["-V", "-p"])
            .arg(dir.path().join("pub"))
            .arg("-m")
            .arg(&data)
            .arg("-x")
            .arg(&sig_path)
            .output()
            .expect("minisign");
        println!(
            "minisign -V: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let theirs = dir.path().join("theirs.minisig");
        let out = Command::new(minisign)
            .args(["-S", "-s"])
            .arg(std::env::var("GAIA_TEST_SIGNING_KEY").expect("key"))
            .arg("-m")
            .arg(&data)
            .arg("-x")
            .arg(&theirs)
            .args(["-t", "by minisign"])
            .output()
            .expect("minisign");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = std::fs::read_to_string(&theirs).expect("read");
        let (_, comment) = gaia_sign::minisign::verify(
            &std::fs::read(&data).expect("read"),
            &text,
            &[secret.public()],
        )
        .expect("checks minisign's");
        println!("minisign -S checked here: trusted comment {comment:?}");
    } else {
        println!("minisign not installed: not compared");
    }

    // No key bytes anywhere under the store root's parent (store, logs).
    let needles = needles(&SEED);
    let mut files = Vec::new();
    every_file(root.path(), &mut files);
    assert!(!files.is_empty());
    for file in &files {
        let bytes = std::fs::read(file).expect("read");
        for needle in &needles {
            assert!(!contains(&bytes, needle), "key bytes in {}", file.display());
        }
    }
    println!(
        "{} files searched for the secret key: none hold it",
        files.len()
    );
}

#[test]
fn a_missing_or_protected_key_names_the_variable_only() {
    let secret = key(&SEED, 1);
    let mut file = secret.to_file().to_string();
    // Mark it password-protected (kdf "Sc"): refused, without key bytes.
    let line = file.lines().nth(1).expect("line").to_string();
    let mut bytes = STANDARD.decode(&line).expect("b64");
    bytes[2..4].copy_from_slice(b"Sc");
    file = file.replace(&line, &STANDARD.encode(&bytes));
    if in_child("a_missing_or_protected_key_names_the_variable_only", &file) {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let s = root.path().join("store").display().to_string();
    let image = stored(&root.path().join("store"), "a", b"a");
    let public = public_file(&secret);
    let error = sign(
        &s,
        image.clone(),
        &public,
        "GAIA_TEST_SIGNING_KEY",
        &Ctx::new(),
    )
    .expect_err("protected");
    println!("{error}");
    assert!(error.to_string().contains("password-protected"));
    let error = sign(&s, image, &public, "GAIA_NOT_SET_ANYWHERE", &Ctx::new()).expect_err("unset");
    println!("{error}");
    assert!(
        error
            .to_string()
            .contains("GAIA_NOT_SET_ANYWHERE is not set")
    );
    for needle in needles(&SEED) {
        assert!(!contains(error.to_string().as_bytes(), &needle));
    }
}

/// The secret key's seed and full key, raw, hex and base64.
fn needles(seed: &[u8; 32]) -> Vec<Vec<u8>> {
    let pair = ed25519_pair(seed);
    let seed = seed.to_vec();
    let mut out = Vec::new();
    for raw in [seed, pair] {
        out.push(STANDARD.encode(&raw).into_bytes());
        out.push(
            raw.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .into_bytes(),
        );
        out.push(raw);
    }
    out
}

fn ed25519_pair(seed: &[u8; 32]) -> Vec<u8> {
    // The secret key file holds the seed then the public key.
    let file = key(seed, 1).to_file();
    let line = file.lines().nth(1).expect("line");
    STANDARD.decode(line).expect("b64")[62..126].to_vec()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn every_file(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read") {
        let entry = entry.expect("entry");
        let kind = entry.file_type().expect("type");
        if kind.is_dir() {
            every_file(&entry.path(), out);
        } else if kind.is_file() {
            out.push(entry.path());
        }
    }
}

/// Files minisign 0.12 made: a throwaway unencrypted key (`-G -W`, which
/// writes no checksum), its public key, and its signature over a message.
mod minisign_012 {
    use gaia_sign::minisign::{PublicKey, SecretKey, verify};

    const KEY: &str = include_str!("fixtures/minisign-0.12.key");
    const PUBLIC: &str = include_str!("fixtures/minisign-0.12.pub");
    const MESSAGE: &[u8] = include_bytes!("fixtures/message.txt");
    const SIGNATURE: &str = include_str!("fixtures/message.txt.minisig");

    #[test]
    fn its_unencrypted_key_signs_and_its_signature_verifies() {
        let public = PublicKey::parse(PUBLIC).expect("public key");
        let secret = SecretKey::parse(KEY).expect("a minisign 0.12 key parses");
        assert_eq!(secret.public().id_hex(), public.id_hex());
        let trusted = [public];
        let ours = secret.sign(MESSAGE, "gaia");
        let (by, comment) = verify(MESSAGE, &ours, &trusted).expect("ours verifies");
        assert_eq!(
            (by.id_hex(), comment.as_str()),
            (trusted[0].id_hex(), "gaia")
        );
        let (_, comment) = verify(MESSAGE, SIGNATURE, &trusted).expect("minisign's verifies");
        assert_eq!(comment, "minisign 0.12 fixture");
    }

    #[test]
    fn a_wrong_checksum_or_a_wrong_half_is_refused() {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        let line = KEY.lines().nth(1).expect("key line");
        let bytes = STANDARD.decode(line).expect("base64");
        let with = |at: usize| {
            let mut b = bytes.clone();
            b[at] ^= 1;
            KEY.replace(line, &STANDARD.encode(&b))
        };
        // A checksum that is set must match.
        let Err(error) = SecretKey::parse(&with(157)) else {
            panic!("byte 157 changed, and the key still parsed")
        };
        assert!(error.contains("checksum does not match"), "{error}");
        // With no checksum, a changed public half no longer matches the seed.
        let Err(error) = SecretKey::parse(&with(100)) else {
            panic!("byte 100 changed, and the key still parsed")
        };
        assert!(error.contains("halves do not match"), "{error}");
    }
}
