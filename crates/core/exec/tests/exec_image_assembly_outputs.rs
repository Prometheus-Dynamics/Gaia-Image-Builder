//! The image outputs of an assembly with several disks: the main raw disk
//! archive (`image.output.disk`), xz transforms, and the other outputs the
//! run publishes under the image output dir.

pub mod support;

use gaia_exec::{ExecutionOutcome, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyDiskPartitionSpec, AssemblyDiskSpec,
    AssemblyPartitionTableSpec, AssemblyTransformKindSpec, ImageAssemblySpec, ResolvedBuildSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{assembly_transform, provider_catalogs, test_spec};

fn xz_available() -> bool {
    Command::new("xz")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
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

fn decompress_xz(path: &Path) -> Vec<u8> {
    let output = Command::new("xz")
        .arg("-dc")
        .arg(path)
        .output()
        .expect("xz -dc");
    assert!(output.status.success(), "xz -dc {}", path.display());
    output.stdout
}

/// A raw disk of one partition whose content is `fill` repeated `len` times.
fn raw_disk(
    id: &str,
    output: &Path,
    partition_image: &Path,
    fill: u8,
    len: usize,
) -> AssemblyDiskSpec {
    fs::write(partition_image, vec![fill; len]).expect("partition image");
    AssemblyDiskSpec {
        id: id.into(),
        output: output.display().to_string().into(),
        partition_table: AssemblyPartitionTableSpec::Mbr,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        truncate: None,
        ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
        partitions: vec![AssemblyDiskPartitionSpec {
            name: "rootfs".into(),
            kind: Some("0x83".into()),
            type_alias: None,
            bootable: false,
            image: Some(partition_image.display().to_string().into()),
            size: None,
            wipe: false,
            materialize: true,
            gpt: Default::default(),
        }],
        gpt: Default::default(),
    }
}

/// Two disks: `sdcard` in the image output dir (the A/B disk) and
/// `slots-sdcard` in the work dir, compressed by an xz transform into the
/// output dir, plus a tar archive there.
fn two_disk_spec(scratch: &TempDir, output_disk: Option<&str>) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out_images = PathBuf::from(&spec.workspace.out_dir).join("images");
    let work_slots = build_dir.join("assembly/slots");
    fs::create_dir_all(&work_slots).expect("slots work dir");
    spec.image.output.collect_dir = Some(out_images.display().to_string());
    spec.image.output.archive_name = Some("helios.img.xz".into());
    spec.image.output.disk = output_disk.map(str::to_string);

    let sdcard = raw_disk(
        "sdcard",
        &out_images.join("sdcard.img"),
        &build_dir.join("rootfs-a.bin"),
        0x33,
        4096,
    );
    let slot_disk = raw_disk(
        "slots-sdcard",
        &work_slots.join("slots-sdcard.img"),
        &build_dir.join("data.bin"),
        0x44,
        8192,
    );
    let mut xz = assembly_transform(
        AssemblyTransformKindSpec::Xz,
        work_slots.join("slots-sdcard.img").display(),
        out_images.join("slots-sdcard.img.xz").display(),
    );
    xz.level = Some(6);
    let bundle_member = build_dir.join("manifest.json");
    fs::write(&bundle_member, "{\"schema\":1}\n").expect("member");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        transforms: vec![xz],
        disks: vec![sdcard, slot_disk],
        archives: vec![AssemblyArchiveSpec {
            id: "slots-bundle".into(),
            output: out_images
                .join("slots.slot.tar")
                .display()
                .to_string()
                .into(),
            members: vec![AssemblyArchiveMemberSpec {
                name: "manifest.json".into(),
                src: Some(bundle_member.display().to_string().into()),
                ..AssemblyArchiveMemberSpec::default()
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    spec
}

#[cfg(unix)]
#[test]
fn archive_takes_its_disk_from_image_output_disk() {
    if !xz_available() {
        eprintln!("skipping: xz is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-output-disk");
    let spec = two_disk_spec(&scratch, Some("sdcard"));
    let outcome = run_assembly(&spec);
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);

    let out_images = PathBuf::from(&spec.workspace.out_dir).join("images");
    let sdcard = fs::read(out_images.join("sdcard.img")).expect("sdcard.img");
    assert_eq!(
        decompress_xz(&out_images.join("helios.img.xz")),
        sdcard,
        "the main archive holds the sdcard disk, not the slot disk"
    );

    let primary = outcome
        .image_results
        .iter()
        .find_map(|result| result.archive_path.clone())
        .expect("primary archive");
    assert_eq!(primary, out_images.join("helios.img.xz"));
    let published = outcome
        .image_results
        .iter()
        .flat_map(|result| result.disk_images.clone())
        .collect::<Vec<_>>();
    // The other outputs of the run, under the image output dir.
    assert!(
        published.contains(&out_images.join("slots-sdcard.img.xz")),
        "{published:?}"
    );
    assert!(
        published.contains(&out_images.join("slots.slot.tar")),
        "{published:?}"
    );
    assert!(
        published.contains(&out_images.join("sdcard.img")),
        "{published:?}"
    );
    // The slot disk is built in the work dir: not a published output.
    assert!(
        !published
            .iter()
            .any(|path| path.ends_with("slots-sdcard.img")),
        "{published:?}"
    );
}

#[cfg(unix)]
#[test]
fn several_disks_without_image_output_disk_name_the_option() {
    if !xz_available() {
        eprintln!("skipping: xz is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-output-disk-missing");
    let spec = two_disk_spec(&scratch, None);
    let outcome = run_assembly(&spec);
    let message = outcome
        .errors
        .first()
        .map(|error| error.message.clone())
        .expect("an error");
    assert!(message.contains("image.output.disk"), "{message}");
    assert!(message.contains("'sdcard'"), "{message}");
    assert!(message.contains("'slots-sdcard'"), "{message}");
}

#[cfg(unix)]
#[test]
fn xz_transform_output_decompresses_to_its_source_and_is_deterministic() {
    if !xz_available() {
        eprintln!("skipping: xz is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-xz-transform");
    let mut spec = test_spec(&scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out_images = PathBuf::from(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(out_images.display().to_string());
    let source = build_dir.join("payload.bin");
    let payload: Vec<u8> = (0..1_500_000u32)
        .map(|i| (i.wrapping_mul(31) % 251) as u8)
        .collect();
    fs::create_dir_all(&build_dir).expect("build dir");
    fs::write(&source, &payload).expect("payload");
    let dest = out_images.join("payload.bin.xz");
    let mut xz = assembly_transform(
        AssemblyTransformKindSpec::Xz,
        source.display(),
        dest.display(),
    );
    xz.level = Some(9);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        transforms: vec![xz],
        ..ImageAssemblySpec::default()
    });

    let first = run_assembly(&spec);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let first_bytes = fs::read(&dest).expect("first xz output");
    assert_eq!(decompress_xz(&dest), payload);

    fs::remove_file(&dest).expect("remove xz output");
    let second = run_assembly(&spec);
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(fs::read(&dest).expect("second xz output"), first_bytes);
    let published = second
        .image_results
        .iter()
        .flat_map(|result| result.disk_images.clone())
        .collect::<Vec<_>>();
    assert!(published.contains(&dest), "{published:?}");
}
