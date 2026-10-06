//! The assembled disk, not an intermediate rootfs image, is what an `.img`
//! archive holds and what the run reports as its primary image output.
pub mod support;

use gaia_exec::{ExecutionOutcome, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, test_spec};

/// A spec whose assembly builds one MBR disk, `sdcard.img`, in the collect
/// dir, with `archive_name` set to `archive_name`.
fn single_disk_spec(archive_name: Option<&str>) -> (ResolvedBuildSpec, PathBuf, PathBuf) {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    spec.image.output.archive_name = archive_name.map(str::to_string);
    let rootfs_image = build_dir.join("rootfs.ext4");
    fs::create_dir_all(&build_dir).expect("build dir");
    fs::write(&rootfs_image, vec![0x22; 2048]).expect("rootfs image");
    let sdcard = collect_dir.join("sdcard.img");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: sdcard.display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: Some("GAIA".into()),
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: Some("0x83".into()),
                type_alias: None,
                bootable: false,
                image: Some(rootfs_image.display().to_string().into()),
                size: None,
                wipe: false,
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    (spec, collect_dir, sdcard)
}

fn assemble(spec: &ResolvedBuildSpec) -> ExecutionOutcome {
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

fn primary_output(outcome: &ExecutionOutcome) -> Option<PathBuf> {
    outcome
        .image_results
        .iter()
        .find_map(|result| result.archive_path.clone())
}

#[test]
fn uncompressed_img_archive_is_the_assembled_disk() {
    let (spec, collect_dir, sdcard) = single_disk_spec(Some("published.img"));

    let outcome = assemble(&spec);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let archive = collect_dir.join("published.img");
    assert_eq!(
        fs::read(&archive).expect("archive"),
        fs::read(&sdcard).expect("sdcard output")
    );
    assert_eq!(primary_output(&outcome), Some(archive));
}

#[test]
fn single_disk_is_the_primary_output_without_an_archive() {
    let (spec, _, sdcard) = single_disk_spec(None);

    let outcome = assemble(&spec);

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(primary_output(&outcome), Some(sdcard));
}
