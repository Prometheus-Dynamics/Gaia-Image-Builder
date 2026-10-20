//! Stage entries mirror their sources: what a source no longer produces is
//! removed from the target on the next apply, and only what the stage itself
//! wrote and left unchanged.
use super::single_pass_shared::{feed_spec, run_build, squashfs_image, write_buildroot_source};
use super::*;

/// A workspace with the given stage entries: `(id, source dir or file under
/// assets/, destination)`. Sources are written by the caller.
fn stage_spec(workspace_root: &Path, entries: &[(&str, &str, &str)]) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("stage-owned-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    for (id, src, dest) in entries {
        spec.stage.files.push(gaia_spec::StageFileSpec {
            id: (*id).into(),
            src: (*src).into(),
            dest: (*dest).into(),
            mode: None,
            origin: gaia_spec::StageContentOriginSpec::StaticAsset,
        });
    }
    spec
}

fn feed_image(ids: &[&str]) -> ImageSpec {
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
    image.feed.stage_files = ids.iter().map(|id| (*id).into()).collect();
    image
}

/// One feed run as the provider does it: prune against the last record, copy,
/// then record what the copy left.
fn run_feed(spec: &ResolvedBuildSpec, image: &ImageSpec, output_dir: &Path, target: &Path) {
    fs::create_dir_all(target).expect("target dir");
    let ownership = plan_stage_ownership(spec, image).expect("plan");
    prune_stage_ownership(output_dir, &ownership, target).expect("prune");
    apply_image_feed_to_rootfs(spec, image, target).expect("apply feed");
    record_stage_ownership(output_dir, &ownership, target).expect("record");
}

fn put(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent dirs");
    fs::write(path, contents).expect("write");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn gone(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

/// Three files under `assets/tree`, installed to `/opt/tree`.
fn tree_fixture(root: &Path) {
    for name in ["a", "b", "c"] {
        put(
            &root.join("assets/tree").join(name),
            &format!("{name}-content"),
        );
    }
}

#[test]
fn a_removed_file_of_a_dir_entry_leaves_the_target() {
    let scratch = TempDir::new("gaia-stage-owned-dir");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    let image = feed_image(&["tree"]);

    run_feed(&spec, &image, &output_dir, &target);
    assert_eq!(read(&target.join("opt/tree/b")), "b-content");

    fs::remove_file(root.join("assets/tree/b")).expect("drop b from the source");
    run_feed(&spec, &image, &output_dir, &target);

    assert!(gone(&target.join("opt/tree/b")), "b is still shipped");
    assert_eq!(read(&target.join("opt/tree/a")), "a-content");
    assert_eq!(read(&target.join("opt/tree/c")), "c-content");
}

#[test]
fn an_unrecorded_file_in_the_same_dir_survives() {
    let scratch = TempDir::new("gaia-stage-owned-unrecorded");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    let image = feed_image(&["tree"]);
    run_feed(&spec, &image, &output_dir, &target);

    put(&target.join("opt/tree/other"), "a package's file");
    fs::remove_file(root.join("assets/tree/b")).expect("drop b");
    run_feed(&spec, &image, &output_dir, &target);

    assert_eq!(read(&target.join("opt/tree/other")), "a package's file");
    assert!(gone(&target.join("opt/tree/b")));
}

#[test]
fn a_stage_file_changed_since_the_copy_survives() {
    let scratch = TempDir::new("gaia-stage-owned-changed");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    let image = feed_image(&["tree"]);
    run_feed(&spec, &image, &output_dir, &target);

    // A package rewrote b after the stage wrote it.
    put(
        &target.join("opt/tree/b"),
        "rewritten by a package, longer than before",
    );
    fs::remove_file(root.join("assets/tree/b")).expect("drop b");
    run_feed(&spec, &image, &output_dir, &target);

    assert_eq!(
        read(&target.join("opt/tree/b")),
        "rewritten by a package, longer than before"
    );
}

#[test]
fn a_removed_stage_item_takes_its_files_and_directories() {
    let scratch = TempDir::new("gaia-stage-owned-removed");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    run_feed(&spec, &feed_image(&["tree"]), &output_dir, &target);
    assert!(target.join("opt/tree/a").exists());

    let without = stage_spec(&root, &[]);
    run_feed(&without, &feed_image(&[]), &output_dir, &target);

    assert!(
        gone(&target.join("opt/tree")),
        "the removed item's dir stays"
    );
}

#[test]
fn a_path_moved_to_another_item_is_not_removed() {
    let scratch = TempDir::new("gaia-stage-owned-moved");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    put(&root.join("assets/old-src"), "old item");
    put(&root.join("assets/new-src"), "new item");
    let before = stage_spec(&root, &[("old", "assets/old-src", "/opt/p")]);
    run_feed(&before, &feed_image(&["old"]), &output_dir, &target);
    assert_eq!(read(&target.join("opt/p")), "old item");

    let after = stage_spec(&root, &[("new", "assets/new-src", "/opt/p")]);
    run_feed(&after, &feed_image(&["new"]), &output_dir, &target);

    assert_eq!(read(&target.join("opt/p")), "new item");
}

#[test]
fn a_renamed_single_file_removes_its_old_destination() {
    let scratch = TempDir::new("gaia-stage-owned-renamed");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    put(&root.join("assets/motd"), "hello");
    let before = stage_spec(&root, &[("motd", "assets/motd", "/etc/motd")]);
    run_feed(&before, &feed_image(&["motd"]), &output_dir, &target);
    assert_eq!(read(&target.join("etc/motd")), "hello");

    let after = stage_spec(&root, &[("motd", "assets/motd", "/etc/motd2")]);
    run_feed(&after, &feed_image(&["motd"]), &output_dir, &target);

    assert!(gone(&target.join("etc/motd")), "the old destination stays");
    assert_eq!(read(&target.join("etc/motd2")), "hello");
}

#[test]
fn without_a_record_nothing_is_removed() {
    let scratch = TempDir::new("gaia-stage-owned-norecord");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    // Leftovers from a build before this scheme: no record describes them.
    put(&target.join("opt/tree/b"), "leftover");
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    let ownership = plan_stage_ownership(&spec, &feed_image(&["tree"])).expect("plan");

    let messages = prune_stage_ownership(&output_dir, &ownership, &target).expect("prune");

    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(read(&target.join("opt/tree/b")), "leftover");
}

#[test]
fn the_record_lists_what_the_copy_left_only() {
    let scratch = TempDir::new("gaia-stage-owned-record");
    let root = scratch.join("workspace");
    let output_dir = scratch.join("output");
    let target = output_dir.join("target");
    tree_fixture(&root);
    let spec = stage_spec(&root, &[("tree", "assets/tree", "/opt/tree")]);
    let ownership = plan_stage_ownership(&spec, &feed_image(&["tree"])).expect("plan");
    fs::create_dir_all(&target).expect("target");
    apply_image_feed_to_rootfs(&spec, &feed_image(&["tree"]), &target).expect("apply");

    // The source of d is planned but the copy never happened: not recorded.
    put(&root.join("assets/tree/d"), "not copied");
    let planned = plan_stage_ownership(&spec, &feed_image(&["tree"])).expect("plan");
    record_stage_ownership(&output_dir, &planned, &target).expect("record");
    let records = read_stage_ownership(&output_dir)
        .expect("read")
        .expect("record");
    let recorded = records
        .iter()
        .filter(|record| matches!(record.kind, OwnedKind::File { .. }))
        .map(|record| record.rel.as_str())
        .collect::<Vec<_>>();
    assert!(recorded.contains(&"opt/tree/a"), "{recorded:?}");
    assert!(!recorded.contains(&"opt/tree/d"), "{recorded:?}");
    assert!(ownership.produced.contains("opt/tree"));
}

/// A fake Buildroot run: the feed is applied by the post-build script.
#[test]
fn a_dir_entry_mirrors_its_source_through_make() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-stage-owned-make");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    write_buildroot_source(
        &workspace_root,
        "build",
        super::single_pass_shared::SINGLE_PASS_MAKEFILE,
    );
    let mut spec = feed_spec(&workspace_root, "stage-owned", "build");
    for name in ["a", "b", "c"] {
        put(
            &workspace_root.join("assets/tree").join(name),
            &format!("{name}-content"),
        );
    }
    spec.stage.files.push(gaia_spec::StageFileSpec {
        id: "tree".into(),
        src: "assets/tree".into(),
        dest: "/opt/tree".into(),
        mode: None,
        origin: gaia_spec::StageContentOriginSpec::StaticAsset,
    });
    let image = squashfs_image("buildroot-source", &["tree"]);
    let output_dir = workspace_root.join("build/image/buildroot-output");

    run_build(&spec, &image, &ImageExecutionPolicy::default()).expect("first build");
    let listing = |name: &str| {
        fs::read_to_string(output_dir.join(name))
            .expect("listing")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert!(listing("images/rootfs.squashfs").contains(&"./opt/tree/b".to_string()));

    fs::remove_file(workspace_root.join("assets/tree/b")).expect("drop b");
    put(
        &output_dir.join("target/opt/tree/other"),
        "a package's file",
    );
    run_build(&spec, &image, &ImageExecutionPolicy::default()).expect("second build");

    let shipped = listing("images/rootfs.squashfs");
    assert!(
        !shipped.contains(&"./opt/tree/b".to_string()),
        "{shipped:?}"
    );
    assert!(shipped.contains(&"./opt/tree/a".to_string()), "{shipped:?}");
    assert!(
        shipped.contains(&"./opt/tree/other".to_string()),
        "{shipped:?}"
    );
}
