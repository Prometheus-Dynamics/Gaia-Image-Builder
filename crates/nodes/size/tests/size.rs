//! The size budget on a real stored tree and owners list. A budget that
//! fits passes; one byte under fails and names the item and the largest
//! contributors.

use gaia_size::size_budget;
use gaia_step::{Ctx, ObjectRef, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Image, ImageKind, Partition, PartitionRole, Size, Tree};
use std::fs;
use std::path::Path;

/// The owners list: an app with `usr/bin/app` (1000 bytes) and `data/cfg`
/// (50 bytes), a library with `usr/lib/libx.so` (3000 bytes), and a
/// directory the app owns, which counts for nothing.
const OWNERS: &str = r#"[
  {"path": "/usr", "mode": 493, "uid": 0, "gid": 0, "owners": ["app"]},
  {"path": "/usr/bin/app", "mode": 493, "uid": 0, "gid": 0, "owners": ["app"]},
  {"path": "/usr/lib/libx.so", "mode": 420, "uid": 0, "gid": 0, "owners": ["lib"]},
  {"path": "/data/cfg", "mode": 420, "uid": 0, "gid": 0, "owners": ["app"]}
]"#;

/// The rootfs partition's declared size, and the image's total.
const ROOTFS: u64 = 4096;
const TOTAL: u64 = 8192;

struct Fixture {
    store: String,
    tree: ObjectRef,
    owners: ObjectRef,
}

fn write(dir: &Path, rel: &str, bytes: usize) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, vec![7u8; bytes]).expect("write");
}

fn fixture(root: &Path) -> Fixture {
    let tree = root.join("tree");
    fs::create_dir_all(tree.join("usr")).expect("mkdir");
    write(&tree, "usr/bin/app", 1000);
    write(&tree, "usr/lib/libx.so", 3000);
    write(&tree, "data/cfg", 50);
    let owners = root.join("owners.json");
    fs::write(&owners, OWNERS).expect("write owners");
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    Fixture {
        store: store_path.display().to_string(),
        tree: store.put_tree(&tree).expect("put tree"),
        owners: store.put_file(&owners).expect("put owners"),
    }
}

fn image(fx: &Fixture, partition: u64, total: u64) -> Image {
    Image {
        kind: ImageKind::Disk,
        object: fx.tree.into(),
        size: Size::bytes(total),
        partitions: vec![Partition {
            name: "rootfs".to_string(),
            role: PartitionRole::Rootfs,
            table_type: "0x83".to_string(),
            start: Size::bytes(0),
            size: Size::bytes(partition),
            content: None,
        }],
    }
}

fn check(fx: &Fixture, image: Image, budget: &str) -> Result<Image, StepError> {
    size_budget(
        &fx.store,
        image,
        Tree {
            object: fx.tree.into(),
            entries: Vec::new(),
        },
        Artifact {
            kind: ArtifactKind::parse("owners"),
            object: fx.owners.into(),
        },
        budget,
        &Ctx::new(),
    )
}

fn failure(fx: &Fixture, image: Image, budget: &str) -> String {
    match check(fx, image, budget) {
        Ok(_) => panic!("the budget {budget} should fail"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_budget_that_fits_passes_and_the_image_is_given_back() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    let budget = r#"{
        "total": "8K",
        "partitions": {"rootfs": 4096},
        "components": {"app": 1050, "lib": 3000}
    }"#;
    let out = check(&fx, image(&fx, ROOTFS, TOTAL), budget).expect("within budget");
    assert_eq!(out.partitions.len(), 1);
    assert_eq!(out.size, Size::bytes(TOTAL));
}

#[test]
fn an_empty_budget_checks_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    check(&fx, image(&fx, ROOTFS, TOTAL), "").expect("no budget, no failure");
}

#[test]
fn a_total_one_byte_under_fails_and_names_the_largest_contributors() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    let text = failure(&fx, image(&fx, ROOTFS, TOTAL), r#"{"total": 8191}"#);
    assert!(
        text.contains("the image is 8192 bytes, over its budget of 8191 by 1"),
        "{text}"
    );
    assert!(text.contains("largest contributors:"), "{text}");
    assert!(text.contains("- partition rootfs: 4096 bytes"), "{text}");
    assert!(text.contains("- component lib: 3000 bytes"), "{text}");
}

#[test]
fn a_partition_one_byte_under_fails() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    let text = failure(
        &fx,
        image(&fx, ROOTFS, TOTAL),
        r#"{"partitions": {"rootfs": 4095}}"#,
    );
    assert!(
        text.contains("partition rootfs is 4096 bytes, over its budget of 4095 by 1"),
        "{text}"
    );
}

#[test]
fn a_component_one_byte_under_fails_and_the_biggest_is_listed_first() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    let text = failure(
        &fx,
        image(&fx, ROOTFS, TOTAL),
        r#"{"components": {"app": 1049}}"#,
    );
    assert!(
        text.contains("component app is 1050 bytes, over its budget of 1049 by 1"),
        "{text}"
    );
    let rootfs = text
        .find("partition rootfs: 4096 bytes")
        .expect("the rootfs is named");
    let lib = text
        .find("component lib: 3000 bytes")
        .expect("the library is named");
    assert!(rootfs < lib, "the biggest contributor comes first:\n{text}");
}

#[test]
fn a_budget_naming_a_missing_partition_fails() {
    let root = tempfile::tempdir().expect("tempdir");
    let fx = fixture(root.path());
    let text = failure(
        &fx,
        image(&fx, ROOTFS, TOTAL),
        r#"{"partitions": {"data": 1}}"#,
    );
    assert!(
        text.contains("the budget names partition data, which the image does not have"),
        "{text}"
    );
}
