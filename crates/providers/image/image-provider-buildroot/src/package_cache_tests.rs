use super::*;
use gaia_testkit::TempDir;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::UNIX_EPOCH;

#[test]
fn stamps_are_recorded_in_build_order() {
    let dir_scratch = TempDir::new("stamps");
    let dir = dir_scratch.join("dir");
    fs::create_dir_all(&dir).expect("dir");
    let now = std::time::SystemTime::now();
    for (offset, stamp) in [
        (0, ".stamp_patched"),
        (1, ".stamp_dotconfig"),
        (2, ".stamp_kconfig_fixup_done"),
        (3, ".stamp_configured"),
        (5, ".stamp_installed"),
        (5, ".stamp_target_installed"),
    ] {
        let file = fs::File::create(dir.join(stamp)).expect("stamp");
        file.set_modified(now + Duration::from_secs(offset))
            .expect("mtime");
    }
    assert_eq!(
        stamps_in(&dir),
        [
            ".stamp_patched",
            ".stamp_dotconfig",
            ".stamp_kconfig_fixup_done",
            ".stamp_configured",
            ".stamp_target_installed",
            ".stamp_installed"
        ]
    );
}

fn write(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, contents).expect("file");
}

/// A tree where `base` (a dependency) and `app` were built: per-package
/// directories (app's holds a copy of base's files), file lists, stamps.
pub(super) fn built_tree(output: &Path, app_binary: &[u8]) {
    let out = output.display().to_string();
    write(
        &output.join("per-package/base/target/usr/lib/libbase.so"),
        b"base",
    );
    // Buildroot copies a dependency's files in as hard links.
    fs::create_dir_all(output.join("per-package/app/target/usr/lib")).expect("lib");
    fs::hard_link(
        output.join("per-package/base/target/usr/lib/libbase.so"),
        output.join("per-package/app/target/usr/lib/libbase.so"),
    )
    .expect("link");
    // Installed outside the install steps, so in no file list.
    write(&output.join("per-package/app/host/opt/tool"), b"tool");
    write(
        &output.join("per-package/app/target/usr/bin/app"),
        app_binary,
    );
    fs::create_dir_all(output.join("per-package/app/target/var/lib/app")).expect("empty dir");
    write(
        &output.join("per-package/app/host/sysroot/usr/lib/pkgconfig/app.pc"),
        format!("prefix={out}/per-package/app/host\n").as_bytes(),
    );
    write(&output.join("images/app.img"), b"image");
    for (name, lists) in [
        (
            "base",
            vec![(".files-list.txt", "base,./usr/lib/libbase.so\n")],
        ),
        (
            "app",
            vec![
                (".files-list.txt", "app,./usr/bin/app\n"),
                (
                    ".files-list-staging.txt",
                    "app,./usr/lib/pkgconfig/app.pc\n",
                ),
                (".files-list-images.txt", "app,./app.img\n"),
            ],
        ),
    ] {
        let stamp_dir = output.join(format!("build/{name}-1"));
        for (list, contents) in lists {
            write(&stamp_dir.join(list), contents.as_bytes());
        }
        for stamp in [
            ".stamp_built",
            ".stamp_target_installed",
            ".stamp_installed",
        ] {
            write(&stamp_dir.join(stamp), b"");
        }
    }
    std::os::unix::fs::symlink(output.join("host/sysroot"), output.join("staging"))
        .expect("staging link");
}

pub(super) fn cache_graph() -> PackageGraph {
    let mut graph = PackageGraph::default();
    for (name, dependencies, reverse) in
        [("base", vec![], vec!["app"]), ("app", vec!["base"], vec![])]
    {
        graph.packages.insert(
            name.to_string(),
            PackageInfo {
                kind: "target".to_string(),
                stamp_dir: Some(format!("build/{name}-1")),
                dependencies: dependencies.into_iter().map(str::to_string).collect(),
                reverse_dependencies: reverse.into_iter().map(str::to_string).collect(),
                ..PackageInfo::default()
            },
        );
    }
    graph
}

/// A two-level cache under `root` (`system/`, `project/`) storing the
/// `project_packages` at the project level.
pub(super) fn test_cache(root: &Path, project_packages: &[&str]) -> PackageCache {
    PackageCache {
        system: Some(root.join("system")),
        project: root.join("project"),
        max_size: Some(u64::MAX),
        default_budgets: Vec::new(),
        policy: gaia_spec::BuildrootPackageCachePolicySpec {
            enabled: true,
            project_packages: project_packages
                .iter()
                .map(|name| name.to_string())
                .collect(),
            ..gaia_spec::BuildrootPackageCachePolicySpec::default()
        },
        note: None,
        registry: None,
        registry_warnings: Vec::new(),
    }
}

pub(super) fn tools_available() -> bool {
    ["cp", "rsync"].iter().all(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

#[test]
fn packages_round_trip_into_another_tree() {
    if !tools_available() {
        return;
    }
    let root_scratch = TempDir::new("round-trip");
    let root = root_scratch.join("root");
    let cache = test_cache(&root, &[]);
    let first = root.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, skipped) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"], "{skipped:?}");

    // Another tree, at another path, with nothing built.
    let second = root.join("elsewhere/second");
    fs::create_dir_all(&second).expect("second");
    assert_eq!(cache.restore(&second, &graph, &keys), ["app", "base"]);
    let app = second.join("per-package/app");
    assert_eq!(
        fs::read(app.join("target/usr/bin/app")).expect("app"),
        b"app binary"
    );
    // Its dependency's files, as Buildroot's per-package preparation
    // would have copied them.
    assert_eq!(
        fs::read(app.join("target/usr/lib/libbase.so")).expect("dep"),
        b"base"
    );
    assert!(app.join("target/var/lib/app").is_dir());
    assert_eq!(
        fs::read_to_string(app.join("host/sysroot/usr/lib/pkgconfig/app.pc")).expect("pc"),
        format!("prefix={}/per-package/app/host\n", second.display())
    );
    assert_eq!(
        fs::read(second.join("images/app.img")).expect("image"),
        b"image"
    );
    assert_eq!(fs::read(app.join("host/opt/tool")).expect("tool"), b"tool");
    // The dependency's file came from the dependency, not app's archive.
    let archive = entry_in(&root.join("system"), "app", "k-app", None).0;
    assert!(
        !archive
            .join("per-package/app/target/usr/lib/libbase.so")
            .exists()
    );
    assert!(archive.join("per-package/app/host/opt/tool").is_file());
    // Its empty directory is kept.
    assert!(archive.join("per-package/app/target/var/lib/app").is_dir());
    let stamps = second.join("build/app-1");
    let modified = |stamp: &str| {
        fs::metadata(stamps.join(stamp))
            .and_then(|metadata| metadata.modified())
            .expect("stamp")
    };
    assert!(modified(".stamp_built") < modified(".stamp_target_installed"));
    assert!(modified(".stamp_target_installed") < modified(".stamp_installed"));
    assert!(stamps.join(".files-list.txt").is_file());

    // Already built: nothing to restore.
    assert!(cache.restore(&second, &graph, &keys).is_empty());
}

#[test]
fn packages_with_the_path_in_binaries_restore_only_at_that_path() {
    if !tools_available() {
        return;
    }
    let root_scratch = TempDir::new("pinned");
    let root = root_scratch.join("root");
    let cache = test_cache(&root, &[]);
    let first = root.join("first");
    let mut binary = b"\0ELF ".to_vec();
    binary.extend_from_slice(first.display().to_string().as_bytes());
    built_tree(&first, &binary);
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    cache.store(&first, &graph, &keys, &[]);

    let second = root.join("second");
    fs::create_dir_all(&second).expect("second");
    assert_eq!(cache.restore(&second, &graph, &keys), ["base"]);

    // The same build after a wipe restores both.
    fs::remove_dir_all(&first).expect("wipe");
    fs::create_dir_all(&first).expect("first");
    assert_eq!(cache.restore(&first, &graph, &keys), ["app", "base"]);
    assert_eq!(
        fs::read(first.join("per-package/app/target/usr/bin/app")).expect("app"),
        binary
    );
}

/// A pinned package (its binary holds the tree path) built in one checkout
/// restores into another checkout of the same build when both use the
/// canonical tree path, and not when each has its own per-checkout path.
#[test]
fn a_canonical_tree_path_restores_pinned_packages_into_another_checkout() {
    use crate::canonical_tree::{CanonicalRequest, canonical_tree_dir};
    use crate::ram_tree::tree_dir;
    if !tools_available() {
        return;
    }
    let root_scratch = TempDir::new("canonical-pinned");
    let root = root_scratch.join("root");
    let cache = test_cache(&root, &[]);
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let ram = root.join("ram");
    let identity = CanonicalRequest {
        build_id: "img".to_string(),
        identity: "defconfig=a\n".to_string(),
    };
    let canonical = canonical_tree_dir(&ram, &identity);
    let mut binary = b"\0ELF ".to_vec();
    binary.extend_from_slice(canonical.display().to_string().as_bytes());
    built_tree(&canonical, &binary);
    let (stored, skipped) = cache.store(&canonical, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"], "{skipped:?}");

    // The next checkout of the build: the same canonical path, after a wipe.
    fs::remove_dir_all(&canonical).expect("wipe");
    fs::create_dir_all(&canonical).expect("canonical");
    assert_eq!(cache.restore(&canonical, &graph, &keys), ["app", "base"]);
    assert_eq!(
        fs::read(canonical.join("per-package/app/target/usr/bin/app")).expect("app"),
        binary
    );

    // Without the canonical path, each checkout has its own: the pinned app
    // does not restore into the other one.
    let first_checkout = tree_dir(&ram, &root.join("one/build/image/buildroot-output"))
        .expect("first checkout tree");
    let second_checkout = tree_dir(&ram, &root.join("two/build/image/buildroot-output"))
        .expect("second checkout tree");
    let mut own_binary = b"\0ELF ".to_vec();
    own_binary.extend_from_slice(first_checkout.display().to_string().as_bytes());
    built_tree(&first_checkout, &own_binary);
    cache.store(&first_checkout, &graph, &keys, &[]);
    fs::create_dir_all(&second_checkout).expect("second checkout");
    assert_eq!(
        cache.restore(&second_checkout, &graph, &keys),
        ["base"],
        "a pinned package restores only at its own path"
    );
}

#[test]
fn output_paths_are_rewritten_everywhere() {
    assert_eq!(
        replace_bytes(
            b"prefix=/a/out/host\nlib=/a/out/host/lib\n",
            b"/a/out",
            b"/b/o"
        ),
        b"prefix=/b/o/host\nlib=/b/o/host/lib\n"
    );
    assert_eq!(find_bytes(b"abc", b""), None);
}

#[test]
fn caches_larger_than_their_free_space_are_reported() {
    let dir_scratch = TempDir::new("space");
    let dir = dir_scratch.join("dir");
    fs::create_dir_all(&dir).expect("dir");
    let warning = cache_space_warning("package cache", &dir, u64::MAX).expect("warning");
    assert!(warning.starts_with(CACHE_SPACE_WARNING_PREFIX), "{warning}");
    assert!(warning.contains("GiB free"), "{warning}");
    assert_eq!(cache_space_warning("package cache", &dir, 1), None);
}

#[test]
fn stamps_of_unchanged_packages_are_made_newer_than_their_inputs() {
    let root_scratch = TempDir::new("refresh");
    let root = root_scratch.join("root");
    let output = root.join("out");
    let stamps = output.join("build/app-1");
    fs::create_dir_all(&stamps).expect("stamps");
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    for (offset, stamp) in [
        (0, ".stamp_dotconfig"),
        (1, ".stamp_configured"),
        (2, ".stamp_installed"),
    ] {
        let file = fs::File::create(stamps.join(stamp)).expect("stamp");
        file.set_modified(old + Duration::from_secs(offset))
            .expect("mtime");
    }
    let graph = cache_graph();
    let mut keys = BTreeMap::from([("app".to_string(), Some("k-app".to_string()))]);
    let modified = |stamp: &str| {
        fs::metadata(stamps.join(stamp))
            .and_then(|metadata| metadata.modified())
            .expect("mtime")
    };

    // No recorded key: left alone.
    refresh_current_stamps(&output, &graph, &keys);
    assert!(modified(".stamp_installed") < std::time::SystemTime::now() - Duration::from_secs(60));

    record_package_key(&stamps, "k-app");
    let before = std::time::SystemTime::now() - Duration::from_secs(1);
    refresh_current_stamps(&output, &graph, &keys);
    assert!(modified(".stamp_dotconfig") > before);
    assert!(modified(".stamp_dotconfig") < modified(".stamp_configured"));
    assert!(modified(".stamp_configured") < modified(".stamp_installed"));

    // A changed key (changed inputs): left for make to rebuild.
    keys.insert("app".to_string(), Some("k-app-2".to_string()));
    let refreshed = modified(".stamp_installed");
    refresh_current_stamps(&output, &graph, &keys);
    assert_eq!(modified(".stamp_installed"), refreshed);
}

#[test]
fn packages_are_stored_at_their_level_and_restored_from_either() {
    if !tools_available() {
        return;
    }
    let root_scratch = TempDir::new("levels");
    let root = root_scratch.join("root");
    let cache = test_cache(&root, &["app"]);
    let first = root.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, _) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"]);
    // The project's own package stays at the project level.
    assert!(root.join("project/app/k-app.json").is_file());
    assert!(!root.join("system/app").exists());
    assert!(root.join("system/base/k-base.json").is_file());

    let second = root.join("second");
    fs::create_dir_all(&second).expect("second");
    assert_eq!(cache.restore(&second, &graph, &keys), ["app", "base"]);
}

/// Regular files under `output/per-package` and `output/images`, with their
/// contents, keyed by path relative to `output`.
fn tree_contents(output: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for relative in ["per-package", "images"] {
        walk_files(output, relative, &mut |member, metadata| {
            if metadata.is_file() {
                let bytes = fs::read(output.join(&member)).expect("read");
                files.insert(member, bytes);
            }
        });
    }
    files
}

/// Removes the directories a cross-filesystem test created, when the test ends
/// (including by a failed assertion).
struct RemoveOnDrop(Vec<PathBuf>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[test]
fn packages_round_trip_across_filesystems() {
    if !tools_available() {
        return;
    }
    // Trees on tmpfs; the cache under $HOME, which is on disk.
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() {
        return;
    }
    let trees = shm.join(format!("gaia-package-cache-trees-{}", std::process::id()));
    if fs::create_dir_all(&trees).is_err() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        let _ = fs::remove_dir_all(&trees);
        return;
    };
    let caches = PathBuf::from(home)
        .join(".cache")
        .join(format!("gaia-test-{}", std::process::id()));
    let _cleanup = RemoveOnDrop(vec![trees.clone(), caches.clone()]);
    fs::create_dir_all(&caches).expect("caches");
    let same_filesystem = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&trees).expect("trees").dev() == fs::metadata(&caches).expect("caches").dev()
    };
    if same_filesystem {
        // The test would not exercise a cross-filesystem store and restore.
        return;
    }

    let cache = test_cache(&caches, &[]);
    let first = trees.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, skipped) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"], "{skipped:?}");

    let second = trees.join("second");
    fs::create_dir_all(&second).expect("second");
    assert_eq!(cache.restore(&second, &graph, &keys), ["app", "base"]);

    // Same files with the same contents; paths embedded in the files are
    // rewritten to the restore target.
    let (first_text, second_text) = (first.display().to_string(), second.display().to_string());
    let expected: BTreeMap<String, Vec<u8>> = tree_contents(&first)
        .into_iter()
        .map(|(member, bytes)| {
            let bytes = replace_bytes(&bytes, first_text.as_bytes(), second_text.as_bytes());
            (member, bytes)
        })
        .collect();
    assert_eq!(
        tree_contents(&second).keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    assert_eq!(tree_contents(&second), expected);

    let app = second.join("per-package/app");
    assert_eq!(
        fs::read(app.join("target/usr/bin/app")).expect("app"),
        b"app binary"
    );
    assert_eq!(
        fs::read(app.join("target/usr/lib/libbase.so")).expect("dep"),
        b"base"
    );
    assert!(app.join("target/var/lib/app").is_dir());
    assert_eq!(
        fs::read_to_string(app.join("host/sysroot/usr/lib/pkgconfig/app.pc")).expect("pc"),
        format!("prefix={second_text}/per-package/app/host\n")
    );
    assert_eq!(fs::read(app.join("host/opt/tool")).expect("tool"), b"tool");
    assert_eq!(
        fs::read(second.join("images/app.img")).expect("image"),
        b"image"
    );

    // Stamps and file lists recreated, in build order.
    let stamps = second.join("build/app-1");
    let modified = |stamp: &str| {
        fs::metadata(stamps.join(stamp))
            .and_then(|metadata| metadata.modified())
            .expect("stamp")
    };
    assert!(modified(".stamp_built") < modified(".stamp_target_installed"));
    assert!(modified(".stamp_target_installed") < modified(".stamp_installed"));
    assert!(stamps.join(".files-list.txt").is_file());
    assert!(second.join("build/base-1/.stamp_installed").is_file());

    // Already built: nothing to restore.
    assert!(cache.restore(&second, &graph, &keys).is_empty());
}

#[path = "package_cache_link_tests.rs"]
mod link_tests;
