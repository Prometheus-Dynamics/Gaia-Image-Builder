//! A file's executable bit survives every way into the store these steps
//! have, with the step's work directory in RAM and on the store's disk: a
//! script a build runs from a stored tree must still be runnable there.
//!
//! The checks run in a child process for each place, because the work
//! directory's place is the process's environment.

use daedalus_cpu_author::FanIn;
use gaia_source_files::{source_archive, source_input, source_patch, source_path};
use gaia_step::{Ctx, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const PATCH: &str = "/usr/bin/patch";

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

fn file(path: &Path, text: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn dir_of(store: &Path, tree: &Tree) -> PathBuf {
    Store::open(store)
        .expect("store")
        .path(&tree.object.object_ref().expect("stored"))
}

/// Each step, from a script (0755 or 0750) and a plain file, into a new
/// store under `root`; the modes of both in the stored tree.
fn check(root: &Path, place: &str) {
    let store = root.join("store");
    let store_str = store.display().to_string();
    let cx = Ctx::new();

    // gaia.source.input: one file, named. Executable by its owner alone is
    // executable; the rest of its mode on this machine is not kept.
    let script = root.join("in/post-build.sh");
    file(&script, "#!/bin/sh\nexit 0\n", 0o750);
    let tree = source_input(
        &store_str,
        &script.display().to_string(),
        "post-build.sh",
        &cx,
    )
    .expect("input");
    assert_eq!(
        mode(&dir_of(&store, &tree).join("post-build.sh")),
        0o755,
        "{place}: an executable input"
    );
    let plain = root.join("in/notes.txt");
    file(&plain, "notes\n", 0o600);
    let tree =
        source_input(&store_str, &plain.display().to_string(), "notes.txt", &cx).expect("input");
    assert_eq!(
        mode(&dir_of(&store, &tree).join("notes.txt")),
        0o644,
        "{place}: a plain input"
    );
    // The same bytes, once executable and once not, are two trees.
    let twin = root.join("in/twin");
    file(&twin, "#!/bin/sh\nexit 0\n", 0o644);
    let tree = source_input(
        &store_str,
        &twin.display().to_string(),
        "post-build.sh",
        &cx,
    )
    .expect("input");
    assert_eq!(
        mode(&dir_of(&store, &tree).join("post-build.sh")),
        0o644,
        "{place}: the same bytes, not executable"
    );

    // gaia.source.path: a directory, as it is.
    let src = root.join("dir");
    file(&src.join("bin/run"), "#!/bin/sh\n", 0o755);
    file(&src.join("a.txt"), "a\n", 0o644);
    let tree = source_path(&store_str, &src.display().to_string(), &cx).expect("path");
    let stored = dir_of(&store, &tree);
    assert_eq!(mode(&stored.join("bin/run")), 0o755, "{place}: path");
    assert_eq!(mode(&stored.join("a.txt")), 0o644, "{place}: path");

    // gaia.source.archive: the members' executable bits.
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data, member_mode) in [
        ("pkg/run", &b"#!/bin/sh\n"[..], 0o755),
        ("pkg/a.txt", &b"a\n"[..], 0o644),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(member_mode);
        header.set_mtime(1_000_000);
        header.set_size(data.len() as u64);
        builder
            .append_data(&mut header, path, data)
            .expect("append");
    }
    let upload = root.join("upload.tar");
    std::fs::write(&upload, builder.into_inner().expect("tar")).expect("write");
    let archive = Artifact {
        kind: ArtifactKind::Archive,
        object: Store::open(&store)
            .expect("store")
            .put_file(&upload)
            .expect("put")
            .into(),
    };
    let tree = source_archive(&store_str, &archive, 1, &cx).expect("archive");
    let stored = dir_of(&store, &tree);
    assert_eq!(mode(&stored.join("run")), 0o755, "{place}: archive");
    assert_eq!(mode(&stored.join("a.txt")), 0o644, "{place}: archive");

    // gaia.source.patch: a script the patch changes, and one it leaves.
    if !Path::new(PATCH).exists() {
        println!("SKIPPED the patch step: no {PATCH}");
        return;
    }
    let put = |name: &str, files: &[(&str, &str, u32)]| {
        let dir = root.join(name);
        for (path, text, file_mode) in files {
            file(&dir.join(path), text, *file_mode);
        }
        Tree {
            object: Store::open(&store)
                .expect("store")
                .put_tree(&dir)
                .expect("put")
                .into(),
            entries: Vec::new(),
        }
    };
    let source = put(
        "psrc",
        &[
            ("configure", "#!/bin/sh\necho one\n", 0o755),
            ("tools/gen", "#!/bin/sh\n", 0o755),
            ("a.txt", "a\n", 0o644),
        ],
    );
    let patches = put(
        "ppatch",
        &[(
            "0001.patch",
            "--- a/configure\n+++ b/configure\n@@ -1,2 +1,2 @@\n #!/bin/sh\n-echo one\n+echo two\n",
            0o644,
        )],
    );
    let trees = [Some(patches)];
    let tree = source_patch(
        &store_str,
        PATCH,
        source,
        FanIn::values(&trees),
        "[\"0001.patch\"]",
        &cx,
    )
    .expect("patch");
    let stored = dir_of(&store, &tree);
    assert_eq!(
        std::fs::read_to_string(stored.join("configure")).expect("read"),
        "#!/bin/sh\necho two\n"
    );
    assert_eq!(mode(&stored.join("configure")), 0o755, "{place}: patched");
    assert_eq!(mode(&stored.join("tools/gen")), 0o755, "{place}: untouched");
    assert_eq!(mode(&stored.join("a.txt")), 0o644, "{place}: patch");
}

/// The environment variable that tells this test binary it is the child
/// of the test below, and where the work directory is to be.
const CHILD: &str = "GAIA_TEST_MODES_PLACE";

/// Runs [`check`] in a process of its own for each place: the place of a
/// work directory is read from the environment, and a test does not change
/// its own.
#[test]
fn an_executable_file_is_executable_in_the_store_from_ram_and_from_disk() {
    if let Ok(place) = std::env::var(CHILD) {
        let root = tempfile::tempdir().expect("tempdir");
        check(root.path(), &place);
        return;
    }
    for place in ["ram", "disk"] {
        let out = std::process::Command::new(std::env::current_exe().expect("this test"))
            .args([
                "--exact",
                "an_executable_file_is_executable_in_the_store_from_ram_and_from_disk",
                "--nocapture",
            ])
            .env(CHILD, place)
            .env("GAIA_WORK_DIR", place)
            .output()
            .expect("runs");
        assert!(
            out.status.success(),
            "work directory {place}:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
