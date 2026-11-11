//! UBI images: three of them reproduce dev's writer byte for byte, their
//! headers and volume table are as the kernel reads them, and a volume set or
//! flash that cannot work is refused with the reason named.
//!
//! The fixtures in `tests/fixtures` are dev's writer's output: the on-media
//! code and `write_ubi_image` of dev (`crates/core/formats/src/ubi`,
//! `crates/core/exec/src/operations/assembly/ubi.rs`) at `origin/dev`, built
//! unchanged in a scratch crate and run with the inputs below. Dev keeps no
//! recorded image file of its own, so these are the recording.

use gaia_fs_ubi::format::{EcHeader, VidHeader, VtblRecord};
use gaia_fs_ubi::{UbiFlash, UbiVolume, VolumeKind, ubi_crc, ubi_image};
use gaia_step::Ctx;
use gaia_store::Store;
use gaia_types::{Size, StoreRef};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A file's bytes in `store`, as a reference a volume names.
fn put(store: &Store, dir: &Path, name: &str, bytes: &[u8]) -> StoreRef {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write");
    store.put_file(&path).expect("put").into()
}

fn volume(
    name: &str,
    id: Option<u32>,
    kind: VolumeKind,
    data: Option<StoreRef>,
    size: Option<u64>,
    autoresize: bool,
) -> UbiVolume {
    UbiVolume {
        name: name.into(),
        id,
        kind,
        data,
        size: size.map(Size),
        autoresize,
    }
}

fn flash(peb: u64, min_io: u64, vid: Option<u64>, seq: Option<u32>) -> UbiFlash {
    UbiFlash {
        peb_size: Size(peb),
        min_io_size: Size(min_io),
        sub_page_size: None,
        vid_header_offset: vid.map(Size),
        image_seq: seq,
        flash_size: None,
        bad_pebs_per_1024: 20,
    }
}

/// Builds `volumes` on `flash` in a fresh store and returns the image bytes.
fn build(dir: &Path, id: &str, flash: &UbiFlash, volumes: &[UbiVolume]) -> Result<Vec<u8>, String> {
    let store_path = dir.join("store");
    let store = Store::open(&store_path).expect("store");
    let found = ubi_image(
        &store_path.display().to_string(),
        id,
        flash,
        volumes,
        &Ctx::new(),
    )
    .map_err(|e| e.to_string())?;
    let path = found.object.path_in(&store).expect("in store");
    Ok(std::fs::read(path).expect("read"))
}

fn rootfs() -> Vec<u8> {
    (0..20_000u32)
        .map(|i| (i.wrapping_mul(7) % 251) as u8)
        .collect()
}

fn kernel() -> Vec<u8> {
    (0..5_000u32).map(|i| (i % 13) as u8).collect()
}

#[test]
fn nand_16k_reproduces_dev_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let (rootfs, kernel, config) = (
        put(&store, dir.path(), "rootfs", &rootfs()),
        put(&store, dir.path(), "kernel", &kernel()),
        put(&store, dir.path(), "config", &[0x33; 3000]),
    );
    // 16 KiB PEBs, 512-byte pages: the VID header at 512, data at 1024.
    let leb = 16 * 1024 - 1024;
    let volumes = [
        volume(
            "rootfs_a",
            Some(0),
            VolumeKind::Dynamic,
            Some(rootfs),
            Some(4 * leb),
            false,
        ),
        volume(
            "kernel",
            Some(1),
            VolumeKind::Static,
            Some(kernel),
            None,
            false,
        ),
        volume(
            "config",
            Some(2),
            VolumeKind::Dynamic,
            Some(config),
            Some(4000),
            false,
        ),
        volume("data", Some(3), VolumeKind::Dynamic, None, Some(1), true),
    ];
    let image = build(
        dir.path(),
        "nand",
        &flash(16 * 1024, 512, None, Some(0x1234_5678)),
        &volumes,
    )
    .expect("builds");
    assert!(image == fixture("nand-16k.ubi"), "differs from dev's image");
}

#[test]
fn nor_4k_reproduces_dev_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let blob = put(&store, dir.path(), "blob", &[0xa5; 5000]);
    // 4 KiB PEBs, 1-byte pages, the VID header at 64 (NOR-like).
    let volumes = [volume(
        "blob",
        Some(0),
        VolumeKind::Static,
        Some(blob),
        None,
        false,
    )];
    let image = build(
        dir.path(),
        "nor",
        &flash(4096, 1, Some(64), Some(0xdead_beef)),
        &volumes,
    )
    .expect("builds");
    assert!(image == fixture("nor-4k.ubi"), "differs from dev's image");
}

#[test]
fn spinand_32k_reproduces_dev_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[0x6c; 9000]);
    // 32 KiB PEBs, 2 KiB pages: the VID header at 2048, data at 4096.
    let leb = 32 * 1024 - 4096;
    let volumes = [
        volume(
            "rootfs_a",
            Some(0),
            VolumeKind::Dynamic,
            Some(img),
            Some(2 * leb),
            false,
        ),
        volume("data", Some(1), VolumeKind::Dynamic, None, None, true),
    ];
    let image = build(
        dir.path(),
        "spinand",
        &flash(32 * 1024, 2048, None, Some(7)),
        &volumes,
    )
    .expect("builds");
    assert!(
        image == fixture("spinand-32k.ubi"),
        "differs from dev's image"
    );
}

#[test]
fn the_layout_and_the_table_are_as_the_kernel_reads_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[0x6c; 9000]);
    let leb = 32 * 1024 - 4096;
    let volumes = [
        volume(
            "rootfs_a",
            Some(0),
            VolumeKind::Dynamic,
            Some(img),
            Some(2 * leb),
            false,
        ),
        volume("data", Some(1), VolumeKind::Dynamic, None, None, true),
    ];
    let peb = 32 * 1024;
    let image = build(
        dir.path(),
        "spinand",
        &flash(32 * 1024, 2048, None, Some(7)),
        &volumes,
    )
    .expect("builds");
    assert_eq!(image.len(), 3 * peb, "two layout PEBs and one data LEB");
    let be32 =
        |bytes: &[u8], at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("u32"));
    for block in image.chunks(peb) {
        // The EC header: magic, version, erase counter 0, offsets, sequence.
        assert_eq!(&block[0..4], b"UBI#");
        assert_eq!(block[4], 1);
        assert_eq!(&block[8..16], &[0; 8]);
        assert_eq!(be32(block, 16), 2048, "vid_hdr_offset");
        assert_eq!(be32(block, 20), 4096, "data_offset");
        assert_eq!(be32(block, 24), 7, "image_seq");
        assert_eq!(be32(block, 60), ubi_crc(&block[..60]), "EC CRC");
        let vid = &block[2048..2048 + 64];
        assert_eq!(&vid[0..4], b"UBI!");
        assert_eq!(be32(vid, 60), ubi_crc(&vid[..60]), "VID CRC");
    }
    // The layout volume: id 0x7fffefff, compat 5, LEB 0 and 1.
    let layout_vid = &image[2048..2048 + 64];
    assert_eq!((layout_vid[5], layout_vid[7]), (1, 5));
    assert_eq!(be32(layout_vid, 8), 0x7fff_efff);
    // The table: rootfs_a with its 2 LEBs reserved, data autoresize.
    let table = &image[4096..peb];
    let record = |id: usize| &table[id * 172..][..172];
    assert_eq!(be32(record(0), 0), 2, "rootfs_a reserves 2 LEBs");
    assert_eq!(&record(0)[16..25], b"rootfs_a\0");
    assert_eq!(record(1)[144], 1, "data is autoresize");
    assert_eq!(&record(1)[16..20], b"data");
    for id in 0..128 {
        assert_eq!(
            be32(record(id), 168),
            ubi_crc(&record(id)[..168]),
            "record {id}"
        );
    }
    // The data LEB: dynamic, no size or CRC, padded to the LEB with erased
    // bytes.
    let lnum0 = &image[2 * peb + 2048..][..64];
    assert_eq!(lnum0[5], 1);
    assert_eq!(be32(lnum0, 12), 0);
    assert_eq!(be32(lnum0, 20), 0);
    assert_eq!(be32(lnum0, 32), 0);
}

#[test]
fn the_sequence_number_comes_from_the_id_when_not_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[1; 10]);
    let volumes = [volume(
        "a",
        None,
        VolumeKind::Static,
        Some(img),
        None,
        false,
    )];
    let image = build(
        dir.path(),
        "nand-image",
        &flash(16 * 1024, 512, None, None),
        &volumes,
    )
    .expect("builds");
    let want = ubi_crc(b"nand-image").max(1);
    assert_eq!(
        u32::from_be_bytes(image[24..28].try_into().expect("u32")),
        want
    );
}

#[test]
fn a_volume_set_that_cannot_work_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[1; 10]);
    let error = build(
        dir.path(),
        "bad",
        &flash(16 * 1024, 512, None, Some(1)),
        &[
            volume("a", Some(1), VolumeKind::Dynamic, None, Some(1), true),
            volume("a", None, VolumeKind::Dynamic, None, Some(1), true),
            volume("x", Some(200), VolumeKind::Dynamic, Some(img), None, false),
            volume("e", None, VolumeKind::Dynamic, None, None, false),
        ],
    )
    .expect_err("refused");
    for want in [
        "only one volume can be autoresize",
        "volumes 'a' and 'a' share id 1",
        "two volumes are named 'a'",
        "volume 'x' id 200 is past the last volume table record (88)",
        "volume 'e' has no data, size or autoresize",
    ] {
        assert!(error.contains(want), "{want} not in: {error}");
    }
}

#[test]
fn a_volume_bigger_than_its_size_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[1; 10]);
    let error = build(
        dir.path(),
        "small",
        &flash(16 * 1024, 512, None, Some(1)),
        &[volume(
            "a",
            None,
            VolumeKind::Dynamic,
            Some(img),
            Some(5),
            false,
        )],
    )
    .expect_err("refused");
    assert!(error.contains("more than its size of 5 bytes"), "{error}");
}

#[test]
fn a_flash_too_small_for_the_volumes_and_reserves_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[0x6c; 9000]);
    let leb = 32 * 1024 - 4096;
    let volumes = [volume(
        "rootfs_a",
        Some(0),
        VolumeKind::Dynamic,
        Some(img),
        Some(2 * leb),
        false,
    )];
    let mut small = flash(32 * 1024, 2048, None, Some(7));
    // Three PEBs of flash: the image needs 2 + 2 + 2 PEBs and the reserve.
    small.flash_size = Some(Size(3 * 32 * 1024));
    let error = build(dir.path(), "spi", &small, &volumes).expect_err("refused");
    assert!(error.contains("need 7 PEBs"), "{error}");
    assert!(error.contains("at least"), "{error}");

    // With room for them, it builds.
    small.flash_size = Some(Size(1024 * 32 * 1024));
    build(dir.path(), "spi", &small, &volumes).expect("fits");
}

#[test]
fn every_header_and_record_decodes_and_passes_the_kernel_checks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let img = put(&store, dir.path(), "img", &[0x6c; 9000]);
    let leb = 32 * 1024 - 4096;
    let volumes = [
        volume(
            "rootfs_a",
            Some(0),
            VolumeKind::Dynamic,
            Some(img),
            Some(2 * leb),
            false,
        ),
        volume("data", Some(1), VolumeKind::Dynamic, None, None, true),
    ];
    let image = build(
        dir.path(),
        "spinand",
        &flash(32 * 1024, 2048, None, Some(7)),
        &volumes,
    )
    .expect("builds");
    let peb = 32 * 1024;
    // The EC header and the VID header of each PEB holding data.
    for (index, block) in image.chunks(peb).enumerate() {
        let ec = EcHeader::decode(&block[..64])
            .expect("decodes")
            .expect("written");
        ec.validate().expect("kernel accepts it");
        assert_eq!(
            (ec.vid_header_offset, ec.data_offset, ec.image_seq),
            (2048, 4096, 7)
        );
        let vid = VidHeader::decode(&block[2048..2112])
            .expect("decodes")
            .expect("holds a LEB");
        vid.validate().expect("kernel accepts it");
        // The layout volume's two LEBs, then the data LEB 0.
        let lnum = if index < 2 { index } else { index - 2 };
        assert_eq!(vid.lnum as usize, lnum);
    }
    // The volume table, in both layout copies: 128 records, the two volumes
    // named, the rest empty.
    for copy in 0..2 {
        let table = &image[copy * peb + 4096..(copy + 1) * peb];
        let mut names = Vec::new();
        for id in 0..128 {
            let record = VtblRecord::decode(&table[id * 172..][..172]).expect("decodes");
            if let Some(record) = record {
                record.validate().expect("kernel accepts it");
                names.push((id, record.name, record.reserved_pebs, record.autoresize));
            }
        }
        assert_eq!(
            names,
            [
                (0, "rootfs_a".to_string(), 2, false),
                (1, "data".to_string(), 1, true),
            ],
            "table copy {copy}"
        );
    }
}
