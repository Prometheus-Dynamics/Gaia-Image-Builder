use super::*;
use std::fs;
use std::path::PathBuf;

#[test]
fn an_assembly_is_recorded_under_the_fingerprint_its_inputs_settled_to() {
    // The build rewrites an assembly input during the run, after the plan
    // was made. The saved fingerprint must be the one the input has after
    // the run, or the next preview reruns an assembly nothing changed.
    let mut spec = test_spec();
    fs::create_dir_all(&spec.workspace.build_dir).expect("build dir");
    let input = PathBuf::from(&spec.workspace.build_dir).join("rootfs.ext4");
    fs::write(&input, "built").expect("input");
    spec.image.assembly = Some(gaia_spec::ImageAssemblySpec {
        archives: vec![gaia_spec::AssemblyArchiveSpec {
            id: "update".into(),
            output: "$assembly.out/update.tar".into(),
            members: vec![gaia_spec::AssemblyArchiveMemberSpec {
                name: "rootfs.ext4".into(),
                src: Some(input.display().to_string().into()),
                entries: None,
            }],
        }],
        ..gaia_spec::ImageAssemblySpec::default()
    });
    let context = crate::AppContext::with_defaults();
    let plan = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    let planned = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("assembly operation")
        .fingerprint;

    fs::write(&input, "rebuilt by the run").expect("rebuilt input");
    let outcome = ExecutionOutcome {
        completed_ids: plan
            .operations
            .iter()
            .map(|operation| operation.id.clone())
            .collect(),
        ..ExecutionOutcome::default()
    };
    save_reuse_state(&spec, &plan, &outcome, None);

    let state = load_reuse_state(&spec).expect("reuse state");
    let settled = gaia_plan::operation_fingerprint(&spec, &gaia_plan::OperationKind::AssembleImage);
    assert_ne!(planned, settled, "the input really changed");
    assert_eq!(
        state.operation_fingerprints.get("image:assembly"),
        Some(&settled)
    );
}
