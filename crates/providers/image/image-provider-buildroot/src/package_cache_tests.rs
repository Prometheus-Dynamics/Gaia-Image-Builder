use super::*;

#[test]
fn stamps_are_recorded_in_build_order() {
    let dir = temp("stamps");
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
    let _ = fs::remove_dir_all(dir);
}

fn temp(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "gaia-package-cache-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn write(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, contents).expect("file");
}

/// A tree where `base` (a dependency) and `app` were built: per-package
/// directories (app's holds a copy of base's files), file lists, stamps.
fn built_tree(output: &Path, app_binary: &[u8]) {
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

fn cache_graph() -> PackageGraph {
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

fn tools_available() -> bool {
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
    let root = temp("round-trip");
    let cache = PackageCache {
        dir: root.join("cache"),
        max_size: u64::MAX,
        note: None,
    };
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
    let archive = cache.entry("app", "k-app", None).0;
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
    let _ = fs::remove_dir_all(root);
}

#[test]
fn packages_with_the_path_in_binaries_restore_only_at_that_path() {
    if !tools_available() {
        return;
    }
    let root = temp("pinned");
    let cache = PackageCache {
        dir: root.join("cache"),
        max_size: u64::MAX,
        note: None,
    };
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
    let _ = fs::remove_dir_all(root);
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
    let dir = temp("space");
    fs::create_dir_all(&dir).expect("dir");
    let warning = cache_space_warning("package cache", &dir, u64::MAX).expect("warning");
    assert!(warning.starts_with(CACHE_SPACE_WARNING_PREFIX), "{warning}");
    assert!(warning.contains("GiB free"), "{warning}");
    assert_eq!(cache_space_warning("package cache", &dir, 1), None);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn stamps_of_unchanged_packages_are_made_newer_than_their_inputs() {
    let root = temp("refresh");
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
    let _ = fs::remove_dir_all(root);
}
