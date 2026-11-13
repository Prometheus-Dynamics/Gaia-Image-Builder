//! The flash node: the unsigned and signed outputs, what a decoder and the
//! stock xz program read back from them, determinism, and the refusals.

use ed25519_dalek::VerifyingKey;
use gaia_flash_image::dictionary;
use gaia_flash_image::flash_image_with;
use gaia_flash_image::signing;
use gaia_step::Ctx;
use gaia_store::{Digest, Store};
use gaia_types::{Artifact, Image, ImageKind, ObjectKind, Size, StoreRef};
use sha2::{Digest as _, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const BOARD: &str = "flash-test";
const FORMAT: &str = "disk";
const KEY_VAR: &str = "GAIA_FLASH_TEST_KEY";
const SEED: [u8; 32] = [7; 32];
const EIGHT_MIB: u64 = 8 * 1024 * 1024;

/// A raw image of `len` bytes: pseudo-random runs mixed with zeros, so the
/// compressed stream has real content.
fn raw_image(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let run = (state % 4096) as usize;
        let byte = (state >> 32) as u8;
        if state.is_multiple_of(3) {
            out.extend(std::iter::repeat_n(0_u8, run));
        } else {
            out.extend((0..run).map(|i| byte.wrapping_add(i as u8)));
        }
    }
    out.truncate(len);
    out
}

/// The raw image as a file in `store`, with its image value.
fn store_image(store_root: &Path, bytes: &[u8]) -> Image {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("disk.img");
    std::fs::write(&path, bytes).expect("write");
    let store = Store::open(store_root).expect("store");
    let object = store.put_file(&path).expect("put");
    Image {
        kind: ImageKind::Disk,
        object: object.into(),
        size: Size(bytes.len() as u64),
        partitions: Vec::new(),
    }
}

fn store_path(root: &Path) -> String {
    root.display().to_string()
}

fn secret_file_text() -> String {
    gaia_sign::minisign::SecretKey::from_seed(&SEED, [1, 2, 3, 4, 5, 6, 7, 8])
        .to_file()
        .to_string()
}

fn key_env(text: String) -> impl Fn(&str) -> Option<String> {
    move |name| (name == KEY_VAR).then(|| text.clone())
}

fn output(store_root: &Path, artifact: &Artifact) -> PathBuf {
    let store = Store::open(store_root).expect("store");
    artifact.object.path_in(&store).expect("in the store")
}

/// The first stream alone, decoded and stopped at its end (a single-stream
/// decoder would reject the second stream after it).
fn decode_first_stream(path: &Path) -> Vec<u8> {
    use xz2::stream::{Action, Status, Stream};
    let input = std::fs::read(path).expect("read");
    let mut stream = Stream::new_stream_decoder(u64::MAX, 0).expect("decoder");
    let mut out = Vec::new();
    loop {
        out.reserve(1 << 20);
        let consumed = stream.total_in() as usize;
        let status = stream
            .process_vec(&input[consumed..], &mut out, Action::Finish)
            .expect("first stream decodes");
        if status == Status::StreamEnd {
            return out;
        }
    }
}

fn decode_all(path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    xz2::read::XzDecoder::new_multi_decoder(std::fs::File::open(path).expect("open"))
        .read_to_end(&mut out)
        .expect("all streams decode");
    out
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn stock_xz() -> bool {
    let present = Path::new("/usr/bin/xz").is_file();
    if !present {
        eprintln!("note: /usr/bin/xz is not present; the stock xz checks are skipped");
    }
    present
}

#[test]
fn unsigned_output_is_the_raw_image_and_reads_back_8_mib() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = store_path(&root.path().join("store"));
    let raw = raw_image(3 * 1024 * 1024 + 17);
    let image = store_image(Path::new(&store), &raw);
    let artifact = flash_image_with(&store, image, BOARD, FORMAT, None, &|_| None, &Ctx::new())
        .expect("flash");
    assert_eq!(artifact.kind.to_string(), "disk+xz");
    let path = output(Path::new(&store), &artifact);
    assert_eq!(decode_first_stream(&path), raw);
    assert_eq!(decode_all(&path), raw, "unsigned: one stream only");
    assert_eq!(dictionary::read_back(&path), Ok(EIGHT_MIB));
    if stock_xz() {
        assert!(
            Command::new("/usr/bin/xz")
                .arg("-t")
                .arg(&path)
                .status()
                .expect("xz")
                .success()
        );
    }
}

#[test]
fn signed_output_is_two_streams_and_the_record_verifies() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = store_path(&root.path().join("store"));
    let raw = raw_image(2 * 1024 * 1024 + 5);
    let image = store_image(Path::new(&store), &raw);
    let text = secret_file_text();
    let artifact = flash_image_with(
        &store,
        image,
        BOARD,
        FORMAT,
        Some(KEY_VAR),
        &key_env(text),
        &Ctx::new(),
    )
    .expect("flash");
    let path = output(Path::new(&store), &artifact);

    assert_eq!(decode_first_stream(&path), raw, "stream 1 is the image");
    let all = decode_all(&path);
    assert_eq!(
        all.len(),
        raw.len() + 108,
        "stream 2 is one 108-byte record"
    );
    let record = &all[raw.len()..];
    assert_eq!(&record[..8], signing::MAGIC);
    assert_eq!(record[8], 1, "Ed25519");
    assert_eq!(&record[9..12], &[0, 0, 0]);

    let public = VerifyingKey::from(&ed25519_dalek::SigningKey::from_bytes(&SEED));
    assert_eq!(&record[12..44], &signing::key_id(public.as_bytes()));
    let message =
        signing::message(BOARD, FORMAT, raw.len() as u64, &sha256(&raw)).expect("message");
    let signature = ed25519_dalek::Signature::from_slice(&record[44..108]).expect("64 bytes");
    public
        .verify_strict(&message, &signature)
        .expect("the record's signature verifies against the rebuilt message");
    assert_eq!(dictionary::read_back(&path), Ok(EIGHT_MIB));

    if stock_xz() {
        let xz = |args: &[&str]| {
            Command::new("/usr/bin/xz")
                .args(args)
                .arg(&path)
                .output()
                .expect("xz")
        };
        assert!(
            xz(&["-t"]).status.success(),
            "xz -t accepts the two-stream file"
        );
        let single = xz(&["-dc", "--single-stream"]);
        assert!(single.status.success());
        assert_eq!(single.stdout, raw, "xz -dc --single-stream gives the image");
    }
}

#[test]
fn the_same_inputs_give_the_same_bytes_in_two_fresh_stores() {
    let raw = raw_image(1024 * 1024 + 3);
    let mut results = Vec::new();
    for _ in 0..2 {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store_path(&root.path().join("store"));
        let image = store_image(Path::new(&store), &raw);
        let artifact = flash_image_with(
            &store,
            image,
            BOARD,
            FORMAT,
            Some(KEY_VAR),
            &key_env(secret_file_text()),
            &Ctx::new(),
        )
        .expect("flash");
        let path = output(Path::new(&store), &artifact);
        results.push((
            artifact.object.digest.clone(),
            std::fs::read(path).expect("read"),
        ));
    }
    assert_eq!(results[0].0, results[1].0, "the same digest");
    assert_eq!(results[0].1, results[1].1, "the same bytes");
}

#[test]
fn an_empty_key_name_is_unsigned_and_a_key_path_signs_the_same() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = store_path(&root.path().join("store"));
    let raw = raw_image(64 * 1024);
    let image = store_image(Path::new(&store), &raw);
    let unsigned = flash_image_with(
        &store,
        image.clone(),
        BOARD,
        FORMAT,
        Some(""),
        &|_| None,
        &Ctx::new(),
    )
    .expect("unsigned");
    assert_eq!(decode_all(&output(Path::new(&store), &unsigned)), raw);

    let key_file = root.path().join("secret.key");
    std::fs::write(&key_file, secret_file_text()).expect("write");
    let by_path = flash_image_with(
        &store,
        image.clone(),
        BOARD,
        FORMAT,
        Some(KEY_VAR),
        &key_env(key_file.display().to_string()),
        &Ctx::new(),
    )
    .expect("signed by path");
    let by_text = flash_image_with(
        &store,
        image,
        BOARD,
        FORMAT,
        Some(KEY_VAR),
        &key_env(secret_file_text()),
        &Ctx::new(),
    )
    .expect("signed by text");
    assert_eq!(
        by_path.object.digest, by_text.object.digest,
        "a key path and its text give one file"
    );
}

#[test]
fn refusals_are_by_name() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = store_path(&root.path().join("store"));
    let raw = raw_image(4096);
    let image = store_image(Path::new(&store), &raw);
    let run = |board: &str,
               format: &str,
               image: Image,
               key: Option<&str>,
               env: &dyn Fn(&str) -> Option<String>| {
        flash_image_with(&store, image, board, format, key, env, &Ctx::new())
            .expect_err("refused")
            .to_string()
    };
    let none = |_: &str| None;

    let error = run("", FORMAT, image.clone(), None, &none);
    assert!(error.contains("empty board"), "{error}");
    let error = run(BOARD, "", image.clone(), None, &none);
    assert!(error.contains("empty format"), "{error}");

    let tree = Image {
        object: StoreRef {
            kind: ObjectKind::Tree,
            digest: image.object.digest.clone(),
        },
        ..image.clone()
    };
    assert!(run(BOARD, FORMAT, tree, None, &none).contains("not a file in the store"));
    let missing = Image {
        object: StoreRef {
            kind: ObjectKind::File,
            digest: Digest::of_bytes(b"not in this store").to_string(),
        },
        ..image.clone()
    };
    assert!(run(BOARD, FORMAT, missing, None, &none).contains("not a file in the store"));

    let error = run(
        BOARD,
        FORMAT,
        image.clone(),
        Some("GAIA_FLASH_TEST_UNSET_VAR"),
        &none,
    );
    assert!(
        error.contains("GAIA_FLASH_TEST_UNSET_VAR is not set"),
        "{error}"
    );

    let not_base64 = "untrusted comment: a key\nnot-base64!!\n";
    let error = run(
        BOARD,
        FORMAT,
        image.clone(),
        Some(KEY_VAR),
        &key_env(not_base64.into()),
    );
    assert!(error.contains("not base64"), "{error}");
    assert!(
        !error.contains("not-base64"),
        "the key text is not in the error: {error}"
    );

    let short = "untrusted comment: a key\nQUFBQQ==\n";
    let error = run(BOARD, FORMAT, image, Some(KEY_VAR), &key_env(short.into()));
    assert!(error.contains("not a minisign"), "{error}");
    assert!(!error.contains("QUFB"), "{error}");
}
