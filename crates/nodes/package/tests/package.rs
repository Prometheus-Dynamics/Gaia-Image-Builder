//! Packages for other installers. The `.ipk` and `.deb` unpack with standard
//! tools (`ar`, `tar`, `dpkg-deb`) to the tree and its control; the OCI
//! archive is a valid layout, and `podman` loads it and runs its entrypoint.
//! A test whose tool is missing says SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when it fails.

use gaia_package::{Control, deb, ipk, oci};
use gaia_step::Ctx;
use gaia_store::{Digest, Store};
use gaia_types::{Artifact, FileEntry, Tree};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A program's path, in the usual places, or None.
fn tool(name: &str) -> Option<String> {
    ["/usr/bin", "/usr/sbin", "/sbin", "/usr/local/bin"]
        .iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| Path::new(p).exists())
}

/// The paths of `names`, or None (SKIPPED, or a failure with GAIA_REQUIRE_TOOLS).
fn tools(names: &[&str]) -> Option<Vec<String>> {
    let found: Option<Vec<String>> = names.iter().map(|n| tool(n)).collect();
    if found.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and one of {names:?} is missing"
        );
        println!("SKIPPED: one of {names:?} is not installed");
    }
    found
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// A small tree: a root-owned tool, a service's private state, a symlink, a
/// text file big enough to compress.
fn sample(root: &Path) {
    write(root, "etc/motd", "hello\n");
    write(root, "usr/bin/tool", "#!/bin/sh\necho tool\n");
    write(root, "var/lib/app/state", "0\n");
    let text: String = (0..4_000)
        .map(|i| format!("line {i} of a compressible file\n"))
        .collect();
    write(root, "usr/share/doc/big.txt", &text);
    let _ = std::fs::remove_file(root.join("bin"));
    std::os::unix::fs::symlink("usr/bin", root.join("bin")).expect("symlink");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        root.join("usr/bin/tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
}

fn entries() -> Vec<FileEntry> {
    let e = |path: &str, mode, uid, gid| FileEntry {
        path: path.into(),
        mode,
        uid,
        gid,
    };
    vec![
        e("usr/bin/tool", 0o755, 0, 0),
        e("var/lib/app", 0o750, 1000, 1000),
        e("var/lib/app/state", 0o600, 1000, 1000),
    ]
}

/// The tree of `src`, put into a store under `root`.
fn tree(root: &Path, src: &Path) -> (Store, Tree, String) {
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(src).expect("put").into(),
        entries: entries(),
    };
    (store, tree, store_path.display().to_string())
}

fn control() -> Control {
    Control {
        package: "gaia-sample".into(),
        version: "1.2.3-1".into(),
        architecture: "amd64".into(),
        maintainer: "Gaia Test <gaia-test@example.invalid>".into(),
        description: "Sample tree\nA package for the tests.\n\nSecond paragraph.".into(),
        depends: vec!["libc6 (>= 2.31)".into()],
    }
}

/// Builds a package of the sample tree in its own store; returns its file.
fn package(
    base: &Path,
    build: fn(&str, Tree, Control, &Ctx) -> Result<Artifact, gaia_step::StepError>,
    ctl: &Control,
) -> (PathBuf, String) {
    let src = base.join("src");
    sample(&src);
    let (store, tree, root) = tree(base, &src);
    let artifact = build(&root, tree, ctl.clone(), &Ctx::new()).expect("builds");
    let path = artifact.object.path_in(&store).expect("in store");
    (path, root)
}

/// The regular files of `src` as a package carries them: the stored modes
/// the entries give, else the file's own. Path to (mode, bytes).
fn expected_of(src: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
    let mut want = files_of(src);
    for entry in entries() {
        if let Some(file) = want.get_mut(&entry.path) {
            file.0 = entry.mode;
        }
    }
    want
}

/// The regular files of `root`, path to (mode, bytes).
fn files_of(root: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, (u32, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("read") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let meta = std::fs::symlink_metadata(entry.path()).expect("meta");
            if meta.is_dir() {
                walk(&entry.path(), &path, out);
            } else if meta.is_file() {
                out.insert(
                    path,
                    (
                        meta.mode() & 0o7777,
                        std::fs::read(entry.path()).expect("read"),
                    ),
                );
            }
        }
    }
    walk(root, "", &mut out);
    out
}

/// The installed size the control text carries: the regular files' bytes.
fn installed(root: &Path) -> u64 {
    files_of(root).values().map(|(_, b)| b.len() as u64).sum()
}

#[test]
fn the_same_tree_and_control_give_the_same_packages() {
    let ctl = control();
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (first, _) = package(a.path(), ipk, &ctl);
    let (second, _) = package(b.path(), ipk, &ctl);
    let digest = gaia_store::file_digest(&first).expect("digest");
    assert_eq!(digest, gaia_store::file_digest(&second).expect("digest"));
    let (deb_a, _) = package(a.path(), deb, &ctl);
    let (deb_b, _) = package(b.path(), deb, &ctl);
    assert_eq!(
        gaia_store::file_digest(&deb_a).expect("digest"),
        gaia_store::file_digest(&deb_b).expect("digest")
    );
    assert_ne!(digest, gaia_store::file_digest(&deb_a).expect("digest"));
}

#[test]
fn a_control_that_cannot_make_a_package_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let (_, tree, root) = tree(dir.path(), &src);
    let mut bad = control();
    bad.package = "Bad Name".into();
    let error = ipk(&root, tree.clone(), bad.clone(), &Ctx::new()).expect_err("name");
    assert!(format!("{error:?}").contains("package name"), "{error:?}");
    bad = control();
    bad.description = "  ".into();
    let error = deb(&root, tree, bad, &Ctx::new()).expect_err("description");
    assert!(format!("{error:?}").contains("description"), "{error:?}");
}

#[test]
fn an_ipk_unpacks_with_ar_and_tar() {
    let Some(tools) = tools(&["ar", "tar"]) else {
        return;
    };
    let (ar, tar) = (&tools[0], &tools[1]);
    let dir = tempfile::tempdir().expect("tempdir");
    let ctl = control();
    let (pkg, _root) = package(dir.path(), ipk, &ctl);
    let out = dir.path().join("unpacked");
    std::fs::create_dir_all(&out).expect("mkdir");

    let listing = Command::new(ar).arg("t").arg(&pkg).output().expect("ar t");
    assert!(listing.status.success());
    let members: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .collect();
    assert_eq!(members, ["debian-binary", "control.tar.gz", "data.tar.gz"]);
    assert!(
        Command::new(ar)
            .arg("x")
            .arg(&pkg)
            .current_dir(&out)
            .status()
            .expect("ar x")
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(out.join("debian-binary")).expect("binary"),
        "2.0\n"
    );

    let data = out.join("data");
    std::fs::create_dir_all(&data).expect("mkdir");
    assert!(
        Command::new(tar)
            .args(["-xzf"])
            .arg(out.join("data.tar.gz"))
            .arg("-C")
            .arg(&data)
            .status()
            .expect("tar")
            .success()
    );
    let want = expected_of(&dir.path().join("src"));
    let got = files_of(&data);
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>()
    );
    for (path, (mode, bytes)) in &want {
        assert_eq!(&got[path].1, bytes, "{path}");
        assert_eq!(got[path].0, *mode, "{path} mode");
    }

    // Owners as the entries give them, read from the headers.
    let mut owners = BTreeMap::new();
    let gz =
        flate2::read::GzDecoder::new(std::fs::File::open(out.join("data.tar.gz")).expect("open"));
    for entry in tar::Archive::new(gz).entries().expect("entries") {
        let entry = entry.expect("entry");
        let name = entry.path().expect("path").to_string_lossy().into_owned();
        let name = name.trim_start_matches("./").to_string();
        owners.insert(
            name,
            (
                entry.header().uid().expect("uid"),
                entry.header().gid().expect("gid"),
            ),
        );
    }
    assert_eq!(owners["var/lib/app/state"], (1000, 1000));
    assert_eq!(owners["usr/bin/tool"], (0, 0));

    let control_text = Command::new(tar)
        .args(["-xzOf"])
        .arg(out.join("control.tar.gz"))
        .arg("./control")
        .output()
        .expect("tar control");
    assert!(control_text.status.success());
    assert_eq!(
        String::from_utf8_lossy(&control_text.stdout),
        ctl.text(installed(&dir.path().join("src")))
    );
}

#[test]
fn a_deb_unpacks_with_dpkg_deb() {
    let Some(tools) = tools(&["dpkg-deb"]) else {
        return;
    };
    let dpkg = &tools[0];
    let dir = tempfile::tempdir().expect("tempdir");
    let ctl = control();
    let (pkg, _root) = package(dir.path(), deb, &ctl);
    let src = dir.path().join("src");

    let info = Command::new(dpkg)
        .arg("--info")
        .arg(&pkg)
        .output()
        .expect("info");
    assert!(
        info.status.success(),
        "{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let field = |name: &str| {
        let out = Command::new(dpkg)
            .arg("--field")
            .arg(&pkg)
            .arg(name)
            .output()
            .expect("field");
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    };
    assert_eq!(field("Package"), "gaia-sample");
    assert_eq!(field("Version"), "1.2.3-1");
    assert_eq!(field("Architecture"), "amd64");
    assert_eq!(field("Depends"), "libc6 (>= 2.31)");
    assert_eq!(
        field("Installed-Size"),
        installed(&src).div_ceil(1024).to_string()
    );

    let control_dir = dir.path().join("control");
    assert!(
        Command::new(dpkg)
            .arg("--control")
            .arg(&pkg)
            .arg(&control_dir)
            .status()
            .expect("control")
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(control_dir.join("control")).expect("control"),
        ctl.text(installed(&src))
    );

    let out = dir.path().join("extracted");
    assert!(
        Command::new(dpkg)
            .arg("--extract")
            .arg(&pkg)
            .arg(&out)
            .status()
            .expect("extract")
            .success()
    );
    let want = expected_of(&src);
    let got = files_of(&out);
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>()
    );
    for (path, (mode, bytes)) in &want {
        assert_eq!(&got[path].1, bytes, "{path}");
        assert_eq!(got[path].0, *mode, "{path} mode");
    }
}

/// The OCI archive's entries by name, read with the tar crate.
fn archive_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut archive = tar::Archive::new(std::fs::File::open(path).expect("open"));
    for entry in archive.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        let name = entry.path().expect("path").to_string_lossy().into_owned();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).expect("read");
        out.insert(name, bytes);
    }
    out
}

#[test]
fn the_oci_archive_is_a_valid_layout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let (store, tree, root) = tree(dir.path(), &src);
    let artifact = oci(
        &root,
        tree,
        "localhost/gaia-test:1",
        "amd64",
        vec!["/usr/bin/tool".into()],
        &Ctx::new(),
    )
    .expect("builds");
    let entries = archive_entries(&artifact.object.path_in(&store).expect("in store"));
    let text = |name: &str| String::from_utf8(entries[name].clone()).expect("text");
    assert_eq!(text("oci-layout"), "{\"imageLayoutVersion\":\"1.0.0\"}\n");

    let index: serde_json::Value = serde_json::from_str(&text("index.json")).expect("index");
    let manifest_ref = &index["manifests"][0];
    assert_eq!(
        manifest_ref["annotations"]["org.opencontainers.image.ref.name"],
        "localhost/gaia-test:1"
    );
    // Every blob is named by its digest, and that digest is its bytes'.
    for (name, bytes) in &entries {
        if let Some(hex) = name.strip_prefix("blobs/sha256/") {
            assert_eq!(Digest::of_bytes(bytes).hex(), hex, "{name}");
        }
    }
    let manifest_hex = manifest_ref["digest"]
        .as_str()
        .expect("digest")
        .trim_start_matches("sha256:");
    let manifest: serde_json::Value =
        serde_json::from_slice(&entries[&format!("blobs/sha256/{manifest_hex}")])
            .expect("manifest");
    let config: serde_json::Value = serde_json::from_slice(
        &entries[&format!(
            "blobs/sha256/{}",
            manifest["config"]["digest"]
                .as_str()
                .expect("config digest")
                .trim_start_matches("sha256:")
        )],
    )
    .expect("config");
    assert_eq!(config["architecture"], "amd64");
    assert_eq!(config["os"], "linux");
    assert_eq!(config["config"]["Entrypoint"][0], "/usr/bin/tool");
    let layer_digest = manifest["layers"][0]["digest"].as_str().expect("layer");
    assert_eq!(config["rootfs"]["diff_ids"][0], layer_digest);

    // The layer is the tree as tar: its files, with the stored modes.
    let layer_path = dir.path().join("layer.tar");
    std::fs::write(
        &layer_path,
        &entries[&format!(
            "blobs/sha256/{}",
            layer_digest.trim_start_matches("sha256:")
        )],
    )
    .expect("write layer");
    let mut layer = tar::Archive::new(std::fs::File::open(&layer_path).expect("layer"));
    let names: Vec<String> = layer
        .entries()
        .expect("entries")
        .map(|e| {
            e.expect("entry")
                .path()
                .expect("path")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(names.contains(&"usr/bin/tool".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("./")), "{names:?}");
}

/// Copies `program` and the libraries `ldd` names for it into `root`, at
/// the same paths, so the program runs in an image of its own.
fn copy_with_libraries(program: &str, root: &Path, ldd: &str) {
    let mut files = vec![program.to_string()];
    let out = Command::new(ldd).arg(program).output().expect("ldd");
    for token in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        if token.starts_with('/') {
            files.push(token.to_string());
        }
    }
    for file in files {
        let dest = root.join(file.trim_start_matches('/'));
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        std::fs::copy(&file, &dest).expect("copy");
    }
}

/// Removes the image when the test ends.
struct Image<'a> {
    podman: &'a str,
    reference: String,
}

impl Drop for Image<'_> {
    fn drop(&mut self) {
        let _ = Command::new(self.podman)
            .args(["rmi", "-f", &self.reference])
            .output();
    }
}

#[test]
fn podman_loads_the_oci_archive_and_runs_its_entrypoint() {
    let Some(tools) = tools(&["podman", "ldd"]) else {
        return;
    };
    let (podman, ldd) = (&tools[0], &tools[1]);
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    copy_with_libraries("/usr/bin/true", &src, ldd);
    let (store, tree, root) = tree(dir.path(), &src);
    let reference = format!("localhost/gaia-package-test:{}", std::process::id());
    let artifact = oci(
        &root,
        tree,
        &reference,
        "amd64",
        vec!["/usr/bin/true".into()],
        &Ctx::new(),
    )
    .expect("builds");
    let archive = artifact.object.path_in(&store).expect("in store");
    let _cleanup = Image {
        podman,
        reference: reference.clone(),
    };

    let load = Command::new(podman)
        .arg("load")
        .arg("-i")
        .arg(&archive)
        .output()
        .expect("podman load");
    assert!(
        load.status.success(),
        "podman load: {}",
        String::from_utf8_lossy(&load.stderr)
    );
    let run = Command::new(podman)
        .args(["run", "--rm", &reference])
        .output()
        .expect("podman run");
    assert!(
        run.status.success(),
        "podman run: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let inspect = Command::new(podman)
        .args([
            "image",
            "inspect",
            "--format",
            "{{.Architecture}} {{json .Config.Entrypoint}}",
            &reference,
        ])
        .output()
        .expect("inspect");
    let text = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
    assert_eq!(text, "amd64 [\"/usr/bin/true\"]");
}
