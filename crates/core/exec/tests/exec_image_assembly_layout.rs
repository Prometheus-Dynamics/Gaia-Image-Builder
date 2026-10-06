pub mod support;

use gaia_exec::{ExecutionOutcome, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use support::{provider_catalogs, test_spec};

fn partition(
    name: &str,
    type_alias: &str,
    image: Option<&Path>,
    size: Option<&str>,
) -> AssemblyDiskPartitionSpec {
    AssemblyDiskPartitionSpec {
        name: name.into(),
        kind: None,
        type_alias: Some(type_alias.into()),
        bootable: false,
        image: image.map(|path| path.display().to_string().into()),
        size: size.map(str::to_string),
        wipe: false,
    }
}

fn disk(output: &Path, partitions: Vec<AssemblyDiskPartitionSpec>) -> AssemblyDiskSpec {
    AssemblyDiskSpec {
        id: "emmc".into(),
        output: output.display().to_string().into(),
        partition_table: AssemblyPartitionTableSpec::Mbr,
        signature: Some("0x48454c49".into()),
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        partitions,
    }
}

fn run_assembly(spec: &ResolvedBuildSpec) -> ExecutionOutcome {
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

/// (bootable, type, start, sectors) of partition entry `slot` in `sector`.
fn entry(sector: &[u8], slot: usize) -> (u8, u8, u32, u32) {
    let offset = 446 + slot * 16;
    (
        sector[offset],
        sector[offset + 4],
        u32::from_le_bytes(sector[offset + 8..offset + 12].try_into().expect("start")),
        u32::from_le_bytes(sector[offset + 12..offset + 16].try_into().expect("size")),
    )
}

fn sector(disk: &[u8], lba: u32) -> &[u8] {
    &disk[lba as usize * 512..(lba as usize + 1) * 512]
}

fn assembly_state(spec: &ResolvedBuildSpec) -> String {
    fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state")
}

#[test]
fn seven_partition_mbr_uses_extended_partition_and_ebr_chain() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir).to_path_buf();
    let images = build_dir.join("layout-images");
    fs::create_dir_all(&images).expect("images dir");
    let autoboot = images.join("autoboot.vfat");
    let boot = images.join("boot.vfat");
    let rootfs = images.join("rootfs.ext4");
    let data = images.join("data.ext4");
    fs::write(&autoboot, vec![0xA1; 600]).expect("autoboot");
    fs::write(&boot, vec![0xB0; 3000]).expect("boot");
    fs::write(&rootfs, vec![0x44; 5000]).expect("rootfs");
    fs::write(&data, vec![0x66; 1500]).expect("data");
    let output = build_dir.join("layout-output/emmc.img");
    let mut autoboot_partition = partition("autoboot", "fat32-lba", Some(&autoboot), Some("1M"));
    autoboot_partition.bootable = true;
    let mut root_b = partition("rootfs-b", "linux", None, Some("64M"));
    root_b.wipe = true;
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![disk(
            &output,
            vec![
                autoboot_partition,
                partition("boot-a", "fat32-lba", Some(&boot), Some("2M")),
                partition("boot-b", "fat32-lba", Some(&boot), Some("2M")),
                partition("rootfs-a", "linux", Some(&rootfs), Some("4M")),
                root_b,
                partition("data", "linux", Some(&data), None),
            ],
        )],
        ..ImageAssemblySpec::default()
    });

    let outcome = run_assembly(&spec);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let bytes = fs::read(&output).expect("disk");
    assert_eq!(bytes.len(), 157_699 * 512);
    let mbr = sector(&bytes, 0);
    assert_eq!(&mbr[440..444], &0x48454c49u32.to_le_bytes());
    assert_eq!(&mbr[510..512], &[0x55, 0xaa]);
    assert_eq!(entry(mbr, 0), (0x80, 0x0c, 2048, 2048));
    assert_eq!(entry(mbr, 1), (0x00, 0x0c, 4096, 4096));
    assert_eq!(entry(mbr, 2), (0x00, 0x0c, 8192, 4096));
    // Extended: from the first EBR to the end of the last logical partition.
    assert_eq!(entry(mbr, 3), (0x00, 0x05, 12288, 157_699 - 12288));

    // EBR entry 0 is relative to its EBR; entry 1 is relative to the
    // extended start and spans the next EBR through its logical partition.
    let ebr5 = sector(&bytes, 12288);
    assert_eq!(&ebr5[510..512], &[0x55, 0xaa]);
    assert_eq!(entry(ebr5, 0), (0x00, 0x83, 2048, 8192));
    assert_eq!(entry(ebr5, 1), (0x00, 0x05, 22528 - 12288, 155_648 - 22528));
    let ebr6 = sector(&bytes, 22528);
    assert_eq!(&ebr6[510..512], &[0x55, 0xaa]);
    assert_eq!(entry(ebr6, 0), (0x00, 0x83, 2048, 131_072));
    assert_eq!(
        entry(ebr6, 1),
        (0x00, 0x05, 155_648 - 12288, 157_699 - 155_648)
    );
    let ebr7 = sector(&bytes, 155_648);
    assert_eq!(&ebr7[510..512], &[0x55, 0xaa]);
    assert_eq!(entry(ebr7, 0), (0x00, 0x83, 2048, 3));
    assert_eq!(entry(ebr7, 1), (0, 0, 0, 0));
    assert!(ebr7[446 + 32..510].iter().all(|byte| *byte == 0));

    // Data lands at the aligned start after each EBR.
    for (lba, image) in [
        (2048u32, &autoboot),
        (4096, &boot),
        (8192, &boot),
        (14336, &rootfs),
        (157_696, &data),
    ] {
        let contents = fs::read(image).expect("image");
        let start = lba as usize * 512;
        assert_eq!(
            &bytes[start..start + contents.len()],
            &contents[..],
            "{lba}"
        );
    }
    assert!(
        bytes[24576 * 512..155_648 * 512]
            .iter()
            .all(|byte| *byte == 0)
    );

    let state = assembly_state(&spec);
    assert!(state.contains("disk.1.partition_count=6"), "{state}");
    assert!(state.contains("disk.1.partition.4.number=5"), "{state}");
    assert!(
        state.contains("disk.1.partition.4.ebr_lba=12288"),
        "{state}"
    );
    assert!(state.contains("disk.1.partition.5.empty=true"), "{state}");
    assert!(
        state.contains("disk.1.partition.5.wipe_bytes=1048576"),
        "{state}"
    );
    assert!(state.contains("disk.1.partition.6.number=7"), "{state}");

    assert_sfdisk_layout(
        &output,
        &[
            (1, 2048, 2048, "c"),
            (2, 4096, 4096, "c"),
            (3, 8192, 4096, "c"),
            (4, 12288, 145_411, "5"),
            (5, 14336, 8192, "83"),
            (6, 24576, 131_072, "83"),
            (7, 157_696, 3, "83"),
        ],
    );
}

/// Cross-checks the layout with `sfdisk -d` when it is installed.
fn assert_sfdisk_layout(image: &Path, expected: &[(u32, u64, u64, &str)]) {
    let Ok(output) = Command::new("sfdisk").arg("-d").arg(image).output() else {
        eprintln!("sfdisk not available; skipping sfdisk layout check");
        return;
    };
    assert!(
        output.status.success(),
        "sfdisk -d failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dump = String::from_utf8_lossy(&output.stdout);
    let prefix = image.display().to_string();
    let mut parsed = Vec::new();
    for line in dump.lines() {
        let Some((device, fields)) = line.split_once(" : ") else {
            continue;
        };
        let Some(number) = device
            .trim()
            .strip_prefix(&prefix)
            .and_then(|number| number.parse::<u32>().ok())
        else {
            continue;
        };
        let field = |name: &str| {
            fields
                .split(',')
                .find_map(|field| field.trim().strip_prefix(name))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        parsed.push((
            number,
            field("start=").parse::<u64>().expect("start"),
            field("size=").parse::<u64>().expect("size"),
            field("type="),
        ));
    }
    let expected = expected
        .iter()
        .map(|(number, start, size, kind)| (*number, *start, *size, kind.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(parsed, expected, "{dump}");
}

#[test]
fn partition_size_pads_images_and_empty_partitions_stay_sparse() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir).to_path_buf();
    let images = build_dir.join("sized-images");
    fs::create_dir_all(&images).expect("images dir");
    let boot = images.join("boot.vfat");
    fs::write(&boot, vec![0xB0; 4096]).expect("boot");
    let output = build_dir.join("sized-output/disk.img");
    let mut empty = partition("spare", "linux", None, Some("256M"));
    empty.wipe = true;
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![disk(
            &output,
            vec![
                partition("boot", "fat32-lba", Some(&boot), Some("8M")),
                empty,
            ],
        )],
        ..ImageAssemblySpec::default()
    });

    let outcome = run_assembly(&spec);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let metadata = fs::metadata(&output).expect("disk metadata");
    // boot: 2048 + 16384 sectors; spare: 256 MiB starting at 18432.
    assert_eq!(metadata.len(), (18432 + 524_288) * 512);
    let bytes = fs::read(&output).expect("disk");
    assert_eq!(entry(&bytes, 0), (0x00, 0x0c, 2048, 16384));
    assert_eq!(entry(&bytes, 1), (0x00, 0x83, 18432, 524_288));
    assert_eq!(entry(&bytes, 2), (0, 0, 0, 0));
    assert_eq!(&bytes[2048 * 512..2048 * 512 + 4096], &[0xB0; 4096][..]);
    assert!(bytes[2048 * 512 + 4096..].iter().all(|byte| *byte == 0));
    let state = assembly_state(&spec);
    assert!(state.contains("disk.1.partition.2.empty=true"), "{state}");
    assert!(
        state.contains("disk.1.partition.2.wipe_bytes=1048576"),
        "{state}"
    );
    assert!(!state.contains("disk.1.partition.1.number"), "{state}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let allocated = metadata.blocks() * 512;
        if allocated == 0 || allocated >= metadata.len() {
            eprintln!("filesystem does not report sparse allocation; skipping sparse check");
        } else {
            // Only the MBR, the boot image and the 1 MiB wipe are written.
            assert!(
                allocated < 16 * 1024 * 1024,
                "allocated {allocated} of {} bytes",
                metadata.len()
            );
        }
    }
}

#[test]
fn partition_image_larger_than_size_fails_before_writing_disk() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir).to_path_buf();
    let images = build_dir.join("too-big-images");
    fs::create_dir_all(&images).expect("images dir");
    let rootfs = images.join("rootfs.ext4");
    fs::write(&rootfs, vec![0x44; 2 * 1024 * 1024 + 1]).expect("rootfs");
    let output = build_dir.join("too-big-output/disk.img");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![disk(
            &output,
            vec![partition("rootfs", "linux", Some(&rootfs), Some("2M"))],
        )],
        ..ImageAssemblySpec::default()
    });

    let outcome = run_assembly(&spec);

    assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
    let message = &outcome.errors[0].message;
    assert!(
        message.contains("is 2097153 bytes, larger than the partition size 2M"),
        "{message}"
    );
    assert!(!output.exists());
}
