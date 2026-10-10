pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use std::fs;
use std::path::Path;
use support::{assembly_file, assembly_tree, provider_catalogs, test_spec};

/// A disk built from a file in a tree, published to the collect dir. The
/// tree is an intermediate: in RAM when the work dir says so.
fn spec_with_work_dir(work_dir: &str) -> ResolvedBuildSpec {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let out_dir = Path::new(&spec.workspace.out_dir);
    spec.image.output.collect_dir = Some(out_dir.join("images").display().to_string());
    let source = build_dir.join("sources/config.txt");
    fs::create_dir_all(source.parent().expect("source dir")).expect("source dir");
    fs::write(&source, "config\n".repeat(4096)).expect("source");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(work_dir.into()),
        trees: vec![assembly_tree("boot", "$assembly.work/boot")],
        files: vec![assembly_file(
            "boot",
            source.display().to_string(),
            "config.txt",
        )],
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: Some("0x48454c49".into()),
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            truncate: None,
            ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "boot".into(),
                kind: Some("0x83".into()),
                type_alias: None,
                bootable: true,
                image: Some("$assembly.work/boot/config.txt".into()),
                size: Some("1M".into()),
                wipe: false,
                materialize: true,
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    spec
}

fn assemble(spec: &ResolvedBuildSpec) {
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

#[test]
fn ram_work_dir_publishes_the_same_disk_as_disk_mode() {
    let ram = spec_with_work_dir("ram");
    let disk = spec_with_work_dir("disk");
    assemble(&ram);
    assemble(&disk);

    let published = |spec: &ResolvedBuildSpec| {
        Path::new(
            spec.image
                .output
                .collect_dir
                .as_deref()
                .expect("collect dir"),
        )
        .join("sdcard.img")
    };
    let ram_disk = published(&ram);
    let disk_disk = published(&disk);
    assert_eq!(
        fs::read(&ram_disk).expect("ram disk"),
        fs::read(&disk_disk).expect("disk disk"),
        "the published raw disk must be the same bytes in RAM and disk modes"
    );
    // The tree is an intermediate and does not land in the collect dir.
    assert!(
        !Path::new(ram.image.output.collect_dir.as_deref().unwrap_or_default())
            .join("boot")
            .exists()
    );
}
