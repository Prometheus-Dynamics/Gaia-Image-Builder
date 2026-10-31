//! Partitions at a fixed `offset` and images outside the partition table
//! (`table = false`): a Rockchip-style GPT disk with the loader at sector 64
//! and U-Boot at 8 MiB, the MBR numbering around them, and overlaps refused.

use super::*;
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyEbrPlacementSpec,
    AssemblyGptPartitionSpec, AssemblyPartitionTableSpec,
};
use gaia_testkit::TempDir;
use std::fs;

const SECTOR: usize = 512;

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn partition(name: &str, image: Option<&str>) -> AssemblyDiskPartitionSpec {
    AssemblyDiskPartitionSpec {
        name: name.into(),
        kind: None,
        type_alias: None,
        bootable: false,
        image: image.map(Into::into),
        size: None,
        wipe: false,
        materialize: true,
        offset: None,
        table: true,
        gpt: AssemblyGptPartitionSpec::default(),
    }
}

fn outside_table(name: &str, image: &str, offset: &str) -> AssemblyDiskPartitionSpec {
    AssemblyDiskPartitionSpec {
        offset: Some(offset.into()),
        table: false,
        ..partition(name, Some(image))
    }
}

fn disk(
    partition_table: AssemblyPartitionTableSpec,
    partitions: Vec<AssemblyDiskPartitionSpec>,
) -> AssemblyDiskSpec {
    AssemblyDiskSpec {
        id: "disk".into(),
        output: "$assembly.out/disk.img".into(),
        partition_table,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        truncate: None,
        ebr_placement: AssemblyEbrPlacementSpec::Default,
        gpt: Default::default(),
        partitions,
    }
}

fn write_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("idbloader.img"), vec![0x1du8; 200 * 1024]).expect("idbloader");
    fs::write(assets.join("u-boot.itb"), vec![0xb0u8; 900 * 1024]).expect("u-boot");
    fs::write(assets.join("root.img"), vec![0x52u8; 3000]).expect("root");
}

fn assemble(root: &Path, disk: AssemblyDiskSpec) -> Result<Vec<u8>, String> {
    let mut spec = test_spec(root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        disks: vec![disk],
        ..ImageAssemblySpec::default()
    });
    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .map_err(|error| error.message)?;
    Ok(fs::read(root.join("out/images/disk.img")).expect("disk image"))
}

fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("u64"))
}

#[test]
fn rockchip_loaders_sit_at_their_offsets_outside_the_gpt() {
    let scratch = TempDir::new("gaia-exec-disk-offsets");
    let root = scratch.join("rockchip");
    write_assets(&root);
    let mut rootfs = partition("rootfs", Some("@assets/root.img"));
    rootfs.offset = Some("16M".into());
    let layout = disk(
        AssemblyPartitionTableSpec::Gpt,
        vec![
            outside_table("idbloader", "@assets/idbloader.img", "32K"),
            outside_table("uboot", "@assets/u-boot.itb", "8M"),
            rootfs,
            partition("data", Some("@assets/root.img")),
        ],
    );

    let bytes = assemble(&root, layout).expect("disk");

    assert!(
        bytes[64 * SECTOR..][..200 * 1024]
            .iter()
            .all(|byte| *byte == 0x1d)
    );
    assert!(
        bytes[8 << 20..][..900 * 1024]
            .iter()
            .all(|byte| *byte == 0xb0)
    );
    assert!(bytes[16 << 20..][..3000].iter().all(|byte| *byte == 0x52));
    // Two table entries: rootfs at 16 MiB, data at the next MiB; the
    // loaders have none.
    let entries = &bytes[2 * SECTOR..][..128 * 128];
    assert_eq!(le_u64(entries, 32), 32768);
    assert_eq!(le_u64(entries, 128 + 32), 34816);
    assert!(entries[2 * 128..].iter().all(|byte| *byte == 0));
}

#[test]
fn an_mbr_disk_numbers_only_its_table_partitions() {
    let scratch = TempDir::new("gaia-exec-disk-offsets-mbr");
    let root = scratch.join("mbr");
    write_assets(&root);
    let mut boot = partition("boot", Some("@assets/root.img"));
    boot.offset = Some("4M".into());
    let mut parts = vec![
        outside_table("idbloader", "@assets/idbloader.img", "32K"),
        boot,
    ];
    for name in ["a", "b", "c", "d"] {
        parts.push(partition(name, Some("@assets/root.img")));
    }
    let spec = test_spec(&root);
    let assembly = ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        disks: vec![disk(AssemblyPartitionTableSpec::Mbr, parts)],
        ..ImageAssemblySpec::default()
    };
    let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");

    let summary = execute_assembly_disk(&spec, &roots, &assembly.disks[0]).expect("disk");

    let numbers: Vec<(String, u32, bool)> = summary
        .partitions
        .iter()
        .map(|partition| (partition.name.clone(), partition.number, partition.in_table))
        .collect();
    // Five table partitions: boot, a, b primary; c, d logical (p5, p6).
    assert_eq!(
        numbers,
        [
            ("idbloader".into(), 0, false),
            ("boot".into(), 1, true),
            ("a".into(), 2, true),
            ("b".into(), 3, true),
            ("c".into(), 5, true),
            ("d".into(), 6, true),
        ]
    );
    assert_eq!(summary.partitions[1].start_lba, 8192);
    let bytes = fs::read(&summary.output).expect("disk image");
    assert!(
        bytes[64 * SECTOR..][..200 * 1024]
            .iter()
            .all(|byte| *byte == 0x1d)
    );
    // The first MBR entry is boot, at 4 MiB.
    assert_eq!(
        u32::from_le_bytes(bytes[446 + 8..446 + 12].try_into().expect("lba")),
        8192
    );
}

#[test]
fn an_image_over_another_partition_or_the_table_is_refused() {
    let scratch = TempDir::new("gaia-exec-disk-offsets-overlap");
    let root = scratch.join("overlap");
    write_assets(&root);
    // U-Boot at 1 MiB runs into the first partition at the default 1 MiB.
    let overlapping = disk(
        AssemblyPartitionTableSpec::Gpt,
        vec![
            partition("rootfs", Some("@assets/root.img")),
            outside_table("uboot", "@assets/u-boot.itb", "1M"),
        ],
    );
    let error = assemble(&root, overlapping).expect_err("overlap");
    assert!(
        error.contains("partition 'uboot' at sector 2048 overlaps partition 'rootfs'")
            || error.contains("partition 'rootfs' at sector 2048 overlaps partition 'uboot'"),
        "{error}"
    );

    // The idbloader at 8 KiB lands in the GPT entries.
    let in_table = disk(
        AssemblyPartitionTableSpec::Gpt,
        vec![outside_table("idbloader", "@assets/idbloader.img", "8K")],
    );
    let error = assemble(&root, in_table).expect_err("table overlap");
    assert!(
        error.contains("overlaps the GPT header and entries"),
        "{error}"
    );
}

#[test]
fn a_uboot_env_from_generated_text_lands_at_its_offset() {
    let scratch = TempDir::new("gaia-exec-disk-offsets-env");
    let root = scratch.join("env");
    write_assets(&root);
    let mut spec = test_spec(&root);
    let mut env = copy_transform(
        "$assembly.tree.env/uboot-env.txt",
        "$assembly.work/uboot.env",
    );
    env.kind = gaia_spec::AssemblyTransformKindSpec::UbootEnv;
    env.env_size = Some("32K".into());
    env.redundant = true;
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![gaia_spec::AssemblyTreeSpec {
            id: "env".into(),
            path: "$assembly.work/env".into(),
            strip: false,
        }],
        files: vec![gaia_spec::AssemblyFileSpec {
            tree: "env".into(),
            src: None,
            src_glob: None,
            src_dir: None,
            exclude: Vec::new(),
            include: Vec::new(),
            strip: false,
            content: Some("boot_slot=a\nbootcount=0\nbootlimit=3\n".into()),
            json: None,
            dest: "uboot-env.txt".into(),
            mode: None,
            dir_mode: None,
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        }],
        transforms: vec![env],
        disks: vec![disk(
            AssemblyPartitionTableSpec::Gpt,
            vec![
                outside_table("env", "$assembly.work/uboot.env", "4M"),
                partition("rootfs", Some("@assets/root.img")),
            ],
        )],
        ..ImageAssemblySpec::default()
    });

    stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
        .map_err(|error| error.message)
        .expect("assembly");

    let bytes = fs::read(root.join("out/images/disk.img")).expect("disk image");
    let env = &bytes[4 << 20..(4 << 20) + 32 * 1024];
    assert_eq!(env[4], 1, "the active flag");
    assert!(env[5..].starts_with(b"boot_slot=a\0bootcount=0\0bootlimit=3\0\0"));
    assert_eq!(env[..4], crc32(&env[5..]).to_le_bytes());
}
