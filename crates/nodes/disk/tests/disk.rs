//! Disk images: the partition table is what `sfdisk --dump` reads back, each
//! partition and blob sits at its offset byte for byte, the layout refusals
//! name their rule, the same inputs give the same bytes from any store, and a
//! U-Boot environment is a partition's content.
//!
//! The `sfdisk` tests say SKIPPED without util-linux's `sfdisk`, unless
//! `GAIA_REQUIRE_TOOLS=1`, when they fail. The rest run everywhere.

use gaia_disk::{
    DiskPartition, DiskTable, PartitionInput, RawBlob, TableScheme, UbootVar, disk_image, layout,
    uboot_env_artifact, uboot_env_image,
};
use gaia_step::Ctx;
use gaia_store::Store;
use gaia_types::{PartitionRole, Size, StoreRef};
use std::path::{Path, PathBuf};
use std::process::Command;

const MIB: u64 = 1024 * 1024;
const GPT_EFI: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
const GPT_LINUX: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";

fn sfdisk() -> Option<PathBuf> {
    ["/usr/sbin/sfdisk", "/sbin/sfdisk", "/usr/bin/sfdisk"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// The `sfdisk` binary, or `None` with a SKIPPED note.
fn require_sfdisk() -> Option<PathBuf> {
    let found = sfdisk();
    if found.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and sfdisk is missing"
        );
        println!("SKIPPED: sfdisk (util-linux) is not installed");
    }
    found
}

/// A pattern of `len` bytes that differs from its neighbours, so an offset
/// error shows as a byte mismatch.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) | 1)
        .collect()
}

/// A file's bytes in `store`, as a reference a partition or blob names.
fn put(store: &Store, dir: &Path, name: &str, bytes: &[u8]) -> StoreRef {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write");
    store.put_file(&path).expect("put").into()
}

fn partition(
    name: &str,
    role: &str,
    table_type: &str,
    size: Option<u64>,
    content: Option<StoreRef>,
) -> DiskPartition {
    DiskPartition {
        name: name.into(),
        role: PartitionRole::parse(role),
        table_type: table_type.into(),
        bootable: false,
        offset: None,
        size: size.map(Size),
        content,
    }
}

fn blob(name: &str, offset: u64, content: StoreRef) -> RawBlob {
    RawBlob {
        name: name.into(),
        offset: Size(offset),
        content,
    }
}

fn table(scheme: TableScheme, size: Option<u64>) -> DiskTable {
    DiskTable {
        scheme,
        first: None,
        alignment: None,
        size: size.map(Size),
    }
}

/// The image's bytes, read from the store path the result names.
fn bytes_of(store: &Store, image: &gaia_types::Image) -> Vec<u8> {
    let object = image.object.object_ref().expect("object");
    std::fs::read(store.path(&object)).expect("read image")
}

/// `sfdisk --dump` of `path`: its stdout and stderr.
fn dump(sfdisk: &Path, path: &Path) -> (String, String) {
    let out = Command::new(sfdisk)
        .arg("--dump")
        .arg(path)
        .output()
        .expect("sfdisk runs");
    assert!(
        out.status.success(),
        "sfdisk --dump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A dump line's fields: start and size in sectors, and the text after
/// `type=` up to the next comma (the type GUID or byte).
fn fields(line: &str) -> (u64, u64, String) {
    let value = |key: &str| {
        let at = line
            .find(key)
            .unwrap_or_else(|| panic!("no {key} in {line}"));
        line[at + key.len()..]
            .trim_start()
            .split([',', ' '])
            .next()
            .unwrap_or("")
            .to_string()
    };
    (
        value("start=").parse().expect("start"),
        value("size=").parse().expect("size"),
        value("type="),
    )
}

#[test]
fn a_gpt_disk_matches_its_declared_layout_in_sfdisk() {
    let Some(sfdisk) = require_sfdisk() else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let store = Store::open(root.path().join("store")).expect("store");
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).expect("src");

    let boot = pattern(3 * MIB as usize, 1);
    let rootfs = pattern(5000, 2);
    let idbloader = pattern(1000, 3);
    let uboot = pattern(300 * 1024, 4);
    let parts = vec![
        partition(
            "boot",
            "esp",
            GPT_EFI,
            None,
            Some(put(&store, &src, "boot", &boot)),
        ),
        partition(
            "rootfs",
            "rootfs",
            GPT_LINUX,
            None,
            Some(put(&store, &src, "rootfs", &rootfs)),
        ),
        partition("data", "data", GPT_LINUX, Some(2 * MIB), None),
    ];
    let blobs = vec![
        blob(
            "idbloader",
            32 * 1024,
            put(&store, &src, "idbloader", &idbloader),
        ),
        blob("u-boot", 8 * MIB, put(&store, &src, "u-boot", &uboot)),
    ];
    let image = disk_image(
        &store.root().display().to_string(),
        "test-gpt",
        &table(TableScheme::Gpt, Some(10 * MIB)),
        &parts,
        &blobs,
        &Ctx::new(),
    )
    .expect("the disk is made");

    // Partition by partition, from the description: boot at 1 MiB sized by
    // its content; rootfs at the next MiB boundary, its 5000 bytes rounded to
    // 5120; data at the next boundary, 2 MiB.
    let expected = [
        (2048u64, 6144u64, GPT_EFI, "boot"),
        (8192, 10, GPT_LINUX, "rootfs"),
        (10240, 4096, GPT_LINUX, "data"),
    ];
    let (out, err) = dump(&sfdisk, &store_path(&store, &image));
    assert!(!err.to_lowercase().contains("crc"), "sfdisk: {err}");
    assert!(out.contains("label: gpt"), "{out}");
    let rows: Vec<&str> = out.lines().filter(|l| l.contains("start=")).collect();
    assert_eq!(rows.len(), expected.len(), "{out}");
    for (row, (start, size, type_guid, name)) in rows.iter().zip(expected) {
        let (got_start, got_size, got_type) = fields(row);
        assert_eq!((got_start, got_size), (start, size), "{row}");
        assert_eq!(got_type, type_guid, "{row}");
        assert!(row.contains(&format!("name=\"{name}\"")), "{row}");
    }

    let made = bytes_of(&store, &image);
    assert_eq!(made.len() as u64, 10 * MIB);
    assert_eq!(
        &made[32 * 1024..32 * 1024 + idbloader.len()],
        &idbloader[..]
    );
    assert_eq!(
        &made[8 * MIB as usize..8 * MIB as usize + uboot.len()],
        &uboot[..]
    );
    assert_eq!(&made[MIB as usize..MIB as usize + boot.len()], &boot[..]);
    let rootfs_at = 4 * MIB as usize;
    assert_eq!(&made[rootfs_at..rootfs_at + rootfs.len()], &rootfs[..]);
    assert!(
        made[rootfs_at + rootfs.len()..rootfs_at + 5120]
            .iter()
            .all(|b| *b == 0)
    );
}

#[test]
fn an_mbr_disk_matches_its_declared_layout_in_sfdisk() {
    let Some(sfdisk) = require_sfdisk() else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let store = Store::open(root.path().join("store")).expect("store");
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).expect("src");

    let kernel = pattern(40_000, 5);
    let mut boot = partition(
        "boot",
        "boot",
        "0x0c",
        None,
        Some(put(&store, &src, "boot", &kernel)),
    );
    boot.bootable = true;
    let parts = vec![
        boot,
        partition("rootfs", "rootfs", "0x83", Some(MIB), None),
        partition("data", "data", "0x83", Some(2 * MIB), None),
    ];
    let blobs = vec![blob(
        "loader",
        64 * 1024,
        put(&store, &src, "loader", &kernel[..4096]),
    )];
    let image = disk_image(
        &store.root().display().to_string(),
        "test-mbr",
        &table(TableScheme::Mbr, None),
        &parts,
        &blobs,
        &Ctx::new(),
    )
    .expect("the disk is made");

    let (out, _err) = dump(&sfdisk, &store_path(&store, &image));
    assert!(out.contains("label: dos"), "{out}");
    let rows: Vec<&str> = out.lines().filter(|l| l.contains("start=")).collect();
    assert_eq!(rows.len(), 3, "{out}");
    // boot at 1 MiB, its 40000 bytes rounded to 40448; rootfs at the next
    // MiB boundary; data after it at the boundary past rootfs's 1 MiB.
    let expected = [
        (2048u64, 79u64, "c"),
        (4096, 2048, "83"),
        (6144, 4096, "83"),
    ];
    for (row, (start, size, type_byte)) in rows.iter().zip(expected) {
        let (got_start, got_size, got_type) = fields(row);
        assert_eq!(
            (got_start, got_size, got_type.as_str()),
            (start, size, type_byte),
            "{row}"
        );
    }
    assert!(
        rows[0].contains("bootable") || rows[0].contains('*'),
        "{}",
        rows[0]
    );
    assert!(!rows[1].contains("bootable"), "{}", rows[1]);

    let made = bytes_of(&store, &image);
    assert_eq!(&made[64 * 1024..64 * 1024 + 4096], &kernel[..4096]);
    assert_eq!(
        &made[MIB as usize..MIB as usize + kernel.len()],
        &kernel[..]
    );
}

/// The store path of an image, for tools that read a file.
fn store_path(store: &Store, image: &gaia_types::Image) -> PathBuf {
    store.path(&image.object.object_ref().expect("object"))
}

#[test]
fn a_description_that_cannot_be_placed_is_refused_with_its_rule() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = Store::open(root.path().join("store")).expect("store");
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).expect("src");
    let content = put(&store, &src, "content", &pattern(4096, 6));
    let store_dir = store.root().display().to_string();
    let run = |scheme: TableScheme, parts: &[DiskPartition], blobs: &[RawBlob]| {
        disk_image(
            &store_dir,
            "refused",
            &table(scheme, None),
            parts,
            blobs,
            &Ctx::new(),
        )
        .expect_err("refused")
        .to_string()
    };

    let mut overlap = partition("a", "data", GPT_LINUX, None, Some(content.clone()));
    overlap.offset = Some(Size(MIB / 2));
    assert!(run(TableScheme::Gpt, &[overlap], &[]).contains("inside the space before it"));

    let bad = partition("a", "data", "0x83", None, Some(content.clone()));
    assert!(run(TableScheme::Gpt, std::slice::from_ref(&bad), &[]).contains("table_type"));

    let five: Vec<DiskPartition> = (0..5)
        .map(|i| {
            partition(
                &format!("p{i}"),
                "data",
                "0x83",
                None,
                Some(content.clone()),
            )
        })
        .collect();
    assert!(run(TableScheme::Mbr, &five, &[]).contains("do not fit"));

    let blob_over = blob("loader", MIB, content.clone());
    let part = partition("a", "data", GPT_LINUX, Some(2 * MIB), None);
    assert!(run(TableScheme::Gpt, &[part], &[blob_over]).contains("overlaps partition 'a'"));

    let tree = StoreRef {
        kind: gaia_types::ObjectKind::Tree,
        digest: content.digest.clone(),
    };
    let as_tree = partition("a", "data", GPT_LINUX, Some(MIB), Some(tree));
    assert!(run(TableScheme::Gpt, &[as_tree], &[]).contains("is a tree"));
}

#[test]
fn the_same_inputs_give_the_same_bytes_from_any_store() {
    let make = |mtime: u64| {
        let root = tempfile::tempdir().expect("tempdir");
        let store = Store::open(root.path().join("store")).expect("store");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).expect("src");
        let file = src.join("boot");
        std::fs::write(&file, pattern(70_000, 7)).expect("write");
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime);
        std::fs::File::open(&file)
            .expect("open")
            .set_times(std::fs::FileTimes::new().set_modified(time))
            .expect("time");
        let content: StoreRef = store.put_file(&file).expect("put").into();
        let parts = vec![partition("boot", "boot", GPT_EFI, None, Some(content))];
        let image = disk_image(
            &store.root().display().to_string(),
            "repro",
            &table(TableScheme::Gpt, None),
            &parts,
            &[],
            &Ctx::new(),
        )
        .expect("made");
        (image.object.digest.clone(), image.size)
    };
    assert_eq!(make(1_000_000), make(2_000_000_000));
}

#[test]
fn a_uboot_environment_is_a_partitions_content() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = Store::open(root.path().join("store")).expect("store");
    let store_dir = store.root().display().to_string();
    let vars = [
        UbootVar {
            name: "boot_slot".into(),
            value: "a".into(),
        },
        UbootVar {
            name: "bootcount".into(),
            value: "0".into(),
        },
    ];
    let artifact =
        uboot_env_artifact(&store_dir, "env", &vars, 0x2000, true, 2, &Ctx::new()).expect("env");
    assert_eq!(artifact.kind.as_str(), "uboot-env");
    let env_path = store.path(&artifact.object.object_ref().expect("object"));
    let once = uboot_env_image(&vars, 0x2000, true).expect("image");
    let bytes = std::fs::read(&env_path).expect("read");
    assert_eq!(bytes.len(), 0x4000);
    assert_eq!(&bytes[..0x2000], &once[..]);
    assert_eq!(&bytes[0x2000..], &once[..]);

    let parts = vec![DiskPartition {
        offset: Some(Size(MIB)),
        ..partition("env", "raw", "0x0c", None, Some(artifact.object.clone()))
    }];
    let image = disk_image(
        &store_dir,
        "env-disk",
        &table(TableScheme::Mbr, None),
        &parts,
        &[],
        &Ctx::new(),
    )
    .expect("disk");
    let made = bytes_of(&store, &image);
    assert_eq!(&made[MIB as usize..MIB as usize + 0x4000], &bytes[..]);

    // The layout alone, for the refusal that a size of zero is.
    let input = PartitionInput {
        name: "x".into(),
        role: PartitionRole::parse("raw"),
        table_type: "0x0c".into(),
        bootable: false,
        offset: None,
        size: None,
        content_len: None,
    };
    assert!(layout(TableScheme::Mbr, None, None, None, &[input], &[]).is_err());
}
