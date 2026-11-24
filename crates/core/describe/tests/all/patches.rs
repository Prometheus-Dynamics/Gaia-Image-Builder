//! A board's patch lists with its family's: the family's entries first, then
//! the board's; `inherit_patches = false` replaces; an entry stated twice,
//! and the key where it means nothing, are refused.

use gaia_describe::{Board, Error, FileRef};

fn files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    dir
}

const FAMILY: &str = "version = 1\n[target]\narch = \"aarch64\"\n[sources.series]\ngit = \"https://example.org/series.git\"\nref = \"abc\"\n";

/// A family with `family` after its head and a board with `board` after its
/// own, read as the board.
fn read(family: &str, board: &str) -> Result<Board, Error> {
    let dir = files(&[
        ("fam/family.toml", &format!("{FAMILY}{family}")),
        ("fam/f.patch", "f"),
        ("b/b.patch", "b"),
        ("b/b2.patch", "b2"),
        (
            "b/board.toml",
            &format!("version = 1\nname = \"b\"\nextends = \"../fam/family.toml\"\n{board}"),
        ),
    ]);
    Board::read(dir.path().join("b/board.toml"))
}

/// Each entry as a short word: a source's path, or a local file's name.
fn names(patches: &Option<Vec<FileRef>>) -> Vec<String> {
    patches
        .iter()
        .flatten()
        .map(|entry| match entry {
            FileRef::InSource { source, path } => format!("{source}:{path}"),
            FileRef::Local { file } => file
                .path()
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned(),
            other => format!("{other:?}"),
        })
        .collect()
}

const SERIES: &str =
    "patches = [{ source = \"series\", path = \"patches\" }, { file = \"f.patch\" }]\n";

#[test]
fn a_boards_patches_follow_its_familys() {
    let board = read(
        &format!("[kernel]\n{SERIES}[bootloader]\n{SERIES}[bootloader.trusted_firmware]\n{SERIES}"),
        "[kernel]\npatches = [{ file = \"b.patch\" }, { file = \"b2.patch\" }]\n[bootloader]\npatches = [{ file = \"b.patch\" }]\n[bootloader.trusted_firmware]\npatches = [{ file = \"b2.patch\" }]\n",
    )
    .expect("reads");
    let kernel = board.kernel.expect("kernel");
    assert_eq!(
        names(&kernel.patches),
        ["series:patches", "f.patch", "b.patch", "b2.patch"]
    );
    assert_eq!(kernel.inherit_patches, None);
    let bootloader = board.bootloader.expect("bootloader");
    assert_eq!(
        names(&bootloader.patches),
        ["series:patches", "f.patch", "b.patch"]
    );
    let tf = bootloader.trusted_firmware.expect("trusted firmware");
    assert_eq!(
        names(&tf.patches),
        ["series:patches", "f.patch", "b2.patch"]
    );
}

#[test]
fn a_list_only_one_of_them_states_is_that_one() {
    let board = read(
        &format!("[kernel]\n{SERIES}"),
        "[kernel]\ndevice_tree = \"x/y\"\n",
    )
    .expect("reads");
    assert_eq!(
        names(&board.kernel.expect("kernel").patches),
        ["series:patches", "f.patch"]
    );
    let board = read(
        "[kernel]\ndefconfig = \"d\"\n",
        "[kernel]\npatches = [{ file = \"b.patch\" }]\n",
    )
    .expect("reads");
    assert_eq!(names(&board.kernel.expect("kernel").patches), ["b.patch"]);
}

#[test]
fn inherit_patches_false_replaces_the_familys_list() {
    let board = read(
        &format!("[kernel]\n{SERIES}[bootloader]\n{SERIES}"),
        "[kernel]\ninherit_patches = false\npatches = [{ file = \"b.patch\" }]\n[bootloader]\ninherit_patches = false\n",
    )
    .expect("reads");
    assert_eq!(names(&board.kernel.expect("kernel").patches), ["b.patch"]);
    // With no list of its own, the family's is dropped.
    assert_eq!(board.bootloader.expect("bootloader").patches, None);
    // `true` is the default, said.
    let board = read(
        &format!("[kernel]\n{SERIES}"),
        "[kernel]\ninherit_patches = true\npatches = [{ file = \"b.patch\" }]\n",
    )
    .expect("reads");
    assert_eq!(
        names(&board.kernel.expect("kernel").patches),
        ["series:patches", "f.patch", "b.patch"]
    );
}

#[test]
fn a_family_entry_stated_again_is_refused() {
    let why = read(
        &format!("[kernel]\n{SERIES}"),
        "[kernel]\npatches = [{ file = \"b.patch\" }, { source = \"series\", path = \"patches\" }]\n",
    )
    .expect_err("twice")
    .to_string();
    assert!(why.contains("kernel.patches[1]"), "{why}");
    assert!(why.contains("inherit_patches = false"), "{why}");
    // The family's own file, named from the board's directory.
    let why = read(
        &format!("[bootloader]\n{SERIES}"),
        "[bootloader]\npatches = [{ file = \"../fam/f.patch\" }]\n",
    )
    .expect_err("twice")
    .to_string();
    assert!(why.contains("bootloader.patches[0]"), "{why}");
    // Replacing on purpose, the same entry is the board's to state.
    let board = read(
        &format!("[kernel]\n{SERIES}"),
        "[kernel]\ninherit_patches = false\npatches = [{ source = \"series\", path = \"patches\" }]\n",
    )
    .expect("reads");
    assert_eq!(
        names(&board.kernel.expect("kernel").patches),
        ["series:patches"]
    );
}

#[test]
fn inherit_patches_is_a_boards_key_and_needs_a_family() {
    let why = read(
        "[kernel]\ninherit_patches = false\n",
        "[kernel]\ndevice_tree = \"x/y\"\n",
    )
    .expect_err("a family")
    .to_string();
    assert!(
        why.contains("kernel.inherit_patches") && why.contains("family.toml"),
        "{why}"
    );

    let dir = files(&[(
        "b/board.toml",
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[bootloader]\ninherit_patches = true\n",
    )]);
    let why = Board::read(dir.path().join("b/board.toml"))
        .expect_err("no family")
        .to_string();
    assert!(why.contains("bootloader.inherit_patches"), "{why}");
    assert!(why.contains("extends no family"), "{why}");
}
