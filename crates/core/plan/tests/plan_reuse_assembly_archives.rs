pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, ReuseState, fingerprint_change_detail, operation_components,
    operation_output_signature, plan_build, plan_build_with_reuse_state, recorded_fingerprint,
    spec_fingerprint,
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
            truncate: None,
            ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "spare".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: None,
                size: Some("2G".into()),
                wipe: true,
                materialize: true,
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

/// The state a finished run records: the fingerprint of each completed
/// operation as the state left by the run has it, see `recorded_fingerprint`.
fn state_recorded_after_run(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    fingerprint_of: impl Fn(&gaia_plan::PlannedOperation) -> u64,
) -> ReuseState {
    let operation = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("assembly operation");
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: ["image:assembly".to_string()].into_iter().collect(),
        operation_fingerprints: [("image:assembly".to_string(), fingerprint_of(operation))]
            .into_iter()
            .collect(),
        operation_output_signatures: operation_output_signature(spec, &operation.kind)
            .map(|signature| ("image:assembly".to_string(), signature))
            .into_iter()
            .collect(),
        operation_input_signatures: Default::default(),
    }
}

fn plan_of(spec: &ResolvedBuildSpec) -> ExecutionPlan {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog)
}

#[test]
fn an_assembly_recorded_after_its_input_settles_is_not_rerun() {
    // The build rewrites an assembly input during the run. The planned
    // fingerprint still holds the input as it was before the run, so the
    // state must hold the fingerprint of the input as the run left it.
    let (spec, member) = spec_with_archive("gaia-plan-assembly-settled");
    let planned_before = plan_of(&spec);
    fs::write(&member, "rewritten by the build").expect("rewritten input");

    let recorded = state_recorded_after_run(&spec, &plan_of(&spec), recorded_fingerprint_of(&spec));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&recorded),
    );
    let reason = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .map(|operation| match &operation.reuse {
            OperationReuse::Execute(reason) => reason.code.to_string(),
            other => format!("{other:?}"),
        });
    assert_ne!(reason.as_deref(), Some("operation_fingerprint_mismatch"));

    // Control: the fingerprint planned before the run is what the bug recorded.
    let stale = state_recorded_after_run(&spec, &planned_before, |operation| operation.fingerprint);
    assert_eq!(
        assembly_reuse_code(&spec, &stale).as_deref(),
        Some("operation_fingerprint_mismatch")
    );
}

fn recorded_fingerprint_of(
    spec: &ResolvedBuildSpec,
) -> impl Fn(&gaia_plan::PlannedOperation) -> u64 + '_ {
    move |operation| recorded_fingerprint(spec, operation)
}

#[test]
fn a_changed_assembly_input_is_named_in_the_fingerprint_change() {
    let (spec, member) = spec_with_archive("gaia-plan-assembly-named");
    let before_plan = plan_of(&spec);
    let before = components_of_assembly(&spec, &before_plan);

    fs::write(&member, "two").expect("updated member");

    let after_plan = plan_of(&spec);
    let after = components_of_assembly(&spec, &after_plan);
    let message = fingerprint_change_detail("image:assembly", &before, &after)
        .expect("a changed input is named");
    assert!(
        message.contains("assembly inputs changed (archive update/rootfs.ext4 "),
        "{message}"
    );
    assert!(!message.contains("no named input"), "{message}");
}

fn components_of_assembly(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> Vec<(String, String)> {
    let operation = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("assembly operation");
    operation_components(spec, plan, operation)
}
