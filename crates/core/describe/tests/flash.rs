//! The `[flash]` keys `load_address` and `medium`, and how a board merges them.

use gaia_describe::{Board, Error, Medium};

fn files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    dir
}

fn read_board(flash: &str) -> Result<Board, Error> {
    let dir = files(&[(
        "b/board.toml",
        &format!(
            "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.s]\ngit = \"u\"\nref = \"v\"\n{flash}"
        ),
    )]);
    Board::read(dir.path().join("b/board.toml"))
}

#[test]
fn both_keys_read() {
    let board = read_board(
        "[flash]\nmethods = [\"usb-loader\"]\nload_address = 0x80360000\nmedium = \"sd\"\n",
    )
    .expect("reads");
    let flash = board.flash.expect("flash");
    assert_eq!(flash.load_address, Some(0x8036_0000));
    assert_eq!(flash.medium, Some(Medium::Sd));
    assert_eq!(
        flash.methods.as_deref(),
        Some(&["usb-loader".to_string()][..])
    );

    let flash = read_board("[flash]\nmedium = \"spi-nand\"\n")
        .expect("reads")
        .flash
        .expect("flash");
    assert_eq!(flash.medium, Some(Medium::SpiNand));
    assert_eq!(flash.load_address, None);

    // They are data for the flashing tool, so they serialize with the board.
    let flash = read_board("[flash]\nload_address = 1\nmedium = \"spi-nor\"\n")
        .expect("reads")
        .flash
        .expect("flash");
    let text = toml::to_string(&flash).expect("serializes");
    assert!(text.contains("load_address = 1"), "{text}");
    assert!(text.contains("medium = \"spi-nor\""), "{text}");
}

#[test]
fn a_negative_load_address_is_refused_at_its_key() {
    let error = read_board("[flash]\nload_address = -1\n").expect_err("refused");
    println!("{error}");
    assert_eq!(error.key, "flash.load_address", "{error}");
    assert!(error.message.contains("negative"), "{error}");
}

#[test]
fn a_bad_medium_is_refused_at_its_key_with_the_four() {
    let error = read_board("[flash]\nmedium = \"usb\"\n").expect_err("refused");
    println!("{error}");
    assert_eq!(error.key, "flash.medium", "{error}");
    for name in ["sd", "emmc", "spi-nand", "spi-nor"] {
        assert!(error.message.contains(&format!("`{name}`")), "{error}");
    }
}

#[test]
fn a_board_merges_flash_key_by_key_over_its_family() {
    let family = "version = 1\n[target]\narch = \"aarch64\"\n[sources.s]\ngit = \"u\"\nref = \"v\"\n[flash]\nmethods = [\"sd-card\"]\nmedium = \"emmc\"\n[flash.files]\nloader = { source = \"s\", path = \"loader.bin\" }\n";
    let dir = files(&[
        ("family.toml", family),
        (
            "b/board.toml",
            "version = 1\nname = \"b\"\nextends = \"../family.toml\"\n[flash]\nmedium = \"sd\"\nload_address = 4096\n",
        ),
    ]);
    let board = Board::read(dir.path().join("b/board.toml")).expect("reads");
    assert_eq!(
        board.family.as_deref(),
        Some(dir.path().join("family.toml").as_path())
    );
    let flash = board.flash.expect("flash");
    // The board's keys win; the family's others are kept.
    assert_eq!(flash.medium, Some(Medium::Sd));
    assert_eq!(flash.load_address, Some(4096));
    assert_eq!(flash.methods.as_deref(), Some(&["sd-card".to_string()][..]));
    assert!(
        flash.files.contains_key("loader"),
        "the family's files are kept"
    );
}
