//! `gaia.generate.image`: an image written by a tool from a payload, with a
//! shell script as the tool, in the sandbox (its root made here, as for
//! `gaia.generate`). Skipped where no sandbox can run.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::{generate_image, unknown_image_tokens};
use gaia_types::Tree;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// The stand-in tool:
/// `img fit DESCRIPTION PAYLOAD` prints `role bytes` per payload file, and
/// fails when a file is over 16 bytes;
/// `img write DESCRIPTION PAYLOAD SLOT SIZE OUT [short]` writes SIZE bytes:
/// a header line, then zeros;
/// `img verify OUT WORD` fails when the header does not begin with WORD.
const TOOL: &str = r#"#!/bin/sh
set -e
case "$1" in
fit)
  for f in "$3"/*; do
    n=$(wc -c < "$f")
    echo "$(basename "$f") $n"
    if [ "$n" -gt 16 ]; then echo "$(basename "$f") is over its bay" >&2; exit 3; fi
  done ;;
write)
  head -c "$5" /dev/zero > "$6"
  printf 'board %s slot %s kernel %s\n' "$(cat "$2")" "$4" "$(cat "$3/kernel")" | dd of="$6" conv=notrunc 2>/dev/null
  if [ "$7" = "short" ]; then head -c 10 /dev/zero > "$6"; fi ;;
verify)
  head -c 5 "$2" | grep -q "$3" ;;
esac
"#;

const BWRAP: &str = "/usr/bin/bwrap";

/// Whether a sandbox can run here.
fn available() -> bool {
    let ok = std::path::Path::new(BWRAP).exists()
        && std::process::Command::new(BWRAP)
            .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_SANDBOX").is_none(),
            "GAIA_REQUIRE_SANDBOX is set, and no sandbox runs here"
        );
        println!("SKIPPED: no bubblewrap with user namespaces here");
    }
    ok
}

/// A root filesystem in `dir` with `/bin/sh`, the programs the stand-in
/// tool runs, and every library they load, each at the path it has here.
fn make_root(dir: &std::path::Path) {
    let place = |from: &std::path::Path, at: &std::path::Path| {
        let to = dir.join(at.strip_prefix("/").expect("absolute"));
        if !to.exists() {
            std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
            std::fs::copy(from, &to).expect("copy");
        }
    };
    for name in ["sh", "cat", "head", "wc", "dd", "grep", "basename"] {
        let program = std::fs::canonicalize(format!("/usr/bin/{name}")).expect("a program");
        place(&program, std::path::Path::new(&format!("/bin/{name}")));
        let listed = std::process::Command::new("ldd")
            .arg(&program)
            .output()
            .expect("ldd");
        for line in String::from_utf8_lossy(&listed.stdout).lines() {
            // `name => /path (addr)` or `/path (addr)`.
            for word in line.split_whitespace().filter(|word| word.starts_with('/')) {
                let library = std::path::Path::new(word);
                let real = std::fs::canonicalize(library).expect("a library");
                place(&real, library);
            }
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store").display().to_string();
        Self {
            _dir: dir,
            root,
            store,
        }
    }

    fn tree(&self, name: &str, files: &[(&str, &str, u32)]) -> Tree {
        let dir = self.root.join(name);
        for (path, text, mode) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, text).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).expect("chmod");
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    fn root(&self) -> Tree {
        let dir = self.root.join("rootfs");
        if !dir.exists() {
            make_root(&dir);
            // The tool reads zeros from the sandbox's /dev.
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    /// The image's bytes and the report, for a payload whose kernel is
    /// `kernel`.
    fn run(
        &self,
        kernel: &str,
        runs: &str,
        slot: &str,
        size: i64,
    ) -> Result<(Vec<u8>, String), String> {
        let (image, report) = generate_image(
            &self.store,
            BWRAP,
            self.root(),
            self.tree("tool", &[("bin/img", TOOL, 0o755)]),
            "bin/img",
            self.tree("source", &[("socs/soc", "rk", 0o644)]),
            self.tree("description", &[("board", "zero", 0o644)]),
            "board",
            self.tree(
                &format!("payload-{kernel}"),
                &[("kernel", kernel, 0o644), ("cmdline", "quiet", 0o644)],
            ),
            runs,
            slot,
            size,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        let read = |object: &gaia_types::StoreRef| {
            std::fs::read(object.path_in(&store).expect("in the store")).expect("read")
        };
        Ok((
            read(&image.object),
            String::from_utf8(read(&report.object)).expect("text"),
        ))
    }

    fn made(&self) -> usize {
        let store = Store::open(&self.store).expect("store");
        step_records(&logs_dir(&store))
            .into_iter()
            .filter(|r| r.step == "gaia.generate.image" && r.made)
            .count()
    }
}

const RUNS: &str = r#"{
  "fit": ["fit", "@description@", "@payload@"],
  "args": ["write", "@description@", "@payload@", "@slot@", "@size@", "@out@"],
  "verify": ["verify", "@out@", "board"]
}"#;

#[test]
fn the_tool_writes_the_image_from_the_payload_and_says_what_fits() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let (image, report) = f.run("k1", RUNS, "base", 4096).expect("made");
    assert_eq!(image.len(), 4096);
    assert!(image.starts_with(b"board zero slot base kernel k1\n"));
    assert!(image[40..].iter().all(|b| *b == 0));
    // What the fit run printed: each payload file and its stored size.
    assert_eq!(report, "cmdline 5\nkernel 2\n");
    // The same again is found; another payload, slot or size is made.
    f.run("k1", RUNS, "base", 4096).expect("again");
    assert_eq!(f.made(), 1);
    f.run("k2", RUNS, "base", 4096).expect("another payload");
    f.run("k1", RUNS, "trial", 4096).expect("another slot");
    let (bigger, _) = f.run("k1", RUNS, "base", 8192).expect("another size");
    assert_eq!(bigger.len(), 8192);
    assert_eq!(f.made(), 4);
}

#[test]
fn a_payload_that_does_not_fit_a_wrong_size_and_a_failed_verify_fail() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    // The fit run fails before the image is written.
    let why = f
        .run("a-kernel-over-sixteen-bytes", RUNS, "base", 4096)
        .expect_err("over its bay");
    println!("{why}");
    assert!(why.contains("gaia.generate.image: exited 3"), "{why}");
    // The tool wrote another size than the image's.
    let short = RUNS.replace("\"@out@\"]", "\"@out@\", \"short\"]");
    let why = f.run("k1", &short, "base", 4096).expect_err("ten bytes");
    assert!(
        why.contains("bin/img wrote 10 bytes at @out@; the image's size is 4096"),
        "{why}"
    );
    // What it wrote does not verify.
    let other = RUNS.replace(
        "\"verify\", \"@out@\", \"board\"",
        "\"verify\", \"@out@\", \"other\"",
    );
    let why = f.run("k1", &other, "base", 4096).expect_err("verify");
    assert!(why.contains("gaia.generate.image: exited 1"), "{why}");
}

#[test]
fn what_cannot_be_run_is_refused_before_anything_runs() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    for (runs, slot, size, message) in [
        (
            RUNS.replace("@payload@", "@payloads@"),
            "base",
            4096,
            "names @payloads@; the names are @source@, @description@, @payload@, @slot@, @size@, @out@",
        ),
        (
            RUNS.to_string(),
            "",
            4096,
            "names @slot@, and the image has no slot id",
        ),
        (
            RUNS.to_string(),
            "base",
            0,
            "size 0: an image has a size above 0",
        ),
        (
            RUNS.replace("\"@size@\", \"@out@\"", "\"@size@\""),
            "base",
            4096,
            "no argument of `args` names @out@",
        ),
        (
            r#"{"fit": [], "args": [], "extra": 1}"#.to_string(),
            "base",
            4096,
            "the runs are not `fit`, `args` and `verify` lists",
        ),
    ] {
        let why = f.run("k1", &runs, slot, size).expect_err(message);
        assert!(why.contains(message), "{why}");
    }
    assert_eq!(f.made(), 0);
    assert_eq!(
        unknown_image_tokens("@payload@/x @out@"),
        Vec::<String>::new()
    );
    assert_eq!(
        unknown_image_tokens("@payloads@ user@host a@b.c"),
        ["@payloads@"]
    );
}
