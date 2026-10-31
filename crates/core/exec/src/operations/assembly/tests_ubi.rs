//! UBI images through the assembly: an A/B slot set read back PEB by PEB
//! (every header CRC, the volume table, each volume's bytes), the image
//! placed on a disk next to a loader, and a flash too small refused.

use super::*;
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyEbrPlacementSpec,
    AssemblyGptPartitionSpec, AssemblyPartitionTableSpec, AssemblyUbiSpec,
    AssemblyUbiVolumeKindSpec, AssemblyUbiVolumeSpec,
};
use gaia_testkit::TempDir;
use std::collections::BTreeMap;
use std::fs;

const PEB: usize = 128 * 1024;
const LEB: usize = PEB - 4096;

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().expect("u32"))
}

fn volume(name: &str, image: Option<&str>, size: Option<&str>) -> AssemblyUbiVolumeSpec {
    AssemblyUbiVolumeSpec {
        name: name.into(),
        id: None,
        kind: AssemblyUbiVolumeKindSpec::Dynamic,
        image: image.map(Into::into),
        size: size.map(Into::into),
        autoresize: false,
    }
}

fn ab_ubi(flash_size: Option<&str>) -> AssemblyUbiSpec {
    let mut data = volume("data", None, Some("1M"));
    data.autoresize = true;
    let mut kernel = volume("kernel", Some("@assets/Image"), None);
    kernel.kind = AssemblyUbiVolumeKindSpec::Static;
    AssemblyUbiSpec {
        id: "nand".into(),
        output: "$assembly.out/nand.ubi".into(),
        peb_size: "128K".into(),
        min_io_size: "2K".into(),
        sub_page_size: None,
        vid_header_offset: None,
        image_seq: Some(0x0bad_cafe),
        flash_size: flash_size.map(Into::into),
        bad_pebs_per_1024: gaia_spec::UBI_DEFAULT_BAD_PEBS_PER_1024,
        volumes: vec![
            kernel,
            volume("rootfs_a", Some("@assets/rootfs.erofs"), Some("2M")),
            volume("rootfs_b", None, Some("2M")),
            data,
        ],
    }
}

fn write_assets(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    let kernel: Vec<u8> = (0..LEB + 777).map(|index| (index * 7) as u8).collect();
    let rootfs: Vec<u8> = (0..3 * LEB + 5)
        .map(|index| (index * 13 + 1) as u8)
        .collect();
    fs::write(assets.join("Image"), &kernel).expect("kernel");
    fs::write(assets.join("rootfs.erofs"), &rootfs).expect("rootfs");
    fs::write(assets.join("idbloader.img"), vec![0x1du8; 64 * 1024]).expect("idbloader");
    (kernel, rootfs)
}

fn stage(root: &Path, assembly: ImageAssemblySpec) -> Result<(), String> {
    let mut spec = test_spec(root);
    spec.image.assembly = Some(assembly);
    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .map(|_| ())
        .map_err(|error| error.message)
}

/// The volumes of a UBI image as the kernel would see them: every PEB's
/// headers checked, LEBs gathered by volume id and number.
fn read_back(image: &[u8]) -> (Vec<u8>, BTreeMap<u32, Vec<u8>>) {
    assert_eq!(image.len() % PEB, 0);
    let mut table = Vec::new();
    let mut lebs: BTreeMap<(u32, u32), Vec<u8>> = BTreeMap::new();
    for peb in image.as_chunks::<PEB>().0 {
        assert_eq!(&peb[..4], b"UBI#");
        assert_eq!(be32(peb, 60), super::ubi::ubi_crc(&peb[..60]));
        assert_eq!(be32(peb, 24), 0x0bad_cafe);
        let vid = &peb[2048..2048 + 64];
        assert_eq!(&vid[..4], b"UBI!");
        assert_eq!(be32(vid, 60), super::ubi::ubi_crc(&vid[..60]));
        let (volume_id, lnum) = (be32(vid, 8), be32(vid, 12));
        let data = &peb[4096..];
        if volume_id == 0x7fff_efff {
            if table.is_empty() {
                table = data.to_vec();
            } else {
                assert_eq!(table, data, "both layout copies agree");
            }
            continue;
        }
        let bytes = if vid[5] == 2 {
            let size = be32(vid, 20) as usize;
            assert_eq!(be32(vid, 32), super::ubi::ubi_crc(&data[..size]));
            data[..size].to_vec()
        } else {
            data.to_vec()
        };
        assert!(lebs.insert((volume_id, lnum), bytes).is_none());
    }
    let mut volumes: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for ((volume_id, _), bytes) in lebs {
        volumes.entry(volume_id).or_default().extend(bytes);
    }
    (table, volumes)
}

#[test]
fn an_ab_slot_set_reads_back_volume_by_volume() {
    let scratch = TempDir::new("gaia-exec-ubi");
    let root = scratch.join("ubi");
    let (kernel, rootfs) = write_assets(&root);

    stage(
        &root,
        ImageAssemblySpec {
            work_dir: Some("$assembly.work".into()),
            ubis: vec![ab_ubi(Some("8M"))],
            ..ImageAssemblySpec::default()
        },
    )
    .expect("assembly");

    let image = fs::read(root.join("out/images/nand.ubi")).expect("ubi image");
    // Two layout PEBs, two kernel LEBs, four rootfs LEBs; nothing for the
    // empty slot or the data volume.
    assert_eq!(image.len(), 8 * PEB);
    let (table, volumes) = read_back(&image);
    assert_eq!(volumes.keys().copied().collect::<Vec<_>>(), [0, 1]);
    assert_eq!(
        volumes[&0], kernel,
        "a static volume keeps its exact length"
    );
    assert_eq!(&volumes[&1][..rootfs.len()], &rootfs[..]);
    assert!(volumes[&1][rootfs.len()..].iter().all(|byte| *byte == 0xff));

    let record = |id: usize| &table[id * 172..][..172];
    let reserved: Vec<u32> = (0..4).map(|id| be32(record(id), 0)).collect();
    // 2M is 16.5 LEBs of 124 KiB, so 17; data's 1M is 9.
    assert_eq!(reserved, [2, 17, 17, 9]);
    let names: Vec<&[u8]> = (0..4)
        .map(|id| {
            let length = u16::from_be_bytes([record(id)[14], record(id)[15]]) as usize;
            &record(id)[16..16 + length]
        })
        .collect();
    assert_eq!(names, [&b"kernel"[..], b"rootfs_a", b"rootfs_b", b"data"]);
    assert_eq!(record(0)[12], 2);
    assert_eq!(record(3)[144], 1, "data is autoresize");
    for id in 0..128 {
        assert_eq!(
            be32(record(id), 168),
            super::ubi::ubi_crc(&record(id)[..168])
        );
    }
}

#[test]
fn a_ubi_image_sits_next_to_the_loader_on_a_raw_flash_disk() {
    let scratch = TempDir::new("gaia-exec-ubi-disk");
    let root = scratch.join("disk");
    write_assets(&root);
    let mut ubi = ab_ubi(None);
    ubi.output = "$assembly.work/nand.ubi".into();
    let raw = |name: &str, image: &str, offset: &str| AssemblyDiskPartitionSpec {
        name: name.into(),
        kind: None,
        type_alias: None,
        bootable: false,
        image: Some(image.into()),
        size: None,
        wipe: false,
        materialize: true,
        offset: Some(offset.into()),
        table: false,
        gpt: AssemblyGptPartitionSpec::default(),
    };

    stage(
        &root,
        ImageAssemblySpec {
            work_dir: Some("$assembly.work".into()),
            ubis: vec![ubi],
            disks: vec![AssemblyDiskSpec {
                id: "flash".into(),
                output: "$assembly.out/flash.img".into(),
                partition_table: AssemblyPartitionTableSpec::Gpt,
                signature: None,
                signature_text: None,
                first_lba: None,
                alignment_lba: None,
                truncate: None,
                ebr_placement: AssemblyEbrPlacementSpec::Default,
                gpt: Default::default(),
                partitions: vec![
                    raw("idbloader", "@assets/idbloader.img", "32K"),
                    raw("ubi", "$assembly.work/nand.ubi", "4M"),
                ],
            }],
            ..ImageAssemblySpec::default()
        },
    )
    .expect("assembly");

    let flash = fs::read(root.join("out/images/flash.img")).expect("flash image");
    let ubi = fs::read(root.join("build/assembly/nand.ubi")).expect("ubi image");
    assert!(
        flash[32 * 1024..][..64 * 1024]
            .iter()
            .all(|byte| *byte == 0x1d)
    );
    assert_eq!(&flash[4 << 20..(4 << 20) + ubi.len()], &ubi[..]);
}

#[test]
fn a_flash_too_small_for_the_reservations_fails_the_build() {
    let scratch = TempDir::new("gaia-exec-ubi-small");
    let root = scratch.join("small");
    write_assets(&root);

    // 2 layout PEBs, UBI's 2 and the volumes' 45 make 49, and a device of
    // 49 or 50 PEBs keeps 1 for bad blocks: 50 PEBs (6400K) is the least.
    let error = stage(
        &root,
        ImageAssemblySpec {
            work_dir: Some("$assembly.work".into()),
            ubis: vec![ab_ubi(Some("6272K"))],
            ..ImageAssemblySpec::default()
        },
    )
    .expect_err("too small");
    assert!(
        error.contains("assembly ubi 'nand': the volumes and UBI's reserves need 50 PEBs"),
        "{error}"
    );
    assert!(error.contains("at least 6553600 bytes"), "{error}");
    assert!(!root.join("out/images/nand.ubi").exists());

    stage(
        &root,
        ImageAssemblySpec {
            work_dir: Some("$assembly.work".into()),
            ubis: vec![ab_ubi(Some("6400K"))],
            ..ImageAssemblySpec::default()
        },
    )
    .expect("6400K fits");
}
