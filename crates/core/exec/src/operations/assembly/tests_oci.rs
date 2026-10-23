//! The OCI image output: the layout and docker-archive writers, the tree
//! layers' tar, the blob cache, and the image config. Split from `tests.rs`.
//! The tools these tests read the output with (`tar`, `gzip`, `zstd`) are
//! the host's; `skopeo` is only used by the ignored test.

use super::super::oci::{rfc3339_utc, write_tree_tar};
use super::super::tar::TarWriter;
use super::*;
use gaia_spec::{
    AssemblyFileSpec, AssemblyOciConfigSpec, AssemblyOciLayerSpec, AssemblyOciSpec,
    AssemblySymlinkSpec, AssemblyTreeId, AssemblyTreeSpec, ImageAssemblySpec,
};
use gaia_testkit::TempDir;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::Command;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Runs a host tool and returns its stdout; a failing tool fails the test.
fn tool(program: &str, args: &[&str]) -> Vec<u8> {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn gunzip(path: &Path) -> Vec<u8> {
    tool("gzip", &["-dc", path.to_str().expect("utf-8 path")])
}

fn json_file(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).expect("json file")).expect("json")
}

/// The `tar -tv` listing of a tar, with numeric owners.
fn listing(tar_path: &Path) -> String {
    String::from_utf8(tool(
        "tar",
        &["--numeric-owner", "-tvf", tar_path.to_str().expect("utf-8")],
    ))
    .expect("listing")
}

/// Every regular file under `dir`, by its path relative to `dir`.
fn files_under(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).expect("under root");
                files.insert(
                    relative.display().to_string(),
                    fs::read(&path).expect("file"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

fn disk_env_panic() -> PlacementEnv {
    panic!("an OCI image reads no RAM facts")
}

fn run(spec: &ResolvedBuildSpec) -> AssemblyStagingSummary {
    stage_image_assembly_with(spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("oci assembly")
}

/// A layer tar of plain files, mode 0644.
fn write_layer_tar(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("layer tar");
    let mut writer = TarWriter::new(BufWriter::new(file));
    for (name, bytes) in entries {
        writer
            .append_file(name, 0o644, bytes.len() as u64, &mut &bytes[..], |_| {})
            .expect("layer entry");
    }
    writer
        .finish()
        .expect("finish")
        .into_inner()
        .expect("flush")
        .sync_all()
        .expect("sync");
}

/// The fixtures of one test: a base layer tar, and the app file that fills
/// the `app` tree (as `bin/app`, mode 0755, with a `bin/sh` symlink).
struct Fixture {
    root: PathBuf,
    base: PathBuf,
    app_src: PathBuf,
}

fn fixture(root: &Path, app: &[u8]) -> Fixture {
    fs::create_dir_all(root.join("assets")).expect("assets");
    let base = root.join("assets/base.tar");
    write_layer_tar(
        &base,
        &[
            ("etc/os-release", b"NAME=gaia\n"),
            ("sbin/init", b"#!/bin/sh\n"),
        ],
    );
    let app_src = root.join("assets/app.bin");
    fs::write(&app_src, app).expect("app");
    Fixture {
        root: root.to_path_buf(),
        base,
        app_src,
    }
}

fn oci_entry(fixture: &Fixture, format: &str, compression: &str, dest: &str) -> AssemblyOciSpec {
    AssemblyOciSpec {
        id: "sim".into(),
        dest: dest.into(),
        format: format.into(),
        layers: vec![
            AssemblyOciLayerSpec {
                tar: Some(fixture.base.display().to_string().into()),
                tree: None,
            },
            AssemblyOciLayerSpec {
                tar: None,
                tree: Some(AssemblyTreeId::new("app")),
            },
        ],
        compression: compression.into(),
        reference: Some("sim:dev".into()),
        config: AssemblyOciConfigSpec {
            architecture: "amd64".into(),
            os: "linux".into(),
            entrypoint: vec!["/sbin/init".into()],
            cmd: Vec::new(),
            env: vec!["container=docker".into()],
            working_dir: Some("/".into()),
            stop_signal: Some("SIGRTMIN+3".into()),
            labels: vec![("org.opencontainers.image.revision".into(), "abc123".into())],
        },
    }
}

/// The spec: the app tree, then the OCI image of the base tar and that tree.
fn oci_spec(fixture: &Fixture, oci: AssemblyOciSpec) -> ResolvedBuildSpec {
    let mut spec = test_spec(&fixture.root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "app".into(),
            path: "$assembly.work/app".into(),
            strip: false,
        }],
        files: vec![AssemblyFileSpec {
            tree: "app".into(),
            src: Some(fixture.app_src.display().to_string().into()),
            src_glob: None,
            exclude: Vec::new(),
            include: Vec::new(),
            strip: false,
            content: None,
            json: None,
            dest: "bin/app".into(),
            mode: Some("0755".into()),
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        }],
        symlinks: vec![AssemblySymlinkSpec {
            tree: "app".into(),
            path: "bin/sh".into(),
            target: "app".into(),
        }],
        ocis: vec![oci],
        ..ImageAssemblySpec::default()
    });
    spec
}

#[cfg(unix)]
#[test]
fn oci_layout_is_spec_shaped_and_byte_identical_across_runs() {
    let scratch = TempDir::new("gaia-exec-oci");
    let root = scratch.join("layout");
    let fixture = fixture(&root, b"app v1\n");
    let spec = oci_spec(
        &fixture,
        oci_entry(&fixture, "oci-layout", "gzip", "$assembly.out/sim.oci"),
    );

    let first = run(&spec);
    let out = PathBuf::from(state_value(&first, "oci.1.output"));
    let first_files = files_under(&out);
    // The same inputs with an empty cache give the same bytes.
    fs::remove_dir_all(root.join("build/oci-blobs")).expect("cache");
    let second = run(&spec);
    assert_eq!(state_value(&second, "oci.1.layer.1.reused"), "false");
    assert_eq!(
        files_under(Path::new(&state_value(&second, "oci.1.output"))),
        first_files,
        "the same inputs give the same OCI layout"
    );

    let index = json_file(&out.join("index.json"));
    assert_eq!(index["schemaVersion"], 2);
    let manifest_ref = &index["manifests"][0];
    assert_eq!(
        manifest_ref["mediaType"],
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(
        manifest_ref["annotations"]["org.opencontainers.image.ref.name"],
        "sim:dev"
    );
    let blobs = out.join("blobs/sha256");
    let manifest_hex = manifest_ref["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .expect("manifest digest");
    let manifest_bytes = fs::read(blobs.join(manifest_hex)).expect("manifest blob");
    assert_eq!(sha256_bytes(&manifest_bytes), manifest_hex);
    assert_eq!(manifest_ref["size"], manifest_bytes.len());
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).expect("manifest");
    assert_eq!(manifest["schemaVersion"], 2);
    let layers = manifest["layers"].as_array().expect("layers");
    assert_eq!(layers.len(), 2);
    for layer in layers {
        assert_eq!(
            layer["mediaType"],
            "application/vnd.oci.image.layer.v1.tar+gzip"
        );
        let hex_digest = layer["digest"]
            .as_str()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .expect("layer digest");
        let blob = fs::read(blobs.join(hex_digest)).expect("layer blob");
        assert_eq!(sha256_bytes(&blob), hex_digest, "blob named by its digest");
        assert_eq!(layer["size"], blob.len());
    }

    let config_hex = manifest["config"]["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .expect("config digest");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(blobs.join(config_hex)).expect("config blob"))
            .expect("config");
    assert_eq!(config["architecture"], "amd64");
    assert_eq!(config["os"], "linux");
    assert_eq!(config["created"], "1970-01-01T00:00:00Z");
    assert_eq!(config["config"]["Entrypoint"][0], "/sbin/init");
    assert_eq!(config["config"]["Env"][0], "container=docker");
    assert_eq!(config["config"]["WorkingDir"], "/");
    assert_eq!(config["config"]["StopSignal"], "SIGRTMIN+3");
    assert_eq!(
        config["config"]["Labels"]["org.opencontainers.image.revision"],
        "abc123"
    );
    assert!(
        config["config"].get("Cmd").is_none(),
        "an empty cmd is left out"
    );
    assert_eq!(config["rootfs"]["type"], "layers");
    let diff_ids = config["rootfs"]["diff_ids"].as_array().expect("diff_ids");
    assert_eq!(diff_ids.len(), 2);
    assert_eq!(config["history"].as_array().expect("history").len(), 2);
    for (index, (layer, diff_id)) in layers.iter().zip(diff_ids).enumerate() {
        let blob_hex = layer["digest"].as_str().expect("digest")["sha256:".len()..].to_string();
        let uncompressed = gunzip(&blobs.join(blob_hex));
        assert_eq!(
            diff_id.as_str().expect("diff id"),
            format!("sha256:{}", sha256_bytes(&uncompressed)),
            "diff_id {index} is the digest of the uncompressed tar"
        );
    }
    let base_tar = fs::read(&fixture.base).expect("base");
    let base_blob = layers[0]["digest"].as_str().expect("digest")["sha256:".len()..].to_string();
    assert_eq!(
        gunzip(&blobs.join(base_blob)),
        base_tar,
        "tar layers are stored as they are"
    );
    let app_blob = layers[1]["digest"].as_str().expect("digest")["sha256:".len()..].to_string();
    let app_tar = blobs.join(app_blob);
    let app_listing = listing_of_gzip(&app_tar, &scratch.join("app-layer"));
    assert!(app_listing.contains("-rwxr-xr-x 0/0"), "{app_listing}");
    assert!(app_listing.contains("bin/sh -> app"), "{app_listing}");
    assert_eq!(
        fs::read_to_string(out.join("oci-layout")).expect("oci-layout"),
        "{\"imageLayoutVersion\":\"1.0.0\"}\n"
    );
    assert_eq!(state_value(&first, "completed_oci_count"), "1");
    assert_eq!(state_value(&first, "oci.1.layer_count"), "2");
}

/// The listing of a gzip-compressed tar, decompressed into `scratch`.
fn listing_of_gzip(path: &Path, scratch: &Path) -> String {
    fs::create_dir_all(scratch).expect("scratch dir");
    let plain = scratch.join("plain.tar");
    fs::write(&plain, gunzip(path)).expect("plain tar");
    listing(&plain)
}

#[cfg(unix)]
#[test]
fn an_app_only_change_recompresses_only_the_app_layer() {
    let scratch = TempDir::new("gaia-exec-oci");
    let root = scratch.join("cache");
    let fixture = fixture(&root, &vec![b'a'; 4 * 1024 * 1024]);
    // A base layer big enough for its compression to show in the timings.
    let base: Vec<u8> = (0..48 * 1024 * 1024usize)
        .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    write_layer_tar(&fixture.base, &[("usr/share/base.bin", &base)]);
    let spec = oci_spec(
        &fixture,
        oci_entry(&fixture, "oci-layout", "gzip", "$assembly.out/sim.oci"),
    );

    let started = std::time::Instant::now();
    let first = run(&spec);
    let first_elapsed = started.elapsed();
    assert_eq!(state_value(&first, "oci.1.layer.1.reused"), "false");
    assert_eq!(state_value(&first, "oci.1.layer.2.reused"), "false");

    fs::write(&fixture.app_src, vec![b'b'; 4 * 1024 * 1024]).expect("changed app");
    let started = std::time::Instant::now();
    let second = run(&spec);
    let second_elapsed = started.elapsed();
    assert_eq!(
        state_value(&second, "oci.1.layer.1.reused"),
        "true",
        "the base layer comes from the cache"
    );
    assert_eq!(state_value(&second, "oci.1.layer.2.reused"), "false");
    assert_eq!(
        state_value(&first, "oci.1.layer.1.blob_digest"),
        state_value(&second, "oci.1.layer.1.blob_digest"),
        "the base blob is unchanged"
    );
    assert_ne!(
        state_value(&first, "oci.1.layer.2.blob_digest"),
        state_value(&second, "oci.1.layer.2.blob_digest"),
        "the app blob changed"
    );
    assert_ne!(
        state_value(&first, "oci.1.config_digest"),
        state_value(&second, "oci.1.config_digest")
    );
    eprintln!(
        "oci app-only change: first run {:.2}s, second run {:.2}s (one layer recompressed)",
        first_elapsed.as_secs_f64(),
        second_elapsed.as_secs_f64()
    );
}

#[cfg(unix)]
#[test]
fn docker_archive_holds_the_config_manifest_and_gzip_layers() {
    let scratch = TempDir::new("gaia-exec-oci");
    let root = scratch.join("docker");
    let fixture = fixture(&root, b"app\n");
    let spec = oci_spec(
        &fixture,
        oci_entry(&fixture, "docker-archive", "gzip", "$assembly.out/sim.tar"),
    );
    let summary = run(&spec);
    let archive = PathBuf::from(state_value(&summary, "oci.1.output"));
    assert!(archive.is_file(), "a docker archive is one tar file");

    let names = listing(&archive);
    let manifest: serde_json::Value = serde_json::from_slice(&tool(
        "tar",
        &["-xOf", archive.to_str().expect("utf-8"), "manifest.json"],
    ))
    .expect("manifest.json");
    let entry = &manifest[0];
    assert_eq!(entry["RepoTags"][0], "sim:dev");
    let config_name = entry["Config"].as_str().expect("Config");
    assert!(names.contains(config_name), "{names}");
    let layer_names = entry["Layers"].as_array().expect("Layers");
    assert_eq!(layer_names.len(), 2);

    let extract = scratch.join("extracted");
    fs::create_dir_all(&extract).expect("extract dir");
    tool(
        "tar",
        &[
            "-xf",
            archive.to_str().expect("utf-8"),
            "-C",
            extract.to_str().expect("utf-8"),
        ],
    );
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(extract.join(config_name)).expect("config"))
            .expect("json");
    let diff_ids = config["rootfs"]["diff_ids"].as_array().expect("diff_ids");
    for (name, diff_id) in layer_names.iter().zip(diff_ids) {
        let layer = gunzip(&extract.join(name.as_str().expect("name")));
        assert_eq!(
            diff_id.as_str().expect("diff id"),
            format!("sha256:{}", sha256_bytes(&layer))
        );
    }
    assert_eq!(
        gunzip(&extract.join(layer_names[0].as_str().expect("name"))),
        fs::read(&fixture.base).expect("base")
    );
}

#[cfg(unix)]
#[test]
fn zstd_and_uncompressed_layers_are_stored_with_their_media_types() {
    for (compression, media_type) in [
        ("zstd", "application/vnd.oci.image.layer.v1.tar+zstd"),
        ("none", "application/vnd.oci.image.layer.v1.tar"),
    ] {
        let scratch = TempDir::new("gaia-exec-oci");
        let root = scratch.join(compression);
        let fixture = fixture(&root, b"app\n");
        let spec = oci_spec(
            &fixture,
            oci_entry(&fixture, "oci-layout", compression, "$assembly.out/sim.oci"),
        );
        let summary = run(&spec);
        let out = PathBuf::from(state_value(&summary, "oci.1.output"));
        let index = json_file(&out.join("index.json"));
        let manifest_hex = index["manifests"][0]["digest"].as_str().expect("digest")
            ["sha256:".len()..]
            .to_string();
        let manifest = json_file(&out.join("blobs/sha256").join(manifest_hex));
        let layers = manifest["layers"].as_array().expect("layers");
        assert!(layers.iter().all(|layer| layer["mediaType"] == media_type));
        let base_digest =
            layers[0]["digest"].as_str().expect("digest")["sha256:".len()..].to_string();
        let blob = out.join("blobs/sha256").join(&base_digest);
        let base = fs::read(&fixture.base).expect("base");
        if compression == "none" {
            assert_eq!(fs::read(&blob).expect("blob"), base);
            assert_eq!(
                base_digest,
                sha256_bytes(&base),
                "uncompressed: blob is the diff_id"
            );
        } else {
            let plain = tool("zstd", &["-dc", blob.to_str().expect("utf-8")]);
            assert_eq!(plain, base);
        }
    }
}

#[cfg(unix)]
#[test]
fn tree_tar_keeps_modes_links_and_zero_owners_and_is_deterministic() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-exec-oci");
    let tree = scratch.join("tree");
    fs::create_dir_all(tree.join("bin")).expect("bin");
    fs::create_dir_all(tree.join("lib")).expect("lib");
    fs::write(tree.join("bin/app"), b"app").expect("app");
    fs::set_permissions(tree.join("bin/app"), fs::Permissions::from_mode(0o755)).expect("mode");
    fs::write(tree.join("etc-conf"), b"conf").expect("conf");
    fs::set_permissions(tree.join("etc-conf"), fs::Permissions::from_mode(0o600)).expect("mode");
    fs::write(tree.join("lib/one.so"), b"lib").expect("lib file");
    fs::hard_link(tree.join("lib/one.so"), tree.join("lib/two.so")).expect("hard link");
    fs::set_permissions(tree.join("lib"), fs::Permissions::from_mode(0o700)).expect("dir mode");
    std::os::unix::fs::symlink("app", tree.join("bin/sh")).expect("symlink");

    let first_tar = scratch.join("first.tar");
    let first = write_tree_tar(&tree, &first_tar).expect("first tar");
    // Only the mtimes change: the layer is the same.
    for path in ["bin/app", "etc-conf", "lib/one.so"] {
        let file = fs::File::open(tree.join(path)).expect("open");
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_234_567_890))
            .expect("mtime");
    }
    let second_tar = scratch.join("second.tar");
    let second = write_tree_tar(&tree, &second_tar).expect("second tar");
    assert_eq!(first, second, "the same tree gives the same diff_id");
    assert_eq!(
        fs::read(&first_tar).expect("a"),
        fs::read(&second_tar).expect("b")
    );
    assert_eq!(first, sha256_bytes(&fs::read(&first_tar).expect("tar")));

    let text = listing(&first_tar);
    assert!(text.contains("-rwxr-xr-x 0/0"), "{text}");
    assert!(text.contains("-rw------- 0/0"), "{text}");
    assert!(text.contains("drwx------ 0/0"), "{text}");
    assert!(text.contains("lrwxrwxrwx 0/0"), "{text}");
    assert!(text.contains("bin/sh -> app"), "{text}");
    assert!(
        text.contains("hrw-r--r-- 0/0"),
        "the second name is a hard link: {text}"
    );
    assert!(text.contains("lib/two.so link to lib/one.so"), "{text}");
}

#[test]
fn rfc3339_times_come_from_the_epoch_seconds() {
    assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
}

/// Checks the layout and the docker archive with `skopeo`, which reads
/// both formats the way a container tool would, without running anything.
#[cfg(unix)]
#[test]
#[ignore = "needs skopeo on the host; run with --ignored"]
fn skopeo_reads_the_oci_layout_and_the_docker_archive() {
    if Command::new("skopeo").arg("--version").output().is_err() {
        eprintln!("skipped: skopeo is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-oci");
    let root = scratch.join("skopeo");
    let fixture = fixture(&root, b"app\n");
    let layout_spec = oci_spec(
        &fixture,
        oci_entry(&fixture, "oci-layout", "gzip", "$assembly.out/sim.oci"),
    );
    let layout = PathBuf::from(state_value(&run(&layout_spec), "oci.1.output"));
    let config = Command::new("skopeo")
        .args(["inspect", "--config"])
        .arg(format!("oci:{}", layout.display()))
        .output()
        .expect("skopeo");
    assert!(
        config.status.success(),
        "skopeo inspect oci layout: {}",
        String::from_utf8_lossy(&config.stderr)
    );

    let archive_spec = oci_spec(
        &fixture,
        oci_entry(&fixture, "docker-archive", "gzip", "$assembly.out/sim.tar"),
    );
    let archive = PathBuf::from(state_value(&run(&archive_spec), "oci.1.output"));
    let inspected = Command::new("skopeo")
        .args(["inspect", "--config"])
        .arg(format!("docker-archive:{}", archive.display()))
        .output()
        .expect("skopeo");
    assert!(
        inspected.status.success(),
        "skopeo inspect docker archive: {}",
        String::from_utf8_lossy(&inspected.stderr)
    );
}
