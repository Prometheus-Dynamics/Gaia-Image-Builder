//! MBR disks with `truncate = "last-data"`, unmaterialized partitions and
//! EBR placement. The tests read the result back the way the Linux msdos
//! parser does (walking the EBR chain) and, when `sfdisk` is installed, with
//! `sfdisk --dump`.

pub mod support;

use gaia_exec::{ExecutionOutcome, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyDiskTruncateSpec,
    AssemblyEbrPlacementSpec, AssemblyPartitionTableSpec, ImageAssemblySpec, ResolvedBuildSpec,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{provider_catalogs, test_spec};

const MIB: u64 = 1024 * 1024;
/// Sectors of the full-size disk for the layouts below, from the planner.
const BOOT_BYTES: usize = 300 * 1024;
const ROOT_BYTES: usize = 1000;

fn partition(
    name: &str,
    type_alias: Option<&str>,
    bootable: bool,
    image: Option<&Path>,
    size: Option<&str>,
    wipe: bool,
    materialize: bool,
) -> AssemblyDiskPartitionSpec {
    AssemblyDiskPartitionSpec {
        name: name.into(),
        kind: None,
        type_alias: type_alias.map(Into::into),
        bootable,
        image: image.map(|path| path.display().to_string().into()),
        size: size.map(Into::into),
        wipe,
        materialize,
    }
}

fn mbr_disk(
    output: &Path,
    truncate: Option<AssemblyDiskTruncateSpec>,
    ebr_placement: AssemblyEbrPlacementSpec,
    partitions: Vec<AssemblyDiskPartitionSpec>,
) -> AssemblyDiskSpec {
    AssemblyDiskSpec {
        id: "sdcard".into(),
        output: output.display().to_string().into(),
        partition_table: AssemblyPartitionTableSpec::Mbr,
        signature: None,
        signature_text: Some("GAIA".into()),
        first_lba: None,
        alignment_lba: None,
        truncate,
        ebr_placement,
        partitions,
    }
}

/// Writes the test images into the build dir and runs the assembly with
/// `disk` as the only disk. `archive_name` publishes the disk as an archive.
fn assemble(
    spec: &mut ResolvedBuildSpec,
    disk: AssemblyDiskSpec,
    archive_name: Option<&str>,
) -> ExecutionOutcome {
    if let Some(name) = archive_name {
        spec.image.output.collect_dir = Some(
            Path::new(&spec.workspace.out_dir)
                .join("images")
                .display()
                .to_string(),
        );
        spec.image.output.archive_name = Some(name.into());
    }
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![disk],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    )
}

fn write_images(build_dir: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(build_dir).expect("build dir");
    let boot = build_dir.join("boot.img");
    let rootfs = build_dir.join("rootfs.img");
    fs::write(&boot, vec![0x11; BOOT_BYTES]).expect("boot image");
    fs::write(&rootfs, vec![0x22; ROOT_BYTES]).expect("rootfs image");
    (boot, rootfs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    ptype: u8,
    start: u32,
    count: u32,
}

fn sector(disk: &[u8], lba: u32) -> &[u8] {
    let offset = lba as usize * 512;
    &disk[offset..offset + 512]
}

fn entries(sector: &[u8]) -> Vec<Entry> {
    (0..4)
        .map(|slot| {
            let offset = 446 + slot * 16;
            Entry {
                ptype: sector[offset + 4],
                start: u32::from_le_bytes(sector[offset + 8..offset + 12].try_into().unwrap()),
                count: u32::from_le_bytes(sector[offset + 12..offset + 16].try_into().unwrap()),
            }
        })
        .collect()
}

/// Primary entries from the MBR, by slot.
fn primaries(disk: &[u8]) -> Vec<Entry> {
    entries(sector(disk, 0))
}

/// Logical partitions as Linux's msdos parser finds them: start at the
/// extended partition, follow each EBR's link entry (relative to the
/// extended start) and read entry 0 relative to its own EBR.
fn logical_partitions(disk: &[u8]) -> Vec<(u32, u32)> {
    let extended = primaries(disk)
        .into_iter()
        .find(|entry| entry.ptype == 0x05 && entry.count > 0)
        .expect("extended partition");
    let mut ebr = extended.start;
    let mut found = Vec::new();
    loop {
        let slots = entries(sector(disk, ebr));
        let logical = slots[0];
        assert!(logical.count > 0 && logical.ptype != 0x05, "EBR at {ebr}");
        found.push((ebr + logical.start, logical.count));
        let link = slots[1];
        if link.ptype == 0x05 && link.count > 0 {
            ebr = extended.start + link.start;
        } else {
            break;
        }
    }
    found
}

/// `(start, sectors)` of each partition `sfdisk --dump` lists, or `None`
/// when sfdisk is not installed.
fn sfdisk_partitions(path: &Path) -> Option<Vec<(u64, u64)>> {
    if Command::new("sfdisk").arg("--version").output().is_err() {
        return None;
    }
    let output = Command::new("sfdisk")
        .arg("--dump")
        .arg(path)
        .output()
        .expect("run sfdisk");
    assert!(
        output.status.success(),
        "sfdisk --dump failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut parts = Vec::new();
    for line in text.lines().filter(|line| line.contains(" : start=")) {
        let field = |name: &str| -> u64 {
            let rest = line.split(&format!("{name}=")).nth(1).expect(name);
            rest.split(',').next().unwrap().trim().parse().expect(name)
        };
        parts.push((field("start"), field("size")));
    }
    Some(parts)
}

fn disk_len(path: &Path) -> u64 {
    fs::metadata(path).expect("disk output").len()
}

/// Boot (p1), rootfs (p2) and an empty 1 MIB primary (p3) fill the primaries.
/// The extended partition (p4 slot) holds p5 with an image, and p6 and p7 are
/// size-only and not materialized.
fn extended_layout(
    boot: &Path,
    rootfs: &Path,
    data_image: &Path,
) -> Vec<AssemblyDiskPartitionSpec> {
    vec![
        partition(
            "boot",
            Some("fat32-lba"),
            true,
            Some(boot),
            None,
            false,
            true,
        ),
        partition(
            "rootfs",
            Some("linux"),
            false,
            Some(rootfs),
            None,
            false,
            true,
        ),
        partition(
            "scratch",
            Some("linux"),
            false,
            None,
            Some("1M"),
            false,
            true,
        ),
        partition(
            "data",
            Some("linux"),
            false,
            Some(data_image),
            None,
            false,
            true,
        ),
        partition(
            "spare-a",
            Some("linux"),
            false,
            None,
            Some("1M"),
            false,
            false,
        ),
        partition(
            "spare-b",
            Some("linux"),
            false,
            None,
            Some("2M"),
            false,
            false,
        ),
    ]
}

/// Sector layout the planner produces for `extended_layout`:
/// packed: EBRs at 8192..8194, data at 10240 (4 sectors), spare-a at 12288
/// (2048), spare-b at 14336 (4096). Default: EBRs at 8192, 12288, 16384.
const PACKED_LOGICAL: [(u32, u32); 3] = [(10240, 4), (12288, 2048), (14336, 4096)];
const DEFAULT_LOGICAL: [(u32, u32); 3] = [(10240, 4), (14336, 2048), (18432, 4096)];

#[test]
fn packed_ebrs_keep_every_logical_partition_when_truncated() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, rootfs) = write_images(&build_dir);
    let data = build_dir.join("data.img");
    fs::write(&data, vec![0x33; 2000]).expect("data image");
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        Some(AssemblyDiskTruncateSpec::LastData),
        AssemblyEbrPlacementSpec::Packed,
        extended_layout(&boot, &rootfs, &data),
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    // Data ends at 5244928 bytes; rounded up to the 1 MIB alignment.
    assert_eq!(disk_len(&output), 6 * MIB);
    let image = fs::read(&output).expect("disk");
    assert_eq!(&image[510..512], &[0x55, 0xaa]);
    // The table still lists the extended partition and all logical partitions
    // at their full sizes, even though the last two are past the file end.
    let extended = primaries(&image)[3];
    assert_eq!(
        (extended.ptype, extended.start, extended.count),
        (0x05, 8192, 10240)
    );
    assert_eq!(logical_partitions(&image), PACKED_LOGICAL.to_vec());
    assert_eq!(&image[10240 * 512..10240 * 512 + 4], &[0x33; 4]);

    if let Some(parts) = sfdisk_partitions(&output) {
        // sfdisk lists the extended partition too.
        let expected = vec![
            (2048, 600),
            (4096, 2),
            (6144, 2048),
            (8192, 10240),
            (10240, 4),
            (12288, 2048),
            (14336, 4096),
        ];
        assert_eq!(parts, expected, "sfdisk --dump");
    }

    let published = outcome
        .image_results
        .iter()
        .flat_map(|result| result.disk_images.clone())
        .collect::<Vec<_>>();
    assert_eq!(published, vec![output]);
}

#[test]
fn default_ebrs_keep_every_logical_partition_when_truncated() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, rootfs) = write_images(&build_dir);
    let data = build_dir.join("data.img");
    fs::write(&data, vec![0x33; 2000]).expect("data image");
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        Some(AssemblyDiskTruncateSpec::LastData),
        AssemblyEbrPlacementSpec::Default,
        extended_layout(&boot, &rootfs, &data),
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    // The last EBR sits at 16384 sectors, so the file reaches 9 MIB: the
    // EBRs are never left past the end.
    assert_eq!(disk_len(&output), 9 * MIB);
    let image = fs::read(&output).expect("disk");
    assert_eq!(logical_partitions(&image), DEFAULT_LOGICAL.to_vec());
}

#[test]
fn untruncated_disk_keeps_full_size_with_packed_ebrs() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, rootfs) = write_images(&build_dir);
    let data = build_dir.join("data.img");
    fs::write(&data, vec![0x33; 2000]).expect("data image");
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        None,
        AssemblyEbrPlacementSpec::Packed,
        extended_layout(&boot, &rootfs, &data),
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    // The full layout ends where spare-b ends: 14336 + 4096 sectors.
    assert_eq!(disk_len(&output), 18432 * 512);
    let image = fs::read(&output).expect("disk");
    assert_eq!(logical_partitions(&image), PACKED_LOGICAL.to_vec());
}

#[test]
fn truncated_disk_archive_has_the_truncated_length() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, rootfs) = write_images(&build_dir);
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        Some(AssemblyDiskTruncateSpec::LastData),
        AssemblyEbrPlacementSpec::Default,
        vec![
            partition(
                "boot",
                Some("fat32-lba"),
                true,
                Some(&boot),
                None,
                false,
                true,
            ),
            partition(
                "rootfs",
                Some("linux"),
                false,
                Some(&rootfs),
                None,
                false,
                true,
            ),
            partition(
                "data",
                Some("linux"),
                false,
                None,
                Some("16M"),
                false,
                false,
            ),
        ],
    );

    fs::create_dir_all(Path::new(&spec.workspace.out_dir).join("images")).expect("images dir");
    let outcome = assemble(&mut spec, disk, Some("published.img.xz"));

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(disk_len(&output), 3 * MIB);
    let archive = Path::new(&spec.workspace.out_dir)
        .join("images")
        .join("published.img.xz");
    let decompressed = Command::new("xz")
        .arg("-dc")
        .arg(&archive)
        .output()
        .expect("decompress");
    assert!(decompressed.status.success());
    assert_eq!(decompressed.stdout, fs::read(&output).expect("disk"));
}

#[test]
fn wiped_partition_counts_its_full_size_when_truncated() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, _rootfs) = write_images(&build_dir);
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        Some(AssemblyDiskTruncateSpec::LastData),
        AssemblyEbrPlacementSpec::Default,
        vec![
            partition(
                "boot",
                Some("fat32-lba"),
                true,
                Some(&boot),
                None,
                false,
                true,
            ),
            partition("slot-b", Some("linux"), false, None, Some("4M"), true, true),
        ],
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    // slot-b starts at 2 MIB and is 4 MIB: the file ends at 6 MIB.
    assert_eq!(disk_len(&output), 6 * MIB);
    let image = fs::read(&output).expect("disk");
    assert_eq!(primaries(&image)[1].count, 8192);
}

#[test]
fn oversized_image_still_errors_when_truncating() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, _rootfs) = write_images(&build_dir);
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        Some(AssemblyDiskTruncateSpec::LastData),
        AssemblyEbrPlacementSpec::Default,
        vec![partition(
            "boot",
            Some("fat32-lba"),
            true,
            Some(&boot),
            Some("256K"),
            false,
            true,
        )],
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(!outcome.errors.is_empty());
    let message = &outcome.errors[0].message;
    assert!(message.contains("partition 'boot'"), "{message}");
    assert!(
        message.contains("larger than the partition size"),
        "{message}"
    );
    assert!(
        !output.exists(),
        "no disk is written for an oversized image"
    );
}

#[test]
fn unmaterialized_partition_with_an_image_is_refused() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let (boot, _rootfs) = write_images(&build_dir);
    let output = build_dir.join("out").join("sdcard.img");
    let disk = mbr_disk(
        &output,
        None,
        AssemblyEbrPlacementSpec::Default,
        vec![partition(
            "boot",
            Some("fat32-lba"),
            true,
            Some(&boot),
            Some("1M"),
            false,
            false,
        )],
    );

    let outcome = assemble(&mut spec, disk, None);

    assert!(!outcome.errors.is_empty());
    let message = &outcome.errors[0].message;
    assert!(message.contains("partition 'boot'"), "{message}");
    assert!(message.contains("not materialized"), "{message}");
}
