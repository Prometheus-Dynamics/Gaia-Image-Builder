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
  echo "wrote slot $4"
  head -c "$5" /dev/zero > "$6"
  printf 'board %s slot %s kernel %s\n' "$(cat "$2")" "$4" "$(cat "$3/kernel")" | dd of="$6" conv=notrunc 2>/dev/null
  if [ "$7" = "short" ]; then head -c 10 /dev/zero > "$6"; fi ;;
verify)
  head -c 5 "$2" | grep -q "$3" && echo "read back ok" ;;
bundle)
  # bundle OUT PAYLOAD ROOTFS TABLE PRODUCT VERSION [--key FILE --expect ID] [--payload-out DIR]
  out=$2; payload=$3; rootfs=$4; table=$5; product=$6; version=$7; shift 7
  key=; expect=; pout=
  while [ $# -gt 0 ]; do
    case "$1" in
      --key) key=$2; shift 2 ;;
      --expect) expect=$2; shift 2 ;;
      --payload-out) pout=$2; shift 2 ;;
      *) echo "unknown $1" >&2; exit 2 ;;
    esac
  done
  if [ -n "$key" ] && [ "$expect" != "key-$(cat "$key" | wc -c)" ] && [ "$expect" != "any" ]; then
    echo "the key file is not the key the build expects ($expect)" >&2; exit 4
  fi
  {
    printf 'product %s version %s\n' "$product" "$version"
    printf 'kernel %s\n' "$(cat "$payload/kernel")"
    printf 'rootfs %s bytes\n' "$(wc -c < "$rootfs")"
    printf 'table %s\n' "$(cat "$table")"
    if [ -n "$key" ]; then printf 'signed with %s\n' "$(cat "$key")"; else echo unsigned; fi
  } > "$out"
  if [ -n "$pout" ]; then
    mkdir "$pout"
    cp "$payload/kernel" "$pout/kernel"
    printf '%s roothash=abc\n' "$(cat "$payload/cmdline")" > "$pout/cmdline"
  fi ;;
checkbundle)
  # checkbundle OUT [--pubkey FILE]
  grep -q "^product " "$2"
  if [ "$3" = "--pubkey" ]; then grep -q "public" "$4"; fi ;;
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
    for name in [
        "sh", "cat", "head", "wc", "dd", "grep", "basename", "mkdir", "cp",
    ] {
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
        let (image, report, _) = generate_image(
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
            None,
            None,
            None,
            None,
            None,
            None,
            None,
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
    // Every run's standard output, in the order of the runs.
    assert_eq!(
        report,
        "cmdline 5\nkernel 2\nwrote slot base\nread back ok\n"
    );
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
            "names @payloads@; the names are @source@, @description@, @payload@, @slot@, @size@, @out@, @rootfs@, @verity-table@, @key@, @product@, @version@",
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
            "names @size@, and the recipe gives no size",
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

const BUNDLE: &str = r#"{
  "args": ["bundle", "@out@", "@payload@", "@rootfs@", "@verity-table@", "@product@", "@version@", "--payload-out", "@payload-out@"],
  "key_args": ["--key", "@key@", "--expect", "@key-id@"],
  "verify": ["checkbundle", "@out@"],
  "verify_key_args": ["--pubkey", "@pubkey@"]
}"#;

impl Fixture {
    /// A file as the slot's root image and one as its verity table, and
    /// the bundle run on them; `key` is the environment variable that
    /// names a key file, and what the build says identifies the key.
    fn bundle(&self, runs: &str, key: Option<(&str, &str)>) -> Result<String, String> {
        let store = Store::open(&self.store).expect("store");
        let file = |name: &str, bytes: &[u8]| {
            let path = self.root.join(name);
            std::fs::write(&path, bytes).expect("write");
            store.put_file(&path).expect("put")
        };
        let rootfs = gaia_types::Image {
            kind: gaia_types::ImageKind::Filesystem,
            object: file("rootfs.img", &[9u8; 4096]).into(),
            size: gaia_types::Size(4096),
            partitions: Vec::new(),
        };
        let table = gaia_types::Artifact {
            kind: gaia_types::ArtifactKind::parse("verity-table"),
            object: file("table", b"root_hash abc").into(),
        };
        let (env, id) = match key {
            Some((env, id)) => (Some(env.to_string()), Some(id.to_string())),
            None => (None, None),
        };
        // The public half is beside the key, under a second variable.
        let pubkey = key.map(|_| "GAIA_TEST_BUNDLE_PUBKEY".to_string());
        let (image, _, payload) = generate_image(
            &self.store,
            BWRAP,
            self.root(),
            self.tree("tool", &[("bin/img", TOOL, 0o755)]),
            "bin/img",
            self.tree("source", &[("socs/soc", "rk", 0o644)]),
            self.tree("description", &[("board", "zero", 0o644)]),
            "board",
            self.tree(
                "payload-b",
                &[("kernel", "k1", 0o644), ("cmdline", "quiet", 0o644)],
            ),
            runs,
            "base",
            0,
            Some(&rootfs),
            Some(&table),
            env.as_ref(),
            id.as_ref(),
            pubkey.as_ref(),
            Some(&"helios".to_string()),
            Some(&"2026.2.0".to_string()),
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        // The payload as the tool bundled it, when a run asked for it.
        let written = payload.object.path_in(&store).expect("in the store");
        let as_bundled = std::fs::read_to_string(written.join("cmdline")).unwrap_or_default();
        Ok(format!(
            "{}{}",
            String::from_utf8(
                std::fs::read(image.object.path_in(&store).expect("in the store")).expect("read"),
            )
            .expect("text"),
            if as_bundled.is_empty() {
                String::new()
            } else {
                format!("payload-out cmdline {as_bundled}")
            }
        ))
    }
}

/// A file that holds the whole slot: the tool is given the root image, its
/// verity table and the image's product and version beside the payload, and
/// a signing key only when the build has one.
#[test]
fn a_bundle_is_written_from_the_slot_and_signed_when_there_is_a_key() {
    if !available() {
        return;
    }
    // The key is a file the environment names: the test runs again in a
    // process of its own that has the variable.
    if std::env::var_os("GAIA_TEST_BUNDLE_KEY").is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = std::process::Command::new(std::env::current_exe().expect("exe"))
            .args([
                "a_bundle_is_written_from_the_slot_and_signed_when_there_is_a_key",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("GAIA_TEST_BUNDLE_KEY", dir.path().join("dev.key"))
            .env("GAIA_TEST_BUNDLE_PUBKEY", dir.path().join("dev.pub"))
            .status()
            .expect("child");
        assert!(status.success(), "the test failed in its child");
        return;
    }
    let f = Fixture::new();
    let unsigned = f.bundle(BUNDLE, None).expect("unsigned");
    assert_eq!(
        unsigned,
        "product helios version 2026.2.0\nkernel k1\nrootfs 4096 bytes\ntable root_hash abc\nunsigned\npayload-out cmdline quiet roothash=abc\n",
        "the bundle, and the payload as the tool bundled it (the step's third output)"
    );
    // A key file the environment names: bound for the run, its `key_args`
    // added, and the step keyed on the key's stated identity.
    let key = std::path::PathBuf::from(
        std::env::var_os("GAIA_TEST_BUNDLE_KEY").expect("set for the child"),
    );
    std::fs::write(&key, "secret-of-key-one").expect("write");
    std::fs::write(
        std::env::var_os("GAIA_TEST_BUNDLE_PUBKEY").expect("set for the child"),
        "the public half",
    )
    .expect("write");
    let signed = f
        .bundle(BUNDLE, Some(("GAIA_TEST_BUNDLE_KEY", "any")))
        .expect("signed");
    assert!(
        signed.contains("signed with secret-of-key-one\n"),
        "{signed}"
    );
    let made = f.made();
    // The same key again: found. Another key by its identity: made again.
    f.bundle(BUNDLE, Some(("GAIA_TEST_BUNDLE_KEY", "any")))
        .expect("again");
    assert_eq!(f.made(), made);
    std::fs::write(&key, "secret-of-key-two").expect("write");
    let other = f
        .bundle(BUNDLE, Some(("GAIA_TEST_BUNDLE_KEY", "key-17")))
        .expect("another key");
    assert!(other.contains("signed with secret-of-key-two\n"), "{other}");
    // The secret is in no record of the step: not in its logs.
    let store = Store::open(&f.store).expect("store");
    let logs = logs_dir(&store);
    let mut texts = String::new();
    for entry in std::fs::read_dir(logs.join("gaia.generate.image"))
        .expect("logs")
        .flatten()
    {
        texts.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
    }
    assert!(
        !texts.contains("secret-of-key"),
        "the key's bytes are in a log"
    );
    // (The key file's path is in the log, in the sandbox's own command
    // line: where the key is kept is not the key.)
    assert!(texts.contains(&key.display().to_string()));
    // The tool is told which key is meant, and refuses another.
    let why = f
        .bundle(BUNDLE, Some(("GAIA_TEST_BUNDLE_KEY", "key-99")))
        .expect_err("not the key the build expects");
    assert!(why.contains("gaia.generate.image: exited 4"), "{why}");
    // The verify run is given the public half when the build signs; a
    // variable that names no file is refused.
    let no_pub = BUNDLE.replace(
        "\"--pubkey\", \"@pubkey@\"",
        "\"--pubkey\", \"/nonexistent\"",
    );
    let why = f
        .bundle(&no_pub, Some(("GAIA_TEST_BUNDLE_KEY", "any")))
        .expect_err("verify without the public half");
    assert!(why.contains("gaia.generate.image: exited"), "{why}");
    // What is refused: a key with nothing that says which, a variable that
    // names no file, a key the runs cannot be given, and a name the recipe
    // gives nothing for.
    let why = f
        .bundle(BUNDLE, Some(("GAIA_TEST_BUNDLE_KEY", "")))
        .expect_err("no identity");
    assert!(why.contains("nothing says which key it is"), "{why}");
    let why = f
        .bundle(BUNDLE, Some(("GAIA_TEST_NO_SUCH_KEY", "any")))
        .expect_err("no file");
    assert!(
        why.contains("GAIA_TEST_NO_SUCH_KEY is not set to a key file's path"),
        "{why}"
    );
    let no_key_args = BUNDLE.replace(
        "\"key_args\": [\"--key\", \"@key@\", \"--expect\", \"@key-id@\"],",
        "",
    );
    let why = f
        .bundle(&no_key_args, Some(("GAIA_TEST_BUNDLE_KEY", "any")))
        .expect_err("no key_args");
    assert!(why.contains("have no `key_args` to give it by"), "{why}");
    let needs_key = BUNDLE.replace(
        "\"@payload-out@\"]",
        "\"@payload-out@\", \"--key\", \"@key@\"]",
    );
    let why = f.bundle(&needs_key, None).expect_err("@key@ with no key");
    assert!(
        why.contains("names @key@, and the build has no signing key"),
        "{why}"
    );
}
