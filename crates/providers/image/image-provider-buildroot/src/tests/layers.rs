//! `[image.layers`: membership by selector, overlaps, owners, the layer
//! tars' contents, reuse of unchanged layers, and determinism.
use crate::layers::layer_json;
use crate::layers::{LAYERS_JSON, build_layers};
use crate::layers_erofs::VerityRecord;
use crate::layers_tar::{EntryKind, LayerTarWriter, read_entries};
use gaia_spec::{
    ArtifactRef, ImageLayerSpec, ImageLayersSpec, InstallEntrySpec, ResolvedBuildSpec,
    StageServiceSpec,
};
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// One source entry for the fake rootfs tar.
enum Src<'a> {
    Dir(&'a str, u32),
    File(&'a str, u32, u64, u64, &'a str),
    Symlink(&'a str, &'a str),
    Hardlink(&'a str, &'a str, u32),
}

/// Writes a rootfs tar with `entries`, as Buildroot's `rootfs.tar` has them.
fn write_rootfs(path: &Path, entries: &[Src<'_>]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    let file = fs::File::create(path).expect("create tar");
    let mut writer = LayerTarWriter::new(BufWriter::new(file), 0);
    for entry in entries {
        match entry {
            Src::Dir(name, mode) => writer.dir(name, *mode, 0, 0).expect("dir"),
            Src::File(name, mode, uid, gid, content) => writer
                .file(name, *mode, *uid, *gid, content.len() as u64, |out| {
                    out.write_all(content.as_bytes())
                        .map_err(|error| error.to_string())
                })
                .expect("file"),
            Src::Symlink(name, target) => writer.symlink(name, target, 0, 0).expect("symlink"),
            Src::Hardlink(name, target, mode) => writer
                .hardlink(name, target, *mode, 0, 0)
                .expect("hardlink"),
        }
    }
    let mut out = writer.finish().expect("finish");
    out.flush().expect("flush");
}

/// The rootfs of these tests: a base, a platform layer (kernel modules) and an
/// app layer (an install and a stage entry, plus a hardlink across layers).
fn rootfs_entries(run_body: &str) -> Vec<Src<'_>> {
    vec![
        Src::Dir("etc", 0o755),
        Src::File("etc/passwd", 0o644, 0, 0, "root:x:0:0\n"),
        Src::Symlink("etc/alias", "passwd"),
        Src::Dir("usr", 0o755),
        Src::Dir("usr/bin", 0o755),
        Src::File("usr/bin/busybox", 0o755, 0, 0, "busybox-bytes"),
        Src::Hardlink("usr/bin/busy2", "usr/bin/busybox", 0o755),
        Src::Dir("lib", 0o755),
        Src::Dir("lib/modules", 0o755),
        Src::File("lib/modules/x.ko", 0o644, 0, 0, "module-bytes"),
        Src::Dir("opt", 0o755),
        Src::Dir("opt/app", 0o755),
        Src::File("opt/app/run", 0o750, 1000, 1000, run_body),
        Src::File("usr/bin/helios", 0o755, 0, 0, "helios-bytes"),
        Src::Hardlink("opt/app/tool", "usr/bin/busybox", 0o755),
    ]
}

fn layer(
    name: &str,
    packages: &[&str],
    paths: &[&str],
    installs: &[&str],
    stage: &[&str],
) -> ImageLayerSpec {
    ImageLayerSpec {
        name: name.into(),
        packages: packages.iter().map(|s| s.to_string()).collect(),
        paths: paths.iter().map(|s| s.to_string()).collect(),
        installs: installs.iter().map(|s| s.to_string()).collect(),
        stage: stage.iter().map(|s| s.to_string()).collect(),
        services: Vec::new(),
    }
}

fn layers_spec(overrides: &[&str]) -> ImageLayersSpec {
    ImageLayersSpec {
        order: vec!["base".into(), "platform".into(), "app".into()],
        default: "base".into(),
        overrides: overrides.iter().map(|s| s.to_string()).collect(),
        erofs: false,
        verity: false,
        layers: vec![
            layer("platform", &["linux"], &["/lib/modules"], &[], &[]),
            layer("app", &["helios-pkg"], &[], &["helios-*"], &["helios-*"]),
        ],
    }
}

/// The build spec: the install `helios-install` lands at `/opt/app`, and the
/// stage service `helios-svc` has a unit file.
fn build_spec() -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("layers-test");
    spec.install.entries.push(InstallEntrySpec {
        id: "helios-install".into(),
        artifact: ArtifactRef::new("helios"),
        dest: "/opt/app".into(),
        replace: false,
        mode: None,
        owner: None,
        group: None,
    });
    spec.stage.services.push(StageServiceSpec {
        id: "helios-svc".into(),
        name: "helios".into(),
        unit_path: "/usr/lib/systemd/system/helios.service".into(),
    });
    spec
}

/// A Buildroot output dir: `images/rootfs.tar`, the package list and the
/// stage ownership record that the layers read.
fn fake_tree(root: &Path, entries: &[Src<'_>], packages: &str) -> PathBuf {
    let tree = root.join("tree");
    write_rootfs(&tree.join("images/rootfs.tar"), entries);
    fs::create_dir_all(tree.join("build")).expect("build dir");
    fs::write(tree.join("build/packages-file-list.txt"), packages).expect("package list");
    fs::write(
        tree.join(".gaia-image-feed-stage-owned.txt"),
        "gaia-image-feed-stage-owned-v1\nhelios-bin\tusr/bin/helios\tfile\t10\t0\n",
    )
    .expect("stage record");
    tree
}

const PACKAGES: &str =
    "busybox,./usr/bin/busybox\nbusybox,./usr/bin/busy2\nlinux,./lib/modules/x.ko\n";

fn names_of(path: &Path) -> BTreeSet<String> {
    read_entries(path)
        .expect("read layer")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn every_path_lands_in_one_layer_and_directories_repeat() {
    let dir = TempDir::new("layers-membership");
    let tree = fake_tree(dir.path(), &rootfs_entries("run-v1"), PACKAGES);
    let collect = dir.path().join("collect");
    let messages =
        build_layers(&build_spec(), &layers_spec(&[]), &tree, &collect).expect("layers build");
    assert!(messages.last().expect("summary").contains("0 of 3 reused"));

    let base = names_of(&collect.join("layers/base.tar"));
    let platform = names_of(&collect.join("layers/platform.tar"));
    let app = names_of(&collect.join("layers/app.tar"));
    // Each file is in exactly one layer.
    for file in [
        "etc/passwd",
        "usr/bin/busybox",
        "usr/bin/busy2",
        "lib/modules/x.ko",
        "opt/app/run",
        "usr/bin/helios",
        "opt/app/tool",
    ] {
        let count = [&base, &platform, &app]
            .iter()
            .filter(|set| set.contains(file))
            .count();
        assert_eq!(count, 1, "{file} is in {count} layers");
    }
    assert!(base.contains("etc/passwd") && base.contains("usr/bin/busybox"));
    assert!(platform.contains("lib/modules/x.ko"));
    assert!(app.contains("opt/app/run") && app.contains("usr/bin/helios"));
    // Directories above members are repeated where they are needed.
    assert!(app.contains("usr/bin") && app.contains("opt") && app.contains("usr"));
    assert!(platform.contains("lib") && platform.contains("lib/modules"));
    assert!(!platform.contains("etc/passwd") && !base.contains("lib/modules/x.ko"));
}

#[test]
fn owners_modes_links_and_data_are_carried_over() {
    let dir = TempDir::new("layers-owners");
    let tree = fake_tree(dir.path(), &rootfs_entries("run-v1"), PACKAGES);
    let collect = dir.path().join("collect");
    build_layers(&build_spec(), &layers_spec(&[]), &tree, &collect).expect("build");

    let base = read_entries(&collect.join("layers/base.tar")).expect("base");
    let run = read_entries(&collect.join("layers/app.tar")).expect("app");
    let run_entry = &run["opt/app/run"];
    assert_eq!(
        (run_entry.uid, run_entry.gid, run_entry.mode),
        (1000, 1000, 0o750)
    );
    assert_eq!(run_entry.digest, gaia_process::sha256_hex_bytes(b"run-v1"));
    // A hardlink whose target shares its layer stays a hardlink.
    assert_eq!(base["usr/bin/busy2"].kind, EntryKind::Hardlink);
    assert_eq!(base["usr/bin/busy2"].link, "usr/bin/busybox");
    // A hardlink whose target lives in another layer is a copy of its data.
    let tool = &run["opt/app/tool"];
    assert_eq!(tool.kind, EntryKind::File);
    assert_eq!(
        tool.digest,
        gaia_process::sha256_hex_bytes(b"busybox-bytes")
    );
    // Symlinks keep their target.
    assert_eq!(base["etc/alias"].kind, EntryKind::Symlink);
    assert_eq!(base["etc/alias"].link, "passwd");
}

#[test]
fn a_file_selected_by_two_layers_needs_an_override() {
    let dir = TempDir::new("layers-overlap");
    let entries = vec![
        Src::Dir("usr", 0o755),
        Src::Dir("usr/lib", 0o755),
        Src::File("usr/lib/libshared.so", 0o644, 0, 0, "shared"),
    ];
    let packages = "libfoo,./usr/lib/libshared.so\nlibbar,./usr/lib/libshared.so\n";
    let mut spec_layers = layers_spec(&[]);
    spec_layers.layers = vec![
        layer("platform", &["libfoo"], &[], &[], &[]),
        layer("app", &["libbar"], &[], &[], &[]),
    ];
    let tree = fake_tree(dir.path(), &entries, packages);
    let error = build_layers(
        &build_spec(),
        &spec_layers,
        &tree,
        &dir.path().join("collect"),
    )
    .expect_err("overlap is an error");
    assert!(error.message.contains("overrides"), "{}", error.message);

    spec_layers.overrides = vec!["/usr/lib/libshared.so".into()];
    let collect = dir.path().join("collect-override");
    build_layers(&build_spec(), &spec_layers, &tree, &collect).expect("override builds");
    assert!(names_of(&collect.join("layers/app.tar")).contains("usr/lib/libshared.so"));
    assert!(!names_of(&collect.join("layers/platform.tar")).contains("usr/lib/libshared.so"));
}

#[test]
fn unchanged_layers_are_reused_and_only_the_changed_one_is_rebuilt() {
    let dir = TempDir::new("layers-reuse");
    let tree = fake_tree(dir.path(), &rootfs_entries("run-v1"), PACKAGES);
    let collect = dir.path().join("collect");
    let spec = build_spec();
    let layers = layers_spec(&[]);
    build_layers(&spec, &layers, &tree, &collect).expect("first build");
    let base_before = fs::read(collect.join("layers/base.tar")).expect("base");

    let again = build_layers(&spec, &layers, &tree, &collect).expect("second build");
    assert!(
        again
            .last()
            .expect("summary")
            .contains("3 of 3 reused, 0 rebuilt"),
        "{again:?}"
    );

    write_rootfs(&tree.join("images/rootfs.tar"), &rootfs_entries("run-v2"));
    let third = build_layers(&spec, &layers, &tree, &collect).expect("third build");
    let text = third.join("\n");
    assert!(
        text.contains("layer base") && text.contains("layer app"),
        "{text}"
    );
    assert!(
        third
            .last()
            .expect("summary")
            .contains("2 of 3 reused, 1 rebuilt"),
        "{third:?}"
    );
    assert!(
        third
            .iter()
            .any(|line| line.starts_with("layer app") && line.ends_with("rebuilt"))
    );
    assert!(
        third
            .iter()
            .any(|line| line.starts_with("layer base") && line.ends_with("reused"))
    );
    assert_eq!(
        fs::read(collect.join("layers/base.tar")).expect("base"),
        base_before
    );
    let app = read_entries(&collect.join("layers/app.tar")).expect("app");
    assert_eq!(
        app["opt/app/run"].digest,
        gaia_process::sha256_hex_bytes(b"run-v2")
    );
}

#[test]
fn layer_outputs_are_byte_identical_for_the_same_rootfs() {
    let dir = TempDir::new("layers-determinism");
    let tree = fake_tree(dir.path(), &rootfs_entries("run-v1"), PACKAGES);
    let (first, second) = (dir.path().join("first"), dir.path().join("second"));
    build_layers(&build_spec(), &layers_spec(&[]), &tree, &first).expect("first");
    build_layers(&build_spec(), &layers_spec(&[]), &tree, &second).expect("second");
    for file in ["base.tar", "platform.tar", "app.tar", LAYERS_JSON] {
        let path = |root: &Path| root.join("layers").join(file);
        assert_eq!(
            fs::read(path(&first)).expect("first output"),
            fs::read(path(&second)).expect("second output"),
            "{file} differs between runs"
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(first.join("layers").join(LAYERS_JSON)).expect("json"))
            .expect("parse");
    assert_eq!(json["schema"], 1);
    assert_eq!(
        json["order"],
        serde_json::json!(["base", "platform", "app"])
    );
    let layers = json["layers"].as_array().expect("layers");
    assert_eq!(layers.len(), 3);
    let base = &layers[0];
    assert_eq!(base["name"], "base");
    assert_eq!(base["tar"], "base.tar");
    assert_eq!(
        base["tar_sha256"],
        gaia_process::sha256_file(&first.join("layers/base.tar")).expect("hash")
    );
    assert_eq!(
        base["tar_size"],
        fs::metadata(first.join("layers/base.tar"))
            .expect("meta")
            .len()
    );
    assert!(base["erofs"].is_null() && base["verity"].is_null());
    assert!(
        base["inputs"]
            .as_str()
            .expect("inputs")
            .starts_with("sha256:")
    );
}

#[test]
fn a_missing_rootfs_tar_is_reported_with_the_setting_to_turn_on() {
    let dir = TempDir::new("layers-missing-tar");
    let tree = dir.path().join("tree");
    fs::create_dir_all(&tree).expect("tree");
    let error = build_layers(
        &build_spec(),
        &layers_spec(&[]),
        &tree,
        &dir.path().join("c"),
    )
    .expect_err("no tar");
    assert!(error.message.contains("rootfs tar"), "{}", error.message);
}

#[test]
fn gnu_tar_output_with_long_names_is_read() {
    let dir = TempDir::new("layers-gnu-tar");
    let tree = dir.path().join("tree");
    let deep = format!("usr/share/{}/data.txt", "segment-".repeat(14));
    fs::create_dir_all(
        tree.join("usr/share")
            .join(deep.split('/').nth(2).expect("part")),
    )
    .expect("deep dir");
    fs::write(tree.join(&deep), "deep").expect("deep file");
    std::os::unix::fs::symlink("data.txt", tree.join("usr/share/link")).expect("link");
    for format in ["gnu", "posix"] {
        let out = dir.path().join(format!("{format}.tar"));
        let status = std::process::Command::new("tar")
            .arg(format!("--format={format}"))
            .arg("-C")
            .arg(&tree)
            .arg("-cf")
            .arg(&out)
            .arg(".")
            .status()
            .expect("run tar");
        assert!(status.success());
        let entries = read_entries(&out).expect("read");
        let file = entries
            .get(&deep)
            .unwrap_or_else(|| panic!("{format}: {deep} missing"));
        assert_eq!(file.digest, gaia_process::sha256_hex_bytes(b"deep"));
        assert_eq!(entries["usr/share/link"].kind, EntryKind::Symlink);
        assert_eq!(entries["usr/share/link"].link, "data.txt");
        assert!(
            entries.contains_key("usr"),
            "{format}: directories are read"
        );
    }
}

/// Timing on a synthetic rootfs of 6,000 files (96 MB) over three layers:
/// a cold build, then a reuse. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn synthetic_rootfs_timing() {
    let dir = TempDir::new("layers-timing");
    let body = "x".repeat(16 * 1024);
    let names: Vec<String> = (0..6000)
        .map(|i| format!("usr/share/pkg{}/f{i}.dat", i % 60))
        .collect();
    let dirs: Vec<String> = (0..60).map(|i| format!("usr/share/pkg{i}")).collect();
    let mut entries: Vec<Src<'_>> = vec![Src::Dir("usr", 0o755), Src::Dir("usr/share", 0o755)];
    for d in &dirs {
        entries.push(Src::Dir(d, 0o755));
    }
    for name in &names {
        entries.push(Src::File(name, 0o644, 0, 0, &body));
    }
    let mut packages = String::new();
    for (i, name) in names.iter().enumerate() {
        packages.push_str(&format!("pkg{},./{name}\n", i % 60));
    }
    let tree = fake_tree(dir.path(), &entries, &packages);
    let mut layers = layers_spec(&[]);
    layers.layers = vec![
        layer("platform", &["pkg1*", "pkg2*", "pkg3*"], &[], &[], &[]),
        layer("app", &["pkg4*", "pkg5*"], &[], &[], &[]),
    ];
    let collect = dir.path().join("collect");
    let start = std::time::Instant::now();
    build_layers(&build_spec(), &layers, &tree, &collect).expect("cold");
    let cold = start.elapsed();
    let start = std::time::Instant::now();
    let again = build_layers(&build_spec(), &layers, &tree, &collect).expect("reuse");
    let reuse = start.elapsed();
    eprintln!(
        "synthetic layers: cold {cold:?}, reuse {reuse:?}; {}",
        again.last().expect("summary")
    );
}

#[test]
fn id_is_the_erofs_sha256_and_verity_records_the_transform_fields() {
    let erofs_sha = "ab".repeat(32);
    let record = VerityRecord {
        root_hash: "cd".repeat(32),
        hash_offset: 8192,
        data_blocks: 2,
        salt: "ef".repeat(32),
    };
    let json = layer_json(
        "app",
        &"01".repeat(32),
        100,
        Some((erofs_sha.clone(), 12288, Some(record))),
        "sha256:x",
    );
    assert_eq!(json["id"], erofs_sha);
    assert_eq!(json["erofs_sha256"], erofs_sha);
    assert_eq!(json["erofs"], "app.erofs");
    assert_eq!(json["tar_sha256"], "01".repeat(32));
    let verity = &json["verity"];
    assert_eq!(verity["root_hash"], "cd".repeat(32));
    assert_eq!(verity["salt"], "ef".repeat(32));
    assert_eq!(verity["hash_offset"], 8192);
    assert_eq!(verity["data_blocks"], 2);
    assert_eq!(verity["data_block_size"], 4096);
    assert_eq!(verity["hash_block_size"], 4096);

    let without = layer_json("base", &"02".repeat(32), 5, None, "sha256:y");
    assert!(without["id"].is_null() && without["erofs"].is_null() && without["verity"].is_null());
}

/// End to end with the real tools: the id is the sha256 of the erofs file as
/// shipped (verity tree included), and the verity fields are recorded. Skipped
/// where `mkfs.erofs` or `veritysetup` is not installed.
#[test]
fn erofs_and_verity_end_to_end_when_the_tools_exist() {
    let tools_present = |tool: &str| {
        std::process::Command::new(tool)
            .arg("--version")
            .output()
            .is_ok()
    };
    if !tools_present("mkfs.erofs") || !tools_present("veritysetup") {
        eprintln!("skipped: mkfs.erofs and veritysetup are needed");
        return;
    }
    let dir = TempDir::new("layers-erofs-verity");
    let tree = fake_tree(dir.path(), &rootfs_entries("run-v1"), PACKAGES);
    let collect = dir.path().join("collect");
    let mut layers = layers_spec(&[]);
    layers.erofs = true;
    layers.verity = true;
    build_layers(&build_spec(), &layers, &tree, &collect).expect("build");
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(collect.join("layers").join(LAYERS_JSON)).expect("json"))
            .expect("parse");
    let app = &json["layers"][2];
    let erofs = collect.join("layers/app.erofs");
    assert_eq!(app["id"], gaia_process::sha256_file(&erofs).expect("hash"));
    assert_eq!(app["erofs_sha256"], app["id"]);
    assert!(
        app["verity"]["root_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64)
    );
    assert!(app["verity"]["hash_offset"].as_u64().is_some());
    // The appended tree verifies against the recorded root hash.
    let verify = std::process::Command::new("veritysetup")
        .arg("verify")
        .arg(&erofs)
        .arg(&erofs)
        .arg(app["verity"]["root_hash"].as_str().expect("root hash"))
        .arg(format!(
            "--hash-offset={}",
            app["verity"]["hash_offset"].as_u64().expect("offset")
        ))
        .arg(format!(
            "--salt={}",
            app["verity"]["salt"].as_str().expect("salt")
        ))
        .output()
        .expect("veritysetup verify");
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    // The erofs before the tree is a valid image.
    if tools_present("fsck.erofs") {
        let fsck = std::process::Command::new("fsck.erofs")
            .arg(&erofs)
            .output()
            .expect("fsck.erofs");
        assert!(
            fsck.status.success(),
            "{}",
            String::from_utf8_lossy(&fsck.stderr)
        );
    }
}
