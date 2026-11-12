//! A disk from the board's `[disk]` table and partitions plus a recipe's
//! partitions, fanned into `gaia.disk`: planned, built, and read back with
//! sfdisk. The fixture's component is a small Python project, so the build
//! needs Python with pip and setuptools, e2fsprogs and sfdisk; without them
//! the build test says SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/disk")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

fn gaia(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(args)
        .current_dir(dir)
        .env("GAIA_STORE", dir.join("store"))
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("runs");
    println!("$ gaia {}", args.join(" "));
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn found(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|p| Path::new(p).exists())
        .map(|p| p.to_string())
}

/// The tools the build needs, or `None` with a SKIPPED note.
fn tools() -> Option<(String, String, String)> {
    let python = Command::new("/usr/bin/python3")
        .args(["-c", "import pip, setuptools"])
        .status()
        .is_ok_and(|s| s.success())
        .then(|| "/usr/bin/python3".to_string());
    let debugfs = found(&["/usr/sbin/debugfs", "/sbin/debugfs", "/usr/bin/debugfs"]);
    let sfdisk = found(&["/usr/sbin/sfdisk", "/sbin/sfdisk", "/usr/bin/sfdisk"]);
    match (python, debugfs, sfdisk) {
        (Some(p), Some(d), Some(s)) => Some((p, d, s)),
        missing => {
            assert!(
                std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
                "GAIA_REQUIRE_TOOLS is set, and a tool is missing: {missing:?}"
            );
            println!("SKIPPED: needs python3 with pip and setuptools, debugfs and sfdisk");
            None
        }
    }
}

fn plan_args<'a>(python: &'a str, debugfs: &'a str) -> Vec<String> {
    vec![
        "--recipes".into(),
        "recipes".into(),
        "--tool".into(),
        format!("python={python}"),
        "--tool".into(),
        format!("debugfs={debugfs}"),
    ]
}

fn run(dir: &Path, verb: &str, extra: &[String]) -> Output {
    let mut args = vec![verb, "product.toml"];
    args.extend(extra.iter().map(String::as_str));
    gaia(dir, &args)
}

/// The disk's path in the store, from `gaia show`.
fn disk_path(dir: &Path) -> (String, String) {
    let out = gaia(dir, &["show", "img.disk"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf-8");
    let path = text
        .lines()
        .find_map(|l| l.strip_prefix("path").map(|p| p.trim().to_string()))
        .expect("a path line");
    let object = text
        .lines()
        .find_map(|l| l.strip_prefix("object").map(|p| p.trim().to_string()))
        .expect("an object line");
    (path, object)
}

#[test]
fn a_disk_is_the_boards_partitions_then_the_fanned_in_ones() {
    let Some((python, debugfs, sfdisk)) = tools() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture(), dir.path());
    let extra = plan_args(&python, &debugfs);

    let out = run(dir.path(), "plan", &extra);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf-8");
    for want in [
        "img::rootp.partition -> img::disk.partitions0",
        "img::datap.partition -> img::disk.partitions1",
        "host.img.disk-layout -> img::disk.layout",
        "img.disk-layout  gaia:disk-layout  gpt, first 1048576, alignment 1048576, board partitions env",
        "img.disk  gaia:image",
    ] {
        assert!(text.contains(want), "no `{want}` in:\n{text}");
    }

    assert!(run(dir.path(), "build", &extra).status.success());
    let (path, first) = disk_path(dir.path());
    let dump = Command::new(&sfdisk)
        .args(["-d", &path])
        .output()
        .expect("sfdisk");
    let dump = String::from_utf8(dump.stdout).expect("utf-8");
    println!("{dump}");
    let rows: Vec<&str> = dump.lines().filter(|l| l.contains("start=")).collect();
    assert_eq!(rows.len(), 3, "{dump}");
    for (row, (start, size, name)) in rows.iter().zip([
        ("2048", "128", "env"),
        ("4096", "", "rootfs"),
        ("", "", "data"),
    ]) {
        assert!(row.contains(&format!("name=\"{name}\"")), "{row}");
        if !start.is_empty() {
            assert!(row.contains(&format!("start= {start:>11}")), "{row}");
        }
        if !size.is_empty() {
            assert!(row.contains(&format!("size= {size:>11}")), "{row}");
        }
    }
    assert!(rows[1].contains("LegacyBIOSBootable"), "{}", rows[1]);
    assert!(rows[0].contains("3DE21764-95BD-54BD-A5C3-4ABE786F38A8"));

    // The board's layout is fed, and in the disk's key: a bigger `env` is
    // another disk.
    let board = dir.path().join("boards/disk-test/board.toml");
    let text = std::fs::read_to_string(&board).expect("board");
    std::fs::write(&board, text.replace("size = \"64K\"", "size = \"128K\"")).expect("board");
    assert!(run(dir.path(), "build", &extra).status.success());
    let (_, second) = disk_path(dir.path());
    assert_ne!(first, second);
}

#[test]
fn a_disk_recipe_needs_the_boards_disk_table_and_partition_types() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture(), dir.path());
    let board = dir.path().join("boards/disk-test/board.toml");
    let text = std::fs::read_to_string(&board).expect("board");
    let extra = plan_args("/usr/bin/python3", "/usr/sbin/debugfs");

    let without_type = text.replace("type = \"3de21764-95bd-54bd-a5c3-4abe786f38a8\"\n", "");
    std::fs::write(&board, &without_type).expect("board");
    let out = run(dir.path(), "plan", &extra);
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        error.contains("partitions[0].type") && error.contains("needs its type"),
        "{error}"
    );

    let start = text.find("[disk]").expect("[disk]");
    let end = text.find("[[partitions]]").expect("[[partitions]]");
    let without_disk = format!("{}{}", &text[..start], &text[end..]);
    std::fs::write(&board, without_disk).expect("board");
    let out = run(dir.path(), "plan", &extra);
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(error.contains("the board has no [disk] table"), "{error}");
}
