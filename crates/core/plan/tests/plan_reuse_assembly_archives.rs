pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyDiskPartitionSpec, AssemblyDiskSpec,
    AssemblyPartitionTableSpec, ImageAssemblySpec, ResolvedBuildSpec,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec_with_root, unique_dir};

fn reuse_state_for_assembly(spec: &ResolvedBuildSpec) -> ReuseState {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["image:assembly"];
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: reused_ids
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: Default::default(),
    }
}

fn assembly_reuse_code(spec: &ResolvedBuildSpec, state: &ReuseState) -> Option<String> {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(state),
    );
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .map(|operation| match &operation.reuse {
            OperationReuse::Execute(reason) => reason.code.to_string(),
            other => format!("{other:?}"),
        })
}

fn spec_with_archive(name: &str) -> (ResolvedBuildSpec, PathBuf) {
    let root_dir = unique_dir(name);
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = test_spec_with_root(root_dir);
    let member = gaia_spec::resolve_workspace_path(&spec.workspace, "@assets/rootfs.ext4")
        .expect("asset path");
    fs::create_dir_all(member.parent().expect("asset parent")).expect("assets dir");
    fs::write(&member, "one").expect("member");
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("image-assembly.state"),
        "kind=image-assembly\ncompleted_archive_count=1\n",
    )
    .expect("assembly runtime state");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "emmc".into(),
            output: "$assembly.out/emmc.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "spare".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: None,
                size: Some("2G".into()),
                wipe: true,
            }],
        }],
        archives: vec![AssemblyArchiveSpec {
            id: "update".into(),
            output: "$assembly.out/update.tar".into(),
            members: vec![
                AssemblyArchiveMemberSpec {
                    name: "manifest.env".into(),
                    src: None,
                    entries: Some(vec![(
                        "IMAGE_SHA256".into(),
                        "${assembly.sha256:$assembly.out/emmc.img}".into(),
                    )]),
                },
                AssemblyArchiveMemberSpec {
                    name: "rootfs.ext4".into(),
                    src: Some("@assets/rootfs.ext4".into()),
                    entries: None,
                },
            ],
        }],
        ..ImageAssemblySpec::default()
    });
    (spec, member)
}

#[test]
fn plan_rebuilds_assembly_when_archive_member_changes() {
    let (spec, member) = spec_with_archive("gaia-plan-assembly-archive-member");
    let state = reuse_state_for_assembly(&spec);

    fs::write(&member, "two").expect("updated member");

    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch")
    );
}

#[test]
fn unchanged_archive_and_empty_partition_inputs_keep_the_fingerprint() {
    let (spec, _member) = spec_with_archive("gaia-plan-assembly-archive-stable");
    let state = reuse_state_for_assembly(&spec);

    assert_ne!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch")
    );
}
