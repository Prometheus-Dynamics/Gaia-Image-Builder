//! A real Buildroot build: a minimal aarch64 config (busybox, a CPIO root
//! filesystem, Buildroot's bootlin toolchain), built twice from the same
//! inputs and once more with one fragment changed.
//!
//! Gated: it runs only with `GAIA_BUILDROOT_TESTS=1`, and then needs
//! `GAIA_BUILDROOT_SOURCE` (a Buildroot tree) and `GAIA_BUILDROOT_DL` (a
//! download cache holding what the config fetches). Without the gate it
//! prints SKIPPED. Optional: `GAIA_BUILDROOT_CPUS` (a CPU list, `taskset`
//! at `/usr/bin/taskset`), `GAIA_BUILDROOT_JOBS` (default 4).

use gaia_os_buildroot::{BuildrootHost, ConfigFragment, OsConfig, build_os};
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_types::{Artifact, Tree};
use std::fs;
use std::path::{Path, PathBuf};

const BASE: &str = "\
BR2_aarch64=y
BR2_TOOLCHAIN_EXTERNAL=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_BLEEDING_EDGE=y
BR2_PACKAGE_BUSYBOX=y
BR2_TARGET_ROOTFS_CPIO=y
BR2_TARGET_ROOTFS_CPIO_GZIP=y
";

fn gated() -> bool {
    if std::env::var_os("GAIA_BUILDROOT_TESTS").is_none() {
        println!(
            "SKIPPED: set GAIA_BUILDROOT_TESTS=1 with GAIA_BUILDROOT_SOURCE and GAIA_BUILDROOT_DL"
        );
        return false;
    }
    true
}

fn env_path(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    assert!(Path::new(&value).is_absolute(), "{name} must be absolute");
    value
}

/// Bytes under `path`, following nothing: links count as their own size.
fn size(path: &Path) -> u64 {
    let meta = fs::symlink_metadata(path).expect("metadata");
    if meta.is_dir() {
        fs::read_dir(path)
            .expect("read_dir")
            .map(|entry| size(&entry.expect("entry").path()))
            .sum()
    } else {
        meta.len()
    }
}

struct Run {
    made: bool,
    outputs: Vec<(String, u64)>,
    rootfs: PathBuf,
    images: PathBuf,
    config: PathBuf,
    logs: PathBuf,
    /// The root filesystem's entries: owners and modes from Buildroot.
    entries: Vec<gaia_types::FileEntry>,
}

fn build(
    store_dir: &Path,
    source: &Tree,
    fragments: Vec<ConfigFragment>,
    host: &BuildrootHost,
) -> Run {
    let store_path = store_dir.display().to_string();
    let (
        rootfs,
        kernel,
        device_trees,
        modules,
        bootloader,
        images,
        config,
        _kernel_build,
        _toolchain,
    ) = build_os(
        &store_path,
        source.clone(),
        &OsConfig {
            fragments,
            ..OsConfig::default()
        },
        &[],
        &gaia_os_buildroot::no_seed(&store_path).expect("seed"),
        &host.clone(),
        &Ctx::new(),
    )
    .expect("the Buildroot step");
    let store = Store::open(store_dir).expect("store");
    let logs = logs_dir(&store);
    let made = step_records(&logs)
        .into_iter()
        .rev()
        .find(|record| record.step == "gaia.os.buildroot")
        .expect("a step record")
        .made;
    let mut outputs = Vec::new();
    let sized = |name: &str, object: &gaia_types::StoreRef| {
        let path = object.path_in(&store).expect("in the store");
        (name.to_string(), size(&path))
    };
    outputs.push(sized("rootfs", &rootfs.object));
    for (name, artifact) in [
        ("kernel", &kernel),
        ("device-trees", &device_trees),
        ("modules", &modules),
        ("bootloader", &bootloader),
        ("images", &images),
        ("config", &config),
    ] {
        let Artifact { object, .. } = artifact;
        outputs.push(sized(name, object));
    }
    let root = |object: &gaia_types::StoreRef| object.path_in(&store).expect("in the store");
    Run {
        made,
        outputs,
        rootfs: root(&rootfs.object),
        entries: rootfs.entries.clone(),
        images: root(&images.object),
        config: root(&config.object),
        logs,
    }
}

fn minimal_fragment(text_extra: &str) -> Vec<ConfigFragment> {
    vec![
        ConfigFragment {
            name: "minimal-aarch64".into(),
            text: BASE.into(),
        },
        ConfigFragment {
            name: "proof".into(),
            text: text_extra.into(),
        },
    ]
}

/// The packages make built in the newest run of this step, read from its
/// run log (`>>> <package> <version> Building` lines).
fn built_in_last_run(logs: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![logs.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("logs").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    let newest = files
        .iter()
        .rev()
        .find(|path| fs::read_to_string(path).is_ok_and(|text| text.contains(">>> ")))
        .expect("a run log");
    let text = fs::read_to_string(newest).expect("read log");
    text.lines()
        .filter_map(|line| {
            let rest = line.split_once(">>> ")?.1;
            let rest = rest.strip_suffix(" Building")?;
            rest.split_whitespace().next().map(str::to_string)
        })
        .collect()
}

#[test]
fn a_minimal_aarch64_buildroot_build_and_its_rebuilds() {
    if !gated() {
        return;
    }
    let source = PathBuf::from(env_path("GAIA_BUILDROOT_SOURCE"));
    let download = env_path("GAIA_BUILDROOT_DL");
    let jobs: u32 = std::env::var("GAIA_BUILDROOT_JOBS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    let cpus = std::env::var("GAIA_BUILDROOT_CPUS").unwrap_or_default();
    let host = BuildrootHost {
        make: "/usr/bin/make".into(),
        taskset: "/usr/bin/taskset".into(),
        cpus,
        jobs,
        download_dir: download,
        config_only: false,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let store_dir = dir.path().join("store");
    let store = Store::open(&store_dir).expect("store");
    let source_tree = Tree {
        object: store
            .put_tree(&source)
            .expect("source into the store")
            .into(),
        entries: Vec::new(),
    };
    drop(store);

    let first = build(&store_dir, &source_tree, minimal_fragment(""), &host);
    println!("first build made={} outputs:", first.made);
    for (name, bytes) in &first.outputs {
        println!("  {name}: {bytes} bytes");
    }
    assert!(first.made, "a first build makes its outputs");
    // The owners and modes Buildroot gives an image, not the build user's.
    let mode = |path: &str| {
        first
            .entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| (e.mode, e.uid, e.gid))
    };
    println!("rootfs entries: {}", first.entries.len());
    for path in ["bin/busybox", "etc/shadow", "root", "tmp"] {
        println!(
            "  {path}: {:?}",
            mode(path).map(|(m, u, g)| (format!("{m:o}"), u, g))
        );
    }
    assert_eq!(
        mode("bin/busybox"),
        Some((0o4755, 0, 0)),
        "busybox is setuid"
    );
    assert_eq!(
        mode("etc/shadow"),
        Some((0o600, 0, 0)),
        "shadow is root's alone"
    );
    assert_eq!(mode("root"), Some((0o700, 0, 0)), "/root is root's alone");
    assert!(
        !first.images.join("rootfs.tar").exists(),
        "the tar the entries are read from is not an output"
    );
    assert!(
        first.rootfs.join("bin/busybox").is_file(),
        "busybox is in the root filesystem"
    );
    assert!(
        first.images.join("rootfs.cpio.gz").is_file(),
        "the CPIO root filesystem is in images"
    );
    let config = fs::read_to_string(&first.config).expect("config");
    assert!(
        config.contains("BR2_aarch64=y"),
        "the resolved config names the arch"
    );

    let again = build(&store_dir, &source_tree, minimal_fragment(""), &host);
    println!("same inputs again: made={}", again.made);
    assert!(
        !again.made,
        "the same inputs are found in the store and nothing is made"
    );

    let changed = build(
        &store_dir,
        &source_tree,
        minimal_fragment("BR2_TARGET_GENERIC_HOSTNAME=\"gaia-proof\"\n"),
        &host,
    );
    let rebuilt = built_in_last_run(&changed.logs);
    println!("one fragment changed: made={}", changed.made);
    println!("packages make built in that run: {rebuilt:?}");
    assert!(
        changed.made,
        "a changed fragment is a new key and makes again"
    );
    assert!(
        fs::read_to_string(&changed.config)
            .expect("config")
            .contains("BR2_TARGET_GENERIC_HOSTNAME=\"gaia-proof\""),
        "the changed fragment is in the resolved config"
    );
}
