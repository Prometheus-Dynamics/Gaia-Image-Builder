//! GPT disks: the protective MBR, both headers and entry arrays with their
//! CRCs, partition offsets, types, names and GUIDs, and reproducibility.
//! Checked byte by byte, and with `sgdisk -v` and `sfdisk --json` where
//! they are installed.

use super::*;
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyEbrPlacementSpec,
    AssemblyGptPartitionSpec, AssemblyPartitionTableSpec, GptGuid,
};
use gaia_testkit::TempDir;
use std::fs;
use std::process::Command;

const SECTOR: usize = 512;

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn partition(name: &str, alias: Option<&str>, image: Option<&str>) -> AssemblyDiskPartitionSpec {
    AssemblyDiskPartitionSpec {
        name: name.into(),
        kind: None,
        type_alias: alias.map(Into::into),
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

/// An ESP of 1 MiB holding `esp.img`, an arm64 root holding `root.img`,
/// an empty wiped data partition, and one with a raw type GUID.
fn gpt_disk() -> AssemblyDiskSpec {
    let mut esp = partition("ESP", Some("esp"), Some("@assets/esp.img"));
    esp.size = Some("1M".into());
    esp.bootable = true;
    let root = partition("root-a", Some("linux-root-arm64"), Some("@assets/root.img"));
    let mut data = partition("data", None, None);
    data.size = Some("2M".into());
    data.wipe = true;
    let mut raw = partition("räw", None, None);
    raw.kind = Some("3b8f8425-20e0-4f3b-907f-1a25a76f98e8".into());
    raw.size = Some("512K".into());
    AssemblyDiskSpec {
        id: "disk".into(),
        output: "$assembly.out/disk.img".into(),
        partition_table: AssemblyPartitionTableSpec::Gpt,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        truncate: None,
        ebr_placement: AssemblyEbrPlacementSpec::Default,
        gpt: Default::default(),
        partitions: vec![esp, root, data, raw],
    }
}

fn write_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("esp.img"), vec![0xe5u8; 3000]).expect("esp");
    fs::write(assets.join("root.img"), vec![0x52u8; 700_000]).expect("root");
}

fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
}

fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("u64"))
}

/// Checks one header: signature, CRCs, and where it says things are.
fn check_header(disk: &[u8], lba: usize, alternate: u64, entries_lba: u64) -> Vec<u8> {
    let header = &disk[lba * SECTOR..(lba + 1) * SECTOR];
    assert_eq!(&header[0..8], b"EFI PART");
    let mut zeroed = header[..92].to_vec();
    zeroed[16..20].fill(0);
    assert_eq!(le_u32(header, 16), crc32(&zeroed), "header CRC");
    assert_eq!(le_u64(header, 24), lba as u64);
    assert_eq!(le_u64(header, 32), alternate);
    assert_eq!(le_u64(header, 72), entries_lba);
    let entries = &disk[entries_lba as usize * SECTOR..][..128 * 128];
    assert_eq!(le_u32(header, 88), crc32(entries), "entries CRC");
    header.to_vec()
}

#[test]
fn gpt_disk_has_both_tables_the_partitions_and_the_same_bytes_each_run() {
    let scratch = TempDir::new("gaia-exec-gpt");
    let root = scratch.join("gaia-assembly-gpt");
    write_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        disks: vec![gpt_disk()],
        ..ImageAssemblySpec::default()
    });
    let operation = OperationId::image_assembly();
    let first =
        stage_image_assembly_with(&spec, &operation, None, disk_env_panic).expect("gpt disk");
    let image = root.join("out/images/disk.img");
    let disk = fs::read(&image).expect("disk image");

    // ESP 2048..4095, root at 4096 for 1368 sectors, data at the next MiB
    // for 4096, the raw one after it for 1024; then the backup, rounded up.
    let starts = [2048u64, 4096, 6144, 10240];
    let counts = [2048u64, 700_000u64.div_ceil(512), 4096, 1024];
    let total = (10240u64 + 1024 + 33).div_ceil(2048) * 2048;
    assert_eq!(disk.len() as u64, total * 512);

    // The protective MBR covers the disk with one 0xEE entry.
    assert_eq!(disk[446 + 4], 0xee);
    assert_eq!(le_u32(&disk, 446 + 8), 1);
    assert_eq!(le_u32(&disk, 446 + 12) as u64, total - 1);
    assert_eq!(&disk[510..512], &[0x55, 0xaa]);

    let primary = check_header(&disk, 1, total - 1, 2);
    let backup = check_header(&disk, total as usize - 1, 1, total - 33);
    assert_eq!(le_u64(&primary, 40), 34, "first usable LBA");
    assert_eq!(le_u64(&primary, 48), total - 34, "last usable LBA");
    assert_eq!(&primary[56..72], &backup[56..72], "one disk GUID");

    let entry = |index: usize| &disk[2 * SECTOR + index * 128..][..128];
    let expected_types = [
        "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
        "B921B045-1DF0-41C3-AF44-4C6F280D3FAE",
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
        "3B8F8425-20E0-4F3B-907F-1A25A76F98E8",
    ];
    let mut partition_guids = Vec::new();
    for index in 0..4 {
        let entry = entry(index);
        let kind = GptGuid::parse(expected_types[index]).expect("type");
        assert_eq!(&entry[0..16], &kind.to_disk_bytes(), "type of {index}");
        assert_eq!(le_u64(entry, 32), starts[index]);
        assert_eq!(le_u64(entry, 40), starts[index] + counts[index] - 1);
        partition_guids.push(entry[16..32].to_vec());
    }
    assert!(
        entry(4).iter().all(|byte| *byte == 0),
        "unused entries are zero"
    );
    assert_eq!(
        le_u64(entry(0), 48),
        1 << 2,
        "bootable is legacy BIOS bootable"
    );
    // Three UTF-16 units, then zeros to the end of the 72-byte field.
    assert!(entry(3)[62..128].iter().all(|byte| *byte == 0));
    let name: Vec<u16> = entry(3)[56..62]
        .chunks(2)
        .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
        .collect();
    assert_eq!(String::from_utf16(&name).expect("name"), "räw");
    partition_guids.dedup();
    assert_eq!(partition_guids.len(), 4, "each partition its own GUID");

    // The images are at their partitions; the wiped one is zero.
    assert!(
        disk[2048 * SECTOR..][..3000]
            .iter()
            .all(|byte| *byte == 0xe5)
    );
    assert!(
        disk[4096 * SECTOR..][..700_000]
            .iter()
            .all(|byte| *byte == 0x52)
    );
    assert!(
        disk[6144 * SECTOR..][..1024 * 1024]
            .iter()
            .all(|byte| *byte == 0)
    );

    let state = first.state.render();
    assert!(state.contains("partition_table=gpt"), "{state}");
    assert!(state.contains(".disk_guid="), "{state}");
    assert!(
        state.contains("type=B921B045-1DF0-41C3-AF44-4C6F280D3FAE"),
        "{state}"
    );

    stage_image_assembly_with(&spec, &operation, None, disk_env_panic).expect("second gpt disk");
    assert_eq!(fs::read(&image).expect("second image"), disk);

    if let Ok(output) = Command::new("sgdisk").arg("-v").arg(&image).output() {
        let report = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && report.contains("No problems found"),
            "sgdisk -v: {report}"
        );
    } else {
        eprintln!("skipped sgdisk -v: sgdisk not installed");
    }
    if let Ok(output) = Command::new("sfdisk").arg("--json").arg(&image).output() {
        let report = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "sfdisk --json: {report}");
        assert!(report.contains("\"label\": \"gpt\""), "{report}");
        for (start, kind) in starts.iter().zip(expected_types) {
            assert!(report.contains(&format!("\"start\": {start},")), "{report}");
            assert!(report.contains(kind), "{report}");
        }
        assert!(report.contains("\"name\": \"root-a\""), "{report}");
    } else {
        eprintln!("skipped sfdisk --json: sfdisk not installed");
    }
}

#[test]
fn gpt_guids_from_the_spec_are_written_as_given() {
    let scratch = TempDir::new("gaia-exec-gpt-guids");
    let root = scratch.join("gaia-assembly-gpt-guids");
    write_assets(&root);
    let mut disk = gpt_disk();
    disk.gpt.disk_guid = Some("01234567-89ab-4cde-8f01-23456789abcd".into());
    disk.partitions[1].gpt.guid = Some("fedcba98-7654-4321-8fed-cba987654321".into());
    disk.first_lba = Some(34);
    disk.alignment_lba = Some(8);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        disks: vec![disk],
        ..ImageAssemblySpec::default()
    });
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("gpt disk");
    let bytes = fs::read(root.join("out/images/disk.img")).expect("disk image");
    let disk_guid = GptGuid::parse("01234567-89ab-4cde-8f01-23456789abcd").expect("guid");
    assert_eq!(&bytes[SECTOR + 56..SECTOR + 72], &disk_guid.to_disk_bytes());
    let partition_guid = GptGuid::parse("fedcba98-7654-4321-8fed-cba987654321").expect("guid");
    assert_eq!(
        &bytes[2 * SECTOR + 128 + 16..][..16],
        &partition_guid.to_disk_bytes()
    );
    // first_lba 34 aligned to 8 is 40.
    assert_eq!(le_u64(&bytes, 2 * SECTOR + 32), 40);
    let state = summary.state.render();
    assert!(
        state.contains("disk_guid=01234567-89AB-4CDE-8F01-23456789ABCD"),
        "{state}"
    );
    assert!(
        state.contains("guid=FEDCBA98-7654-4321-8FED-CBA987654321"),
        "{state}"
    );
}

#[test]
fn gpt_disk_rejects_a_first_lba_inside_its_tables() {
    let scratch = TempDir::new("gaia-exec-gpt-first-lba");
    let root = scratch.join("gaia-assembly-gpt-first-lba");
    write_assets(&root);
    let mut disk = gpt_disk();
    disk.first_lba = Some(16);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        disks: vec![disk],
        ..ImageAssemblySpec::default()
    });
    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("first_lba 16");
    assert!(error.message.contains("at least 34"), "{}", error.message);
}
