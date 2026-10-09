use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::UNIX_EPOCH;

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

/// A two-level cache under `root` (`system/`, `project/`) storing the
/// `project_packages` at the project level.
fn test_cache(root: &Path, project_packages: &[&str]) -> PackageCache {
    PackageCache {
        system: Some(root.join("system")),
        project: root.join("project"),
        max_size: u64::MAX,
        policy: gaia_spec::BuildrootPackageCachePolicySpec {
            enabled: true,
            project_packages: project_packages
                .iter()
                .map(|name| name.to_string())
                .collect(),
            ..gaia_spec::BuildrootPackageCachePolicySpec::default()
        },
        note: None,
    }
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
    let _ = fs::remove_dir_all(root);
}

#[test]
fn packages_with_the_path_in_binaries_restore_only_at_that_path() {
    if !tools_available() {
        return;
    }
    let root = temp("pinned");
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

#[test]
fn packages_are_stored_at_their_level_and_restored_from_either() {
    if !tools_available() {
        return;
    }
    let root = temp("levels");
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
    let _ = fs::remove_dir_all(root);
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
    let _ = fs::remove_dir_all(&trees);
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
    let _ = fs::remove_dir_all(&caches);
    fs::create_dir_all(&caches).expect("caches");
    let same_filesystem = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&trees).expect("trees").dev() == fs::metadata(&caches).expect("caches").dev()
    };
    if same_filesystem {
        // The test would not exercise a cross-filesystem store and restore.
        let _ = fs::remove_dir_all(&trees);
        let _ = fs::remove_dir_all(&caches);
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

    let _ = fs::remove_dir_all(&trees);
    let _ = fs::remove_dir_all(&caches);
}

/// The link step as it was: one `rsync -a --link-dest` per source.
fn link_with_rsync(destination: &Path, sources: &[PathBuf]) -> Result<(), String> {
    for source in sources {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        let status = Command::new("rsync")
            .arg("-a")
            .arg(format!("--link-dest={}/", source.display()))
            .arg(format!("{}/", source.display()))
            .arg(format!("{}/", destination.display()))
            .status()
            .map_err(|error| format!("rsync: {error}"))?;
        if !status.success() {
            return Err(format!("rsync exited with {status}"));
        }
    }
    Ok(())
}

/// Every entry under `root` (the root itself as "") with its metadata.
fn walk_all(root: &Path, relative: &str, out: &mut Vec<(String, PathBuf, fs::Metadata)>) {
    let dir = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let metadata = fs::symlink_metadata(&dir).expect("metadata");
    out.push((relative.to_string(), dir.clone(), metadata));
    for entry in fs::read_dir(&dir).expect("read_dir").flatten() {
        let name = entry.file_name().into_string().expect("utf-8");
        let member = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if entry.file_type().expect("type").is_dir() {
            walk_all(root, &member, out);
        } else {
            let metadata = fs::symlink_metadata(entry.path()).expect("metadata");
            out.push((member, entry.path(), metadata));
        }
    }
}

/// What the link step decided in `destination`: every entry's kind, mode,
/// modification time, content or target, and the other entries (in
/// `destination` or in `sources`) it is a hard link of.
fn describe_links(destination: &Path, sources: &[PathBuf]) -> BTreeMap<String, String> {
    let mut roots = vec![("D".to_string(), destination.to_path_buf())];
    roots.extend(
        sources
            .iter()
            .enumerate()
            .map(|(index, source)| (format!("S{index}"), source.clone())),
    );
    let mut walked = Vec::new();
    for (label, root) in &roots {
        let mut entries = Vec::new();
        if root.is_dir() {
            walk_all(root, "", &mut entries);
        }
        walked.extend(
            entries
                .into_iter()
                .map(|(rel, path, metadata)| (label.clone(), rel, path, metadata)),
        );
    }
    let mut groups: BTreeMap<(u64, u64), Vec<String>> = BTreeMap::new();
    for (label, rel, _, metadata) in &walked {
        if metadata.is_file() {
            groups
                .entry((metadata.dev(), metadata.ino()))
                .or_default()
                .push(format!("{label}:{rel}"));
        }
    }
    let mut described = BTreeMap::new();
    for (label, rel, path, metadata) in &walked {
        if label != "D" {
            continue;
        }
        let mode = metadata.mode() & 0o7777;
        // Directories and symlinks: rsync and the link step set their times
        // differently (later entries touch a directory's time, and std cannot
        // set a symlink's), so only their kind, mode and target are compared.
        let description = if metadata.is_dir() {
            format!("dir mode {mode:o}")
        } else if metadata.file_type().is_symlink() {
            format!(
                "link mode {mode:o} -> {}",
                fs::read_link(path).expect("link").display()
            )
        } else {
            let own = format!("D:{rel}");
            let others = groups[&(metadata.dev(), metadata.ino())]
                .iter()
                .filter(|member| **member != own)
                .collect::<Vec<_>>();
            format!(
                "file mode {mode:o} time {}.{} {:?} links {others:?}",
                metadata.mtime(),
                metadata.mtime_nsec(),
                String::from_utf8_lossy(&fs::read(path).expect("read"))
            )
        };
        described.insert(rel.clone(), description);
    }
    described
}

/// Sets a file's or directory's modification time.
fn set_time(path: &Path, seconds: u64) {
    let time = UNIX_EPOCH + Duration::from_secs(seconds);
    fs::File::open(path)
        .expect("open")
        .set_modified(time)
        .expect("time");
}

#[test]
fn dependency_links_match_rsync() {
    if !tools_available() {
        return;
    }
    let root = temp("links");
    let a = root.join("a/target");
    let b = root.join("b/target");
    let c = root.join("c/target");
    write(&a.join("usr/lib/liba.so"), b"liba-1");
    write(&a.join("usr/bin/tool"), b"tool-one");
    write(&a.join("usr/share/doc/readme"), b"read me");
    fs::create_dir_all(a.join("usr/empty")).expect("dir");
    std::os::unix::fs::symlink("liba.so", a.join("usr/lib/liba.so.1")).expect("link");
    fs::hard_link(a.join("usr/lib/liba.so"), a.join("usr/lib/dup")).expect("hard link");
    fs::set_permissions(a.join("usr/bin/tool"), fs::Permissions::from_mode(0o755)).expect("mode");
    fs::set_permissions(a.join("usr/empty"), fs::Permissions::from_mode(0o700)).expect("mode");
    set_time(&a.join("usr/bin/tool"), 1_000_000);
    set_time(&a.join("usr/lib/liba.so"), 1_000_000);
    set_time(&a.join("usr/lib"), 1_100_000);

    // Same length and time as in `a`, other content: rsync keeps `a`'s file.
    write(&b.join("usr/lib/liba.so"), b"liba-2");
    write(&b.join("usr/bin/tool"), b"tool-two");
    fs::set_permissions(b.join("usr/bin/tool"), fs::Permissions::from_mode(0o755)).expect("mode");
    set_time(&b.join("usr/bin/tool"), 1_000_000);
    set_time(&b.join("usr/lib/liba.so"), 1_000_000);
    // Other length: rsync replaces it.
    write(&b.join("usr/share/doc/readme"), b"read me, longer");
    write(&b.join("usr/lib/libb.so"), b"libb");
    fs::create_dir_all(b.join("usr/extra")).expect("dir");
    write(&b.join("usr/extra/x"), b"x");
    set_time(&b.join("usr/lib"), 1_200_000);
    set_time(&b.join("usr/extra"), 1_300_000);
    set_time(&b.join("usr/share/doc/readme"), 1_400_000);

    // Hard links of files in `a`, a symlink to the same target again, and
    // a symlink through a directory.
    fs::create_dir_all(c.join("usr/lib")).expect("dir");
    fs::hard_link(a.join("usr/lib/liba.so"), c.join("usr/lib/liba.so")).expect("hard link");
    std::os::unix::fs::symlink("liba.so", c.join("usr/lib/liba.so.1")).expect("link");
    std::os::unix::fs::symlink("../lib/liba.so", c.join("usr/lib/alias")).expect("link");
    fs::create_dir_all(c.join("usr/share/doc")).expect("dir");
    fs::set_permissions(c.join("usr/share"), fs::Permissions::from_mode(0o750)).expect("mode");

    let sources = vec![a.clone(), b.clone(), c.clone()];
    let by_rsync = root.join("rsync/per-package/app/target");
    let by_links = root.join("links/per-package/app/target");
    link_with_rsync(&by_rsync, &sources).expect("rsync");
    TreeListings::default()
        .link(&by_links, &sources)
        .expect("links");
    let expected = describe_links(&by_rsync, &sources);
    assert!(expected.contains_key("usr/lib/liba.so"));
    assert_eq!(describe_links(&by_links, &sources), expected);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_directory_that_is_a_file_elsewhere_is_refused() {
    let root = temp("conflict");
    let a = root.join("a");
    let b = root.join("b");
    write(&a.join("usr/thing"), b"file");
    write(&b.join("usr/thing/inside"), b"dir");
    let result = TreeListings::default().link(&root.join("out"), &[a, b]);
    assert!(result.is_err());
    let _ = fs::remove_dir_all(root);
}

/// Times the link step of a synthetic 100-package build (fan-in through a
/// toolchain and host tools, each package depending on a few earlier ones),
/// against the rsync-per-dependency step it replaces. Run with:
/// `cargo test -p gaia-image-provider-buildroot package_cache -- --ignored --nocapture`.
#[test]
#[ignore]
fn dependency_link_benchmark() {
    if !tools_available() {
        return;
    }
    const PACKAGES: usize = 100;
    const FILES: usize = 150;
    let root = PathBuf::from("/dev/shm").join(format!("gaia-link-bench-{}", std::process::id()));
    let built = root.join("built");
    let mut random = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let mut direct: Vec<BTreeSet<usize>> = Vec::new();
    for index in 0..PACKAGES {
        let mut dependencies = BTreeSet::new();
        if index > 0 {
            dependencies.insert(0);
        }
        if index > 1 {
            dependencies.insert(1);
        }
        if index > 0 {
            for _ in 0..3 {
                dependencies.insert((next() as usize) % index);
            }
        }
        direct.push(dependencies);
    }
    let closure = |index: usize| {
        let mut all = BTreeSet::new();
        let mut queue = direct[index].iter().copied().collect::<Vec<_>>();
        while let Some(dependency) = queue.pop() {
            if all.insert(dependency) {
                queue.extend(direct[dependency].iter().copied());
            }
        }
        all
    };
    let name = |index: usize| format!("pkg{index:03}");
    let sources_of = |base: &Path, index: usize, tree: &str| {
        closure(index)
            .into_iter()
            .map(|dependency| base.join("per-package").join(name(dependency)).join(tree))
            .filter(|source| source.is_dir())
            .collect::<Vec<_>>()
    };

    // A build: each package links its dependencies, then adds its own files.
    let mut listings = TreeListings::default();
    for index in 0..PACKAGES {
        for tree in ["host", "target"] {
            let sources = sources_of(&built, index, tree);
            if !sources.is_empty() {
                listings
                    .link(
                        &built.join("per-package").join(name(index)).join(tree),
                        &sources,
                    )
                    .expect("build link");
            }
            for file in 0..FILES / 2 {
                write(
                    &built.join(format!(
                        "per-package/{}/{tree}/usr/lib/{}/f{file:04}",
                        name(index),
                        name(index)
                    )),
                    format!("{}-{file}", name(index)).as_bytes(),
                );
            }
        }
    }

    let by_links = root.join("links");
    let mut listings = TreeListings::default();
    let started = std::time::Instant::now();
    for index in 0..PACKAGES {
        for tree in ["host", "target"] {
            let sources = sources_of(&built, index, tree);
            if !sources.is_empty() {
                listings
                    .link(
                        &by_links.join("per-package").join(name(index)).join(tree),
                        &sources,
                    )
                    .expect("links");
            }
        }
    }
    let links = started.elapsed();

    let by_rsync = root.join("rsync");
    let started = std::time::Instant::now();
    for index in 0..PACKAGES {
        for tree in ["host", "target"] {
            let sources = sources_of(&built, index, tree);
            if !sources.is_empty() {
                link_with_rsync(
                    &by_rsync.join("per-package").join(name(index)).join(tree),
                    &sources,
                )
                .expect("rsync");
            }
        }
    }
    let rsync = started.elapsed();

    let count = |base: &Path| {
        let mut entries = Vec::new();
        walk_all(&base.join("per-package"), "", &mut entries);
        entries.len()
    };
    let (links_entries, rsync_entries) = (count(&by_links), count(&by_rsync));
    eprintln!(
        "link step for {PACKAGES} packages: in-process {links:?} ({links_entries} entries), rsync per dependency {rsync:?} ({rsync_entries} entries)"
    );
    assert_eq!(links_entries, rsync_entries);
    let _ = fs::remove_dir_all(root);
}
