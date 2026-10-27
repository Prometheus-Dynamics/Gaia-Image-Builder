//! `include` on a `src_glob` file entry, and the gzip transform's level and
//! determinism. Split from `tests.rs` for its length.

use super::tests::test_spec;
use super::tests_binaries::{disk_env_panic, state_value};
use super::*;
use gaia_spec::{
    AssemblyFileSpec, AssemblyTransformKindSpec, AssemblyTransformSpec, AssemblyTreeSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;

fn glob_entry(glob: &Path, dest: &str, include: &[&str], optional: bool) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: include.iter().map(|name| name.to_string()).collect(),
        strip: false,
        tree: "rootfs".into(),
        src: None,
        src_glob: Some(glob.display().to_string().into()),
        exclude: Vec::new(),
        content: None,
        json: None,
        dest: dest.into(),
        mode: None,
        dir_mode: None,
        optional,
        src_dir: None,
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

/// Three overlays and a text file, in `root/src/overlays`.
fn overlays(root: &Path) -> std::path::PathBuf {
    let dir = root.join("src/overlays");
    fs::create_dir_all(&dir).expect("overlays");
    for name in ["a.dtbo", "b.dtbo", "c.dtbo"] {
        fs::write(dir.join(name), format!("overlay {name}\n")).expect("overlay");
    }
    fs::write(dir.join("readme.txt"), "not an overlay\n").expect("readme");
    dir.join("*.dtbo")
}

#[test]
fn include_copies_only_the_named_matches() {
    let scratch = TempDir::new("gaia-exec-include");
    let root = scratch.join("gaia-include");
    fs::create_dir_all(&root).expect("root");
    let glob = overlays(&root);
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(tree_assembly(
        &tree,
        vec![glob_entry(&glob, "overlays/", &["a.dtbo", "c.dtbo"], false)],
    ));

    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("include copies");

    assert!(tree.join("overlays/a.dtbo").is_file());
    assert!(tree.join("overlays/c.dtbo").is_file());
    assert!(!tree.join("overlays/b.dtbo").exists());
    assert!(!tree.join("overlays/readme.txt").exists());
}

#[test]
fn include_with_wildcard_entries_copies_every_match_of_each() {
    let scratch = TempDir::new("gaia-exec-include-star");
    let root = scratch.join("gaia-include-star");
    fs::create_dir_all(&root).expect("root");
    let glob = overlays(&root);
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(tree_assembly(
        &tree,
        vec![glob_entry(&glob, "overlays/", &["a*", "b.dtbo"], false)],
    ));

    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("wildcard include");

    assert!(tree.join("overlays/a.dtbo").is_file());
    assert!(tree.join("overlays/b.dtbo").is_file());
    assert!(!tree.join("overlays/c.dtbo").exists());
}

#[test]
fn include_entry_matching_nothing_is_an_error_naming_it() {
    let scratch = TempDir::new("gaia-exec-include-missing");
    let root = scratch.join("gaia-include-missing");
    fs::create_dir_all(&root).expect("root");
    let glob = overlays(&root);
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(tree_assembly(
        &tree,
        vec![glob_entry(&glob, "overlays/", &["a.dtbo", "z.dtbo"], false)],
    ));

    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("unmatched include");

    assert!(error.message.contains("'z.dtbo'"), "{}", error.message);
}

#[test]
fn optional_include_entry_matching_nothing_is_skipped() {
    let scratch = TempDir::new("gaia-exec-include-optional");
    let root = scratch.join("gaia-include-optional");
    fs::create_dir_all(&root).expect("root");
    let glob = overlays(&root);
    let tree = root.join("build/assembly/rootfs");
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(tree_assembly(
        &tree,
        vec![glob_entry(&glob, "overlays/", &["z.dtbo"], true)],
    ));

    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("optional include");

    assert!(
        !tree.join("overlays").exists()
            || fs::read_dir(tree.join("overlays"))
                .expect("dir")
                .next()
                .is_none()
    );
}

fn gzip_transform(src: &Path, dest: &str, level: Option<u32>) -> AssemblyTransformSpec {
    AssemblyTransformSpec {
        kind: AssemblyTransformKindSpec::Gzip,
        src: Some(src.display().to_string().into()),
        dest: dest.into(),
        deterministic: true,
        level,
        hash_dest: None,
        data_block_size: None,
        hash_block_size: None,
        long: false,
        salt: None,
        uuid: None,
    }
}

/// Runs a gzip transform of `image` to `name` in the work dir; returns the
/// bytes the run published (the path its state records).
fn gzip_run(root: &Path, tree: &Path, image: &Path, name: &str, level: Option<u32>) -> Vec<u8> {
    let mut spec = test_spec(root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        transforms: vec![gzip_transform(
            image,
            &format!("$assembly.work/{name}"),
            level,
        )],
        ..tree_assembly(tree, Vec::new())
    });
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("gzip");
    fs::read(state_value(&summary, "transform.1.dest")).expect("gzip output")
}

#[cfg(unix)]
#[test]
fn gzip_output_is_byte_identical_across_runs_and_honours_the_level() {
    let scratch = TempDir::new("gaia-exec-gzip");
    let root = scratch.join("gaia-gzip");
    fs::create_dir_all(&root).expect("root");
    // A kernel-like file: compressible, so the levels differ in size.
    let image = root.join("Image");
    let payload: Vec<u8> = (0..200_000u32)
        .flat_map(|n| (n % 251).to_le_bytes())
        .collect();
    fs::write(&image, &payload).expect("image");
    let tree = root.join("build/assembly/rootfs");

    let first = gzip_run(&root, &tree, &image, "boot.gz", Some(9));
    let second = gzip_run(&root, &tree, &image, "boot.gz", Some(9));
    assert_eq!(first, second, "gzip -n gives the same bytes every run");
    assert_eq!(&first[..2], &[0x1f, 0x8b], "gzip magic");
    assert_eq!(first[3], 0, "no file name in the header");
    assert_eq!(
        &first[4..8],
        &[0, 0, 0, 0],
        "no modification time in the header"
    );

    let level1 = gzip_run(&root, &tree, &image, "boot1.gz", Some(1));
    assert_ne!(level1, first, "level 1 and level 9 differ");
    assert!(
        level1.len() > first.len(),
        "the lower level compresses less"
    );
}
