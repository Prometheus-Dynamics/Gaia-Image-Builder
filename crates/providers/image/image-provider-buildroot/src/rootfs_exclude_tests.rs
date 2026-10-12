//! Tests for leaving packages out of the rootfs: the generated script runs
//! with `sh` on a fake output tree, so the removal is what Buildroot would run.
use super::*;
use crate::rootfs_exclude_elf::tests::elf64_object;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// A fresh output dir with `build/` and `target/`, removed when dropped.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "gaia-rootfs-exclude-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("build")).expect("build dir");
    fs::create_dir_all(root.join("target")).expect("target dir");
    Scratch(root)
}

fn put(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("parent dirs");
    fs::write(path, contents).expect("write");
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn stage(root: &Path, excluded: &[&str]) -> StagedRootfsExclude {
    stage_rootfs_exclude(root, &names(excluded))
        .expect("stage")
        .expect("an exclusion is staged")
}

/// Runs the staged post-build script on the output's target, as Buildroot does.
fn run_script(staged: &StagedRootfsExclude, root: &Path) {
    let status = Command::new("sh")
        .arg(&staged.script)
        .arg(root.join("target"))
        .status()
        .expect("sh runs the script");
    assert!(status.success(), "the post-build script failed");
}

fn graph(json: &str) -> PackageGraph {
    PackageGraph::parse(json).expect("a package graph")
}

#[test]
fn removes_excluded_files_keeps_shared_ones_and_prunes_emptied_directories() {
    let root = scratch("remove");
    put(
        &root,
        "build/packages-file-list.txt",
        b"libcamera,./usr/lib/libcamera.so.0.7\n\
          libcamera,./usr/share/libcamera/ipa/a.json\n\
          libcamera,./usr/lib/shared.txt\n\
          kept,./usr/lib/shared.txt\n\
          kept,./etc/kept.conf\n\
          libpisp,./usr/include/libpisp/x.hpp\n",
    );
    for rel in [
        "usr/lib/libcamera.so.0.7",
        "usr/share/libcamera/ipa/a.json",
        "usr/lib/shared.txt",
        "etc/kept.conf",
        "usr/include/libpisp/x.hpp",
    ] {
        put(&root, &format!("target/{rel}"), b"data");
    }

    let staged = stage(&root, &["libcamera", "libpisp"]);
    run_script(&staged, &root);
    assert!(staged.applied(), "the script leaves its marker");
    let summary = staged.verify().expect("the removal verifies");

    assert_eq!(summary.files, 3);
    assert_eq!(summary.shared, vec!["usr/lib/shared.txt".to_string()]);
    assert_eq!(
        summary.message(),
        "rootfs: left out 2 packages (libcamera, libpisp): 3 files, 0.0 MiB"
    );
    let target = root.join("target");
    assert!(!target.join("usr/lib/libcamera.so.0.7").exists());
    assert!(!target.join("usr/include").exists(), "emptied dirs go");
    assert!(!target.join("usr/share").exists(), "emptied dirs go");
    assert!(target.join("usr/lib/shared.txt").is_file(), "shared stays");
    assert!(target.join("etc/kept.conf").is_file(), "kept stays");
    assert!(
        target.join("usr/lib").is_dir(),
        "a dir with kept files stays"
    );
}

#[test]
fn the_graph_check_names_a_kept_package_that_depends_on_an_excluded_one() {
    let json = r#"{"app":{"type":"target","dependencies":["libcamera","skeleton"],"reverse_dependencies":[]},"libcamera":{"type":"target","dependencies":["libpisp","skeleton","host-cmake"],"reverse_dependencies":["app"]},"libpisp":{"type":"target","dependencies":["skeleton"],"reverse_dependencies":["libcamera"]},"host-cmake":{"type":"host","dependencies":[],"reverse_dependencies":["libcamera"]},"skeleton":{"type":"target","dependencies":[],"reverse_dependencies":["app","libcamera","libpisp"]},"rootfs-tar":{"type":"rootfs","dependencies":["skeleton"]}}"#;
    let graph = graph(json);

    // Leaving out libpisp: app needs libcamera, which needs libpisp.
    let error = check_rootfs_exclude_graph(&graph, &names(&["libpisp"]))
        .expect_err("a kept package needs libpisp");
    assert!(error.message.contains("'app'"), "{}", error.message);
    assert!(error.message.contains("'libpisp'"), "{}", error.message);
    assert!(
        error.message.contains("app -> libcamera -> libpisp"),
        "{}",
        error.message
    );

    // Leaving out libcamera alone: app depends on it directly.
    let error = check_rootfs_exclude_graph(&graph, &names(&["libcamera", "libpisp"]))
        .expect_err("app needs libcamera");
    assert!(
        error.message.contains("app -> libcamera"),
        "{}",
        error.message
    );

    // Everything that needs them is left out too: nothing kept needs them.
    check_rootfs_exclude_graph(&graph, &names(&["app", "libcamera", "libpisp"]))
        .expect("no kept package needs an excluded one");
}

#[test]
fn the_graph_check_refuses_unknown_and_host_packages() {
    let json = r#"{"libcamera":{"type":"target","dependencies":[],"reverse_dependencies":[]},"host-cmake":{"type":"host","dependencies":[],"reverse_dependencies":[]}}"#;
    let graph = graph(json);
    let unknown = check_rootfs_exclude_graph(&graph, &names(&["opencv4"])).expect_err("unknown");
    assert!(
        unknown.message.contains("'opencv4' is not a package"),
        "{}",
        unknown.message
    );
    let host = check_rootfs_exclude_graph(&graph, &names(&["host-cmake"])).expect_err("host");
    assert!(host.message.contains("host package"), "{}", host.message);
}

#[test]
fn a_removed_library_still_needed_by_a_kept_binary_is_an_error() {
    let root = scratch("needed");
    put(
        &root,
        "build/packages-file-list.txt",
        b"libcamera,./usr/lib/libcamera.so.0.7\nkept,./usr/bin/app\n",
    );
    let library = elf64_object(&[], Some("libcamera.so.0.7"));
    put(&root, "target/usr/lib/libcamera.so.0.7", &library);
    put(
        &root,
        "per-package/libcamera/target/usr/lib/libcamera.so.0.7",
        &library,
    );
    put(
        &root,
        "target/usr/bin/app",
        &elf64_object(&["libc.so.6", "libcamera.so.0.7"], None),
    );

    let staged = stage(&root, &["libcamera"]);
    run_script(&staged, &root);
    let error = staged
        .verify()
        .expect_err("app needs a library that is left out");
    assert!(error.message.contains("usr/bin/app"), "{}", error.message);
    assert!(
        error.message.contains("'libcamera.so.0.7'"),
        "{}",
        error.message
    );
    assert!(error.message.contains("'libcamera'"), "{}", error.message);
}

#[test]
fn a_kept_object_providing_the_needed_name_passes_the_elf_check() {
    let root = scratch("provided");
    put(
        &root,
        "build/packages-file-list.txt",
        b"libcamera,./usr/lib/libcamera.so.0.7\nkept,./usr/bin/app\nkept,./usr/lib/compat/libcamera.so.0.7\n",
    );
    let library = elf64_object(&[], Some("libcamera.so.0.7"));
    put(&root, "target/usr/lib/libcamera.so.0.7", &library);
    put(
        &root,
        "per-package/libcamera/target/usr/lib/libcamera.so.0.7",
        &library,
    );
    put(
        &root,
        "target/usr/bin/app",
        &elf64_object(&["libcamera.so.0.7"], None),
    );
    // A kept compatibility library with the same soname.
    put(
        &root,
        "target/usr/lib/compat/libcamera.so.0.7",
        &elf64_object(&[], Some("libcamera.so.0.7")),
    );

    let staged = stage(&root, &["libcamera"]);
    run_script(&staged, &root);
    let summary = staged.verify().expect("the kept library provides the name");
    assert_eq!(summary.files, 1);
}

#[test]
fn a_script_that_did_not_run_is_an_error() {
    let root = scratch("not-run");
    put(
        &root,
        "build/packages-file-list.txt",
        b"libcamera,./usr/lib/x.so\n",
    );
    put(&root, "target/usr/lib/x.so", b"data");
    let staged = stage(&root, &["libcamera"]);
    let error = staged.verify().expect_err("the script never ran");
    assert!(error.message.contains("did not run"), "{}", error.message);
    assert!(
        root.join("target/usr/lib/x.so").is_file(),
        "nothing removed"
    );
}

#[test]
fn the_digest_is_empty_for_no_exclusion_and_follows_the_list() {
    assert_eq!(rootfs_exclude_digest(&[]), "");
    let one = rootfs_exclude_digest(&names(&["libcamera"]));
    assert!(!one.is_empty());
    assert_ne!(one, rootfs_exclude_digest(&names(&["libpisp"])));
    assert_ne!(
        rootfs_exclude_digest(&names(&["libcamera", "libpisp"])),
        one
    );
}

/// A read-only dry run against a real Buildroot output tree, with nothing
/// written or removed:
/// `GAIA_ROOTFS_DRY_TREE=<output dir> [GAIA_ROOTFS_DRY_EXCLUDE=a,b] cargo test
/// -p gaia-image-provider-buildroot dry_analysis -- --ignored --nocapture`.
#[test]
#[ignore = "reads a real Buildroot output tree; set GAIA_ROOTFS_DRY_TREE"]
fn dry_analysis_of_a_real_tree() {
    let Ok(tree) = env::var("GAIA_ROOTFS_DRY_TREE") else {
        return;
    };
    let output = PathBuf::from(tree);
    let excluded = env::var("GAIA_ROOTFS_DRY_EXCLUDE")
        .unwrap_or_else(|_| "libcamera,libpisp".into())
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    let graph_text = fs::read_to_string(output.join(".gaia-buildroot-packages.json"))
        .expect("the recorded package graph");
    let graph = PackageGraph::parse(&graph_text).expect("a package graph");
    println!("excluding: {}", excluded.join(", "));
    match check_rootfs_exclude_graph(&graph, &excluded) {
        Ok(()) => println!("graph check: ok"),
        Err(error) => println!("graph check: {}", error.message),
    }

    // Every kept target package that reaches an excluded one, not just the first.
    let excluded_set: BTreeSet<&str> = excluded.iter().map(String::as_str).collect();
    for (name, package) in &graph.packages {
        if package.kind == "target"
            && !excluded_set.contains(name.as_str())
            && let Some(path) = dependency_path_to(&graph, name, &excluded_set)
        {
            println!(
                "kept package reaches an excluded one: {}",
                path.join(" -> ")
            );
        }
    }

    // The removal the script would make: what the file list says, without
    // touching the tree.
    let list = fs::read_to_string(output.join("build/packages-file-list.txt"))
        .expect("the package file list");
    let mut kept_paths = BTreeSet::new();
    let mut lines = Vec::new();
    for line in list.lines() {
        let Some((pkg, path)) = line.split_once(',') else {
            continue;
        };
        let path = path.trim_start_matches("./").to_string();
        if excluded_set.contains(pkg) {
            lines.push((pkg.to_string(), path));
        } else {
            kept_paths.insert(path);
        }
    }
    let (shared, removed): (Vec<_>, Vec<_>) = lines
        .into_iter()
        .partition(|(_, path)| kept_paths.contains(path));
    let mut bytes = 0u64;
    let target = output.join("target");
    for (pkg, path) in &removed {
        let file = output
            .join("per-package")
            .join(pkg)
            .join("target")
            .join(path);
        if let Ok(metadata) = fs::symlink_metadata(&file) {
            bytes += metadata.len();
        }
    }
    println!(
        "would remove {} files ({:.1} MiB); kept because a kept package lists them: {}",
        removed.len(),
        bytes as f64 / (1024.0 * 1024.0),
        shared.len()
    );

    // Names the removed files provide, and what the remaining objects need.
    let mut providers: BTreeMap<String, String> = BTreeMap::new();
    for (pkg, path) in &removed {
        if let Some(name) = Path::new(path).file_name() {
            providers
                .entry(name.to_string_lossy().into_owned())
                .or_insert_with(|| pkg.clone());
        }
        let file = output
            .join("per-package")
            .join(pkg)
            .join("target")
            .join(path);
        if let Ok(bytes) = fs::read(&file)
            && let Some(ElfNames {
                soname: Some(soname),
                ..
            }) = elf_names(&bytes)
        {
            providers.entry(soname).or_insert_with(|| pkg.clone());
        }
    }
    // The target as the removal leaves it: the removed paths are gone.
    let removed_paths: BTreeSet<&str> = removed.iter().map(|(_, path)| path.as_str()).collect();
    let mut entries = Vec::new();
    collect_entries(&target, &mut entries).expect("the target tree");
    entries.retain(|path| {
        path.strip_prefix(&target)
            .is_ok_and(|rel| !removed_paths.contains(rel.to_string_lossy().as_ref()))
    });
    let mut kept_names = BTreeSet::new();
    let mut objects = Vec::new();
    for path in &entries {
        if let Some(name) = path.file_name() {
            kept_names.insert(name.to_string_lossy().into_owned());
        }
        if let Some(names) = read_elf_names(path) {
            if let Some(soname) = &names.soname {
                kept_names.insert(soname.clone());
            }
            objects.push((
                path.strip_prefix(&target)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
                names,
            ));
        }
    }
    println!("kept ELF objects: {}", objects.len());
    for (rel, names) in &objects {
        for needed in &names.needed {
            if let Some(pkg) = providers.get(needed)
                && !kept_names.contains(needed)
            {
                println!("  /{rel} needs {needed} (only {pkg} provides it)");
            }
        }
    }
}
