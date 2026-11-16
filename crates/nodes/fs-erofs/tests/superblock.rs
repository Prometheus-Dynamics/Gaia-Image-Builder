//! The superblock check against images `mkfs.erofs` makes on this machine.
//! Without erofs-utils the tests say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when they fail.

use gaia_fs_erofs::superblock;
use std::path::Path;
use std::process::Command;

/// The erofs-utils program `name`, in the usual places, if it is there.
fn tool(name: &str) -> Option<String> {
    ["/usr/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| Path::new(p).is_file())
}

/// A tree with a tail that packs (text) and one block-aligned file.
fn tree(dir: &Path) {
    std::fs::create_dir_all(dir.join("d")).expect("mkdir");
    std::fs::write(dir.join("text"), "hello erofs superblock\n".repeat(400)).expect("write");
    std::fs::write(dir.join("d/aligned"), vec![7u8; 8192]).expect("write");
}

/// The superblock of an image made with `args` on `tree`.
fn made(mkfs: &str, dir: &Path, name: &str, args: &[&str]) -> superblock::Superblock {
    let src = dir.join("tree");
    if !src.exists() {
        tree(&src);
    }
    let image = dir.join(format!("{name}.erofs"));
    let out = Command::new(mkfs)
        .arg("--quiet")
        .args(args)
        .arg(&image)
        .arg(&src)
        .output()
        .expect("mkfs.erofs runs");
    assert!(
        out.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&image).expect("read");
    superblock::read(&bytes).expect("an erofs superblock")
}

macro_rules! skip_without_tool {
    ($mkfs:ident) => {
        let Some($mkfs) = tool("mkfs.erofs") else {
            assert!(
                std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
                "erofs-utils missing"
            );
            println!("SKIPPED: no mkfs.erofs on this machine");
            return;
        };
    };
}

/// Each compression the machine's tool makes is what the superblock says.
#[test]
fn each_compression_is_read_back_as_made() {
    skip_without_tool!(mkfs);
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, args, want) in [
        ("none", vec![], "none"),
        ("lz4", vec!["-zlz4"], "lz4"),
        ("lz4hc", vec!["-zlz4hc"], "lz4hc"),
        ("lzma", vec!["-zlzma"], "lzma"),
        ("deflate", vec!["-zdeflate"], "deflate"),
        ("zstd", vec!["-zzstd"], "zstd"),
    ] {
        let sb = made(&mkfs, dir.path(), name, &args);
        let line = superblock::check(&sb, want, &[]).unwrap_or_else(|e| panic!("{name}: {e}"));
        // lz4 and lz4hc are one family in the superblock.
        let family = if want == "lz4hc" { "lz4" } else { want };
        assert!(line.contains(&format!("compression {family}")), "{line}");
    }
}

/// An image made with an option is accepted for it, and refused for any other
/// compression.
#[test]
fn a_made_compressor_is_refused_for_another() {
    skip_without_tool!(mkfs);
    let dir = tempfile::tempdir().expect("tempdir");
    let lzma = made(&mkfs, dir.path(), "lzma", &["-zlzma"]);
    assert!(superblock::check(&lzma, "zstd", &[]).is_err());
    assert!(superblock::check(&lzma, "none", &[]).is_err());
    let none = made(&mkfs, dir.path(), "none2", &[]);
    assert!(superblock::check(&none, "lz4", &[]).is_err());
}

/// Fragments show as the packed inode, and are refused when not asked for.
#[test]
fn fragments_are_read_back_as_asked() {
    skip_without_tool!(mkfs);
    let dir = tempfile::tempdir().expect("tempdir");
    let frag = made(&mkfs, dir.path(), "frag", &["-zlz4hc", "-Efragments"]);
    assert!(superblock::check(&frag, "lz4hc", &["fragments"]).is_ok());
    assert!(superblock::check(&frag, "lz4hc", &[]).is_err());
    let plain = made(&mkfs, dir.path(), "plain", &["-zlz4hc"]);
    assert!(superblock::check(&plain, "lz4hc", &[]).is_ok());
    assert!(superblock::check(&plain, "lz4hc", &["fragments"]).is_err());
}

/// Ztailpacking is set on this tree (its text tail packs), and not asked for
/// it is refused.
#[test]
fn ztailpacking_set_by_a_packed_tail_only_when_asked() {
    skip_without_tool!(mkfs);
    let dir = tempfile::tempdir().expect("tempdir");
    let ztail = made(&mkfs, dir.path(), "ztail", &["-zlz4hc", "-Eztailpacking"]);
    let line = superblock::check(&ztail, "lz4hc", &["ztailpacking"]).expect("asked");
    assert!(line.ends_with("ztailpacking yes"), "{line}");
    assert!(superblock::check(&ztail, "lz4hc", &[]).is_err());
}
