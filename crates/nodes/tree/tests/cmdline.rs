//! `gaia.cmdline`: the step, with files of the form a generator writes and
//! a verity table of the form `gaia.verity` writes.

use gaia_step::{Ctx, Store};
use gaia_tree::cmdline;
use gaia_types::{Artifact, ArtifactKind, Tree};

const HASH: &str = "d49facf3f0db6b2a180af10a6685f8c75b94e3436ba2fb64c654f36b3b5df407";
const WORDS: &str = "mod.owned=vendor,dev-a,vendor,dev-b mod.pisp_be=vendor,dev-a\n";
const SLOT: &str = "os.slot=base os.roothash=@roothash@ os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 panic=5\n";

fn run(base: &str, extra: &str, slot_path: &str, limit: i64) -> Result<String, String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_path = dir.path().join("store").display().to_string();
    let store = Store::open(&store_path).expect("store");
    let generated = dir.path().join("generated");
    std::fs::create_dir_all(&generated).expect("mkdir");
    std::fs::write(generated.join("cmdline.txt"), WORDS).expect("write");
    std::fs::write(generated.join("cmdline-slot.txt"), SLOT).expect("write");
    let table = dir.path().join("table");
    std::fs::write(
        &table,
        format!("hash_type 1\nalgorithm sha256\nroot_hash {HASH}\nsalt 00\n"),
    )
    .expect("write");
    let made = cmdline(
        &store_path,
        base,
        Tree {
            object: store.put_tree(&generated).expect("put").into(),
            entries: Vec::new(),
        },
        "cmdline.txt",
        slot_path,
        extra,
        &Artifact {
            kind: ArtifactKind::parse("verity-table"),
            object: store.put_file(&table).expect("put").into(),
        },
        limit,
        &Ctx::new(),
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(made.kind.to_string(), "cmdline");
    Ok(std::fs::read_to_string(made.object.path_in(&store).expect("in the store")).expect("read"))
}

#[test]
fn the_file_is_the_whole_line_with_the_tables_root_hash() {
    let line = run("console=ttyAMA0,115200", "quiet", "cmdline-slot.txt", 2048).expect("made");
    assert_eq!(
        line,
        format!(
            "console=ttyAMA0,115200 mod.owned=vendor,dev-a,vendor,dev-b mod.pisp_be=vendor,dev-a quiet os.slot=base os.roothash={HASH} os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 panic=5\n"
        )
    );
    // No slot words: no root hash is read, and the line is the rest.
    let line = run("console=ttyAMA0,115200", "", "", 0).expect("made");
    assert_eq!(
        line,
        "console=ttyAMA0,115200 mod.owned=vendor,dev-a,vendor,dev-b mod.pisp_be=vendor,dev-a\n"
    );
}

#[test]
fn a_word_the_slot_owns_and_a_line_too_long_fail_the_step() {
    let why = run("console=ttyAMA0 panic=30", "", "cmdline-slot.txt", 0).expect_err("panic");
    assert!(
        why.contains("`panic=30`, a word of the board's base"),
        "{why}"
    );
    let why = run("", "os.debug=1", "cmdline-slot.txt", 0).expect_err("namespace");
    assert!(why.contains("a word of the image's additions"), "{why}");
    let why = run("console=ttyAMA0", "", "cmdline-slot.txt", 120).expect_err("too long");
    assert!(why.contains("is over 120 bytes"), "{why}");
    assert!(why.contains("a word of the slot"), "{why}");
    let why = run("", "", "missing.txt", 0).expect_err("no such file");
    assert!(why.contains("the slot's words, missing.txt"), "{why}");
}
