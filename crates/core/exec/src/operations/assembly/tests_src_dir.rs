//! `src_dir` file entries (a whole directory tree copied into a tree) and
//! the `src_glob` skip of directories. Split from `tests.rs`.

use super::tests::test_spec;
use super::tests_binaries::disk_env_panic;
use super::*;
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};

/// A `src_dir` entry copying `src` into `dest` of the `rootfs` tree.
fn dir_entry(src: &Path, dest: &str, exclude: &[&str], mode: Option<&str>) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        tree: "rootfs".into(),
        src: None,
        src_glob: None,
        src_dir: Some(src.display().to_string().into()),
        exclude: exclude.iter().map(|name| name.to_string()).collect(),
        content: None,
        json: None,
        dest: dest.into(),
        mode: mode.map(str::to_string),
        dir_mode: None,
        optional: false,
        preserve_symlink: false,
        allow_missing_libs: false,
    }
}

fn tree_assembly(tree: &Path, files: Vec<AssemblyFileSpec>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: tree.display().to_string().into(),
            strip: false,
        }],
        files,
        ..ImageAssemblySpec::default()
    }
}

/// A SvelteKit-style static build under `root/src/<name>`: nested, hashed
/// paths, a source map, and a relative symbolic link to `index.html`.
fn site(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("src").join(name);
    fs::create_dir_all(dir.join("_app/immutable/chunks")).expect("site dirs");
    fs::write(dir.join("index.html"), "<html></html>\n").expect("index");
    fs::write(dir.join("_app/immutable/chunks/abc123.js"), "export {};\n").expect("chunk");
    fs::write(dir.join("_app/immutable/chunks/def456.js.map"), "{}\n").expect("map");
    #[cfg(unix)]
    std::os::unix::fs::symlink("index.html", dir.join("home.html")).expect("link");
    dir
}

/// Runs one `src_dir` entry over `dir` and returns the tree and the outcome.
fn copy_dir(
    scratch: &TempDir,
    entry: AssemblyFileSpec,
) -> (PathBuf, Result<AssemblyStagingSummary, String>) {
    let root = scratch.join("root");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(tree_assembly(&tree, vec![entry]));
    let result =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .map_err(|error| error.message);
    (tree, result)
}

#[cfg(unix)]
#[test]
fn nested_tree_is_copied_under_dest_with_its_structure() {
    let scratch = TempDir::new("gaia-exec-src-dir-nested");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let dir = site(&root, "site");
    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, ".", &[], None));
    let summary = result.expect("nested tree copies");

    assert!(tree.join("index.html").is_file());
    assert!(tree.join("_app/immutable/chunks/abc123.js").is_file());
    assert!(tree.join("_app/immutable/chunks/def456.js.map").is_file());
    assert!(
        !tree.join("site").exists(),
        "the directory itself is not copied"
    );
    assert_eq!(
        fs::read_to_string(tree.join("_app/immutable/chunks/abc123.js")).expect("chunk"),
        "export {};\n"
    );
    assert_eq!(state_count(&summary, "staged_file_count"), 3);
}

#[cfg(unix)]
#[test]
fn dest_forms_all_copy_the_contents_under_the_directory() {
    // `.` is the tree root; `web/` and a bare `web` both name a directory.
    for (name, dest, prefix) in [
        ("dot", ".", ""),
        ("slash", "web/", "web/"),
        ("bare", "web", "web/"),
    ] {
        let scratch = TempDir::new(&format!("gaia-exec-src-dir-dest-{name}"));
        let root = scratch.join("root");
        fs::create_dir_all(&root).expect("root");
        let dir = site(&root, "site");
        let (tree, result) = copy_dir(&scratch, dir_entry(&dir, dest, &[], None));
        result.unwrap_or_else(|error| panic!("dest {dest}: {error}"));
        assert!(
            tree.join(format!("{prefix}index.html")).is_file(),
            "dest {dest}"
        );
        assert!(
            tree.join(format!("{prefix}_app/immutable/chunks/abc123.js"))
                .is_file(),
            "dest {dest}"
        );
    }
}

#[cfg(unix)]
#[test]
fn regular_files_keep_their_mode_unless_mode_is_set() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-exec-src-dir-modes");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let dir = site(&root, "site");
    fs::set_permissions(
        dir.join("_app/immutable/chunks/abc123.js"),
        fs::Permissions::from_mode(0o751),
    )
    .expect("source mode");
    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, ".", &[], None));
    result.expect("kept modes");
    let mode = |path: &Path| fs::metadata(path).expect("copy").permissions().mode() & 0o777;
    assert_eq!(mode(&tree.join("_app/immutable/chunks/abc123.js")), 0o751);

    let scratch = TempDir::new("gaia-exec-src-dir-mode-set");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let dir = site(&root, "site");
    fs::set_permissions(
        dir.join("_app/immutable/chunks/abc123.js"),
        fs::Permissions::from_mode(0o751),
    )
    .expect("source mode");
    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, ".", &[], Some("0644")));
    result.expect("mode set");
    assert_eq!(mode(&tree.join("_app/immutable/chunks/abc123.js")), 0o644);
    assert_eq!(mode(&tree.join("index.html")), 0o644);
}

#[cfg(unix)]
#[test]
fn directories_keep_their_mode_unless_dir_mode_is_set() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| fs::metadata(path).expect("metadata").permissions().mode() & 0o7777;
    let scratch = TempDir::new("gaia-exec-src-dir-dir-mode");
    let root = scratch.join("root");
    let dir = root.join("src/conf");
    fs::create_dir_all(dir.join("private/keys")).expect("dirs");
    fs::write(dir.join("private/keys/id"), "key\n").expect("file");
    // A read-only directory with contents: its mode must go on after them.
    fs::set_permissions(dir.join("private/keys"), fs::Permissions::from_mode(0o555))
        .expect("keys mode");
    fs::set_permissions(dir.join("private"), fs::Permissions::from_mode(0o750))
        .expect("private mode");

    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, "etc/conf/", &[], None));
    result.expect("the tree copies");
    assert_eq!(mode(&tree.join("etc/conf/private")), 0o750);
    assert_eq!(mode(&tree.join("etc/conf/private/keys")), 0o555);
    assert!(tree.join("etc/conf/private/keys/id").is_file());
    // Leave the scratch removable.
    fs::set_permissions(
        tree.join("etc/conf/private/keys"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("restore");

    let scratch = TempDir::new("gaia-exec-src-dir-dir-mode-set");
    let mut entry = dir_entry(&dir, "etc/conf/", &[], None);
    entry.dir_mode = Some("0700".into());
    let (tree, result) = copy_dir(&scratch, entry);
    let summary = result.expect("the tree copies");
    assert_eq!(mode(&tree.join("etc/conf/private")), 0o700);
    assert_eq!(mode(&tree.join("etc/conf/private/keys")), 0o700);
    let state = summary.state.render();
    assert!(state.contains(".dir_mode=0700"), "{state}");
    fs::set_permissions(dir.join("private/keys"), fs::Permissions::from_mode(0o755))
        .expect("restore");
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_kept_as_links_and_not_followed() {
    let scratch = TempDir::new("gaia-exec-src-dir-links");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let dir = site(&root, "site");
    // Dangling on purpose: a followed link would fail the copy.
    std::os::unix::fs::symlink("/nonexistent/elsewhere", dir.join("dangling")).expect("link");
    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, ".", &[], None));
    result.expect("links copy");

    let link = tree.join("home.html");
    assert!(
        fs::symlink_metadata(&link)
            .expect("link metadata")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&link).expect("target"),
        Path::new("index.html")
    );
    assert_eq!(
        fs::read_link(tree.join("dangling")).expect("target"),
        Path::new("/nonexistent/elsewhere")
    );
}

#[cfg(unix)]
#[test]
fn exclude_drops_names_at_any_depth_and_skips_excluded_directories() {
    let scratch = TempDir::new("gaia-exec-src-dir-exclude");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let dir = site(&root, "site");
    fs::create_dir_all(dir.join("private")).expect("private");
    fs::write(dir.join("private/secret.txt"), "secret\n").expect("secret");
    let (tree, result) = copy_dir(&scratch, dir_entry(&dir, ".", &["*.map", "private"], None));
    result.expect("exclude copies");

    assert!(tree.join("index.html").is_file());
    assert!(tree.join("_app/immutable/chunks/abc123.js").is_file());
    assert!(
        !tree.join("_app/immutable/chunks/def456.js.map").exists(),
        "a nested name is excluded"
    );
    assert!(
        !tree.join("private").exists(),
        "an excluded directory is skipped with its contents"
    );
}

#[cfg(unix)]
#[test]
fn an_absent_or_empty_directory_fails_unless_optional() {
    let scratch = TempDir::new("gaia-exec-src-dir-absent");
    let root = scratch.join("root");
    fs::create_dir_all(root.join("src/empty")).expect("empty dir");
    let missing = root.join("src/missing");
    let (_, result) = copy_dir(&scratch, dir_entry(&missing, ".", &[], None));
    let error = result.expect_err("absent src_dir");
    assert!(error.contains("missing or has no entries"), "{error}");

    let empty = root.join("src/empty");
    let (_, result) = copy_dir(&scratch, dir_entry(&empty, ".", &[], None));
    let error = result.expect_err("empty src_dir");
    assert!(error.contains("missing or has no entries"), "{error}");

    let mut optional = dir_entry(&missing, "web/", &[], None);
    optional.optional = true;
    let (tree, result) = copy_dir(&scratch, optional);
    result.expect("optional absent src_dir");
    assert!(!tree.join("web").exists());
}

#[cfg(unix)]
#[test]
fn a_src_dir_that_is_a_file_fails_naming_it() {
    let scratch = TempDir::new("gaia-exec-src-dir-file");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    let file = root.join("plain.txt");
    fs::write(&file, "not a directory\n").expect("file");
    let (_, result) = copy_dir(&scratch, dir_entry(&file, ".", &[], None));
    let error = result.expect_err("a file as src_dir");
    assert!(error.contains("is not a directory"), "{error}");
}

#[cfg(unix)]
#[test]
fn src_glob_skips_a_matching_directory_and_copies_the_files() {
    let scratch = TempDir::new("gaia-exec-src-glob-dirs");
    let root = scratch.join("root");
    let mix = root.join("src/mix");
    fs::create_dir_all(mix.join("sub")).expect("sub dir");
    fs::write(mix.join("a.txt"), "a\n").expect("a");
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    let entry = AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        tree: "rootfs".into(),
        src: None,
        src_glob: Some(mix.join("*").display().to_string().into()),
        src_dir: None,
        exclude: Vec::new(),
        content: None,
        json: None,
        dest: "out/".into(),
        mode: None,
        dir_mode: None,
        optional: false,
        preserve_symlink: false,
        allow_missing_libs: false,
    };
    spec.image.assembly = Some(tree_assembly(&tree, vec![entry]));

    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("a directory match is skipped, not an error");
    assert!(tree.join("out/a.txt").is_file());
    assert!(!tree.join("out/sub").exists());
}

/// A numeric value of the run's state, by key.
fn state_count(summary: &AssemblyStagingSummary, key: &str) -> usize {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
        .parse()
        .expect("a count")
}
