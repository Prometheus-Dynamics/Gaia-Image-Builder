use super::*;
use gaia_image_providers::{CONTENT_DIGESTS_FILE, record_collect_dir_digests};
use std::time::{Duration, SystemTime};

const FAKE_DIGEST: &str = "ab";

fn temp_collect_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("gaia-reuse-content-tests")
        .join(format!("{name}-{nonce}"));
    fs::create_dir_all(&dir).expect("collect dir");
    dir
}

fn fake_digest() -> String {
    FAKE_DIGEST.repeat(32)
}

#[test]
fn recorded_manifest_signature_equals_hashed_signature() {
    let dir = temp_collect_dir("equals-hash");
    let image = dir.join("disk.img");
    fs::write(&image, b"image bytes").expect("image");
    let hashed = content_state_signature(&image);

    assert_eq!(cached_content_signature(&dir, &image), hashed);
    record_collect_dir_digests(&dir).expect("record");
    assert_eq!(cached_content_signature(&dir, &image), hashed);
}

#[test]
fn collect_dir_digest_ignores_the_manifest_itself() {
    let dir = temp_collect_dir("manifest-excluded");
    fs::write(dir.join("disk.img"), b"image bytes").expect("image");
    fs::write(dir.join("image-provider.txt"), b"provider=x\n").expect("marker");
    let before = collect_dir_digest(&dir);

    record_collect_dir_digests(&dir).expect("record");

    assert!(dir.join(CONTENT_DIGESTS_FILE).is_file());
    assert_eq!(collect_dir_digest(&dir), before);
}

#[test]
fn recorded_digest_is_used_without_reading_the_file() {
    let dir = temp_collect_dir("recorded-wins");
    let image = dir.join("disk.img");
    fs::write(&image, b"image bytes").expect("image");
    record_collect_dir_digests(&dir).expect("record");

    // Replace the recorded digest, keeping size and mtime. A matching entry
    // must win over the file's real contents, which proves the file is not read.
    let manifest = dir.join(CONTENT_DIGESTS_FILE);
    let text = fs::read_to_string(&manifest).expect("manifest");
    let real = content_state_signature(&image);
    let real_hex = real.trim_start_matches("sha256:");
    fs::write(&manifest, text.replace(real_hex, &fake_digest())).expect("tamper");

    assert_eq!(
        cached_content_signature(&dir, &image),
        format!("sha256:{}", fake_digest())
    );
}

#[test]
fn changed_mtime_forces_a_rehash() {
    let dir = temp_collect_dir("mtime-rehash");
    let image = dir.join("disk.img");
    fs::write(&image, b"image bytes").expect("image");
    record_collect_dir_digests(&dir).expect("record");
    let manifest = dir.join(CONTENT_DIGESTS_FILE);
    let real = content_state_signature(&image);
    let real_hex = real.trim_start_matches("sha256:").to_string();
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .expect("manifest")
            .replace(&real_hex, &fake_digest()),
    )
    .expect("tamper");

    // Same size, different mtime: the manifest entry no longer applies.
    let moved = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(&image)
        .expect("open")
        .set_modified(moved)
        .expect("set mtime");

    assert_eq!(cached_content_signature(&dir, &image), real);
}

#[test]
fn changed_size_forces_a_rehash() {
    let dir = temp_collect_dir("size-rehash");
    let image = dir.join("disk.img");
    fs::write(&image, b"image bytes").expect("image");
    record_collect_dir_digests(&dir).expect("record");
    fs::write(&image, b"different, longer image bytes").expect("rewrite");

    assert_eq!(
        cached_content_signature(&dir, &image),
        content_state_signature(&image)
    );
}
