//! A git source's `subdir`: read, refused on the other kinds, and checked.

use gaia_describe::{Board, Error, Source};

fn files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    dir
}

/// A board with one source, `[sources.x]`, written as `source`.
fn board_with(source: &str) -> Result<Board, Error> {
    let dir = files(&[(
        "b/board.toml",
        &format!(
            "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.x]\n{source}\n"
        ),
    )]);
    Board::read(dir.path().join("b/board.toml"))
}

#[test]
fn a_git_source_reads_its_subdir() {
    let board = board_with(
        "git = \"https://example.org/sdk.git\"\nref = \"7e30\"\nsubdir = \"src/little/uboot\"",
    )
    .expect("reads");
    let Source::Git { subdir, .. } = &board.sources["x"] else {
        panic!("a git source: {:?}", board.sources["x"]);
    };
    assert_eq!(
        subdir.as_ref().map(|s| s.as_str()),
        Some("src/little/uboot")
    );

    // Written with stray separators, it is the same directory.
    let board = board_with("git = \"u\"\nref = \"v\"\nsubdir = \"a//b/\"").expect("reads");
    let Source::Git { subdir, .. } = &board.sources["x"] else {
        panic!("a git source");
    };
    assert_eq!(subdir.as_ref().map(|s| s.as_str()), Some("a/b"));

    // Without one, the root is the tree.
    let board = board_with("git = \"u\"\nref = \"v\"").expect("reads");
    assert!(matches!(
        &board.sources["x"],
        Source::Git { subdir: None, .. }
    ));
}

#[test]
fn a_subdir_on_another_kind_is_refused() {
    for source in [
        "download = \"https://example.org/a.tar\"\nsubdir = \"a\"",
        "image = \"registry.example/x:1\"\nsubdir = \"a\"",
        "path = \"sdk\"\nsubdir = \"a\"",
    ] {
        let error = board_with(source).expect_err("refused");
        println!("{error}");
        assert!(
            error.message.contains("only a git source has a `subdir`"),
            "{error}"
        );
    }
}

#[test]
fn a_subdir_that_leaves_the_checkout_is_refused_at_its_key() {
    for (subdir, says) in [
        ("../x", "has `.` or `..`"),
        ("a/../x", "has `.` or `..`"),
        ("./a", "has `.` or `..`"),
        ("/x", "is absolute"),
        ("", "an empty `subdir`"),
    ] {
        let error = board_with(&format!("git = \"u\"\nref = \"v\"\nsubdir = \"{subdir}\""))
            .expect_err("refused");
        println!("{error}");
        assert_eq!(error.key, "sources.x.subdir", "{error}");
        assert!(error.message.contains(says), "{error}");
    }
}
