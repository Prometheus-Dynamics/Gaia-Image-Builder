//! Tests of the fingerprint explanations `gaia preview` gives: named inputs
//! that changed, and the fallback for state recorded without them.

use super::tests::{record_reuse_state, scratch_root};
use super::*;
use crate::commands::load_reuse_state;
use gaia_spec::{BuildrootImageSpec, ImageDefinition};
use std::path::Path;

fn buildroot_spec(root: &Path, overrides: &[(&str, &str)]) -> gaia_spec::ResolvedBuildSpec {
    let mut spec = gaia_spec::ResolvedBuildSpec::new("app-overrides");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.workspace.out_dir = "out".into();
    spec.image.definition = ImageDefinition::Buildroot(BuildrootImageSpec {
        defconfig: Some("test_defconfig".into()),
        config_overrides: overrides
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        ..BuildrootImageSpec::default()
    });
    spec
}

/// Records a finished run of `plan` through the real save path, so the
/// state and its components sidecar are both written.
fn record_finished_run(spec: &gaia_spec::ResolvedBuildSpec, plan: &gaia_plan::ExecutionPlan) {
    let outcome = gaia_exec::ExecutionOutcome {
        completed_ids: plan
            .operations
            .iter()
            .map(|operation| operation.id.clone())
            .collect(),
        ..gaia_exec::ExecutionOutcome::default()
    };
    crate::commands::state::save_reuse_state(spec, plan, &outcome, None);
}

#[test]
fn a_changed_override_key_is_previewed_by_name() {
    let scratch = scratch_root("override-key");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let spec = buildroot_spec(
        &root,
        &[
            ("BR2_TARGET_GENERIC_HOSTNAME", "alpha"),
            ("BR2_TARGET_GENERIC_ISSUE", "one"),
        ],
    );
    let baseline = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    record_finished_run(&spec, &baseline);

    let changed = buildroot_spec(
        &root,
        &[
            ("BR2_TARGET_GENERIC_HOSTNAME", "alpha"),
            ("BR2_TARGET_GENERIC_ISSUE", "two"),
        ],
    );
    let report = preview_resolved(
        &context,
        &changed,
        &[],
        &RebuildRequest::default(),
        false,
        false,
    )
    .expect("the preview of a valid build succeeds");

    let image = report
        .operations
        .iter()
        .find(|operation| operation.id == "image:build")
        .expect("image operation");
    assert!(image.executes);
    assert!(
        image
            .reason
            .contains("config_overrides changed (BR2_TARGET_GENERIC_ISSUE)"),
        "{}",
        image.reason
    );
    assert!(
        !image.reason.contains("no detail recorded"),
        "{}",
        image.reason
    );
    assert!(!image.reason.contains("HOSTNAME"), "{}", image.reason);
    for operation in report
        .operations
        .iter()
        .filter(|operation| !operation.executes)
    {
        assert_eq!(operation.reason, "reused from state-file", "{operation:?}");
    }
}

#[test]
fn a_state_recorded_without_details_says_no_detail_recorded() {
    let scratch = scratch_root("no-details");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let spec = buildroot_spec(&root, &[("BR2_TARGET_GENERIC_ISSUE", "one")]);
    let baseline = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    // The state format of older Gaia: no components sidecar.
    record_reuse_state(&spec, &baseline);

    let changed = buildroot_spec(&root, &[("BR2_TARGET_GENERIC_ISSUE", "two")]);
    let report = preview_resolved(
        &context,
        &changed,
        &[],
        &RebuildRequest::default(),
        false,
        false,
    )
    .expect("the preview of a valid build succeeds");

    let image = report
        .operations
        .iter()
        .find(|operation| operation.id == "image:build")
        .expect("image operation");
    assert!(
        image
            .reason
            .contains("fingerprint changed (no detail recorded)"),
        "{}",
        image.reason
    );
}

#[test]
fn a_changed_build_env_key_is_previewed_by_name() {
    let scratch = scratch_root("build-env");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let java = |gradle_home: &str| {
        let mut spec = gaia_spec::ResolvedBuildSpec::new("app-env");
        spec.workspace.root_dir = root.display().to_string();
        spec.workspace.build_dir = "build".into();
        spec.workspace.out_dir = "out".into();
        spec.image.definition = ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        });
        spec.artifacts.push(gaia_spec::ArtifactSpec::new(
            "devmgrd",
            gaia_spec::ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                build_target: "devmgrd".into(),
                build_args: Vec::new(),
                build_command: vec!["gradle".into(), "build".into()],
                build_env: vec![
                    ("GRADLE_USER_HOME".into(), gradle_home.into()),
                    ("JAVA_OPTS".into(), "-Xmx1g".into()),
                ],
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/devmgrd.jar".into(),
            },
        ));
        spec
    };
    let spec = java("/cache/a");
    let baseline = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    record_finished_run(&spec, &baseline);

    let report = preview_resolved(
        &context,
        &java("/cache/b"),
        &[],
        &RebuildRequest::default(),
        false,
        false,
    )
    .expect("the preview of a valid build succeeds");

    let artifact = report
        .operations
        .iter()
        .find(|operation| operation.id == "artifact:devmgrd")
        .expect("artifact operation");
    assert!(artifact.executes);
    assert!(
        artifact
            .reason
            .contains("build_env changed (GRADLE_USER_HOME)"),
        "{}",
        artifact.reason
    );
    assert!(
        !artifact.reason.contains("JAVA_OPTS"),
        "{}",
        artifact.reason
    );
}

/// What `gaia run` does: the reasons come from the plan explained before the
/// run saves its state, and the rebuild-reasons report carries them.
#[test]
fn a_run_names_the_changed_input_in_its_rebuild_reasons() {
    let scratch = scratch_root("run-reasons");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let spec = buildroot_spec(&root, &[("BR2_TARGET_GENERIC_ISSUE", "one")]);
    let baseline = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    record_finished_run(&spec, &baseline);

    let changed = buildroot_spec(&root, &[("BR2_TARGET_GENERIC_ISSUE", "two")]);
    let reuse_state = load_reuse_state(&changed);
    let recorded = recorded_for_explanation(&changed, reuse_state.as_ref(), ReuseScope::Build);
    let mut plan = gaia_plan::plan_build_with_rebuilds(
        &changed,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        reuse_state.as_ref(),
        &RebuildRequest::default(),
    );
    explain_rebuild_reasons(&changed, &mut plan, reuse_state.as_ref(), &recorded);

    let outcome = gaia_exec::ExecutionOutcome::default();
    let reasons = gaia_report::render_rebuild_reasons(&changed, &plan, &outcome);
    let image = reasons
        .iter()
        .find(|reason| reason.operation_id == "image:build")
        .expect("image rebuild reason");
    assert_eq!(image.code, "operation_fingerprint_mismatch");
    assert!(
        image
            .message
            .contains("config_overrides changed (BR2_TARGET_GENERIC_ISSUE)"),
        "{}",
        image.message
    );
    assert!(
        !image.message.contains("no detail recorded"),
        "{}",
        image.message
    );
}

/// `gaia plan` names the changed input the way `run` and `preview` do.
#[test]
fn a_changed_override_key_is_planned_by_name() {
    let scratch = scratch_root("plan-override-key");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let spec = buildroot_spec(
        &root,
        &[
            ("BR2_TARGET_GENERIC_HOSTNAME", "alpha"),
            ("BR2_TARGET_GENERIC_ISSUE", "one"),
        ],
    );
    let baseline = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    record_finished_run(&spec, &baseline);

    let changed = buildroot_spec(
        &root,
        &[
            ("BR2_TARGET_GENERIC_HOSTNAME", "alpha"),
            ("BR2_TARGET_GENERIC_ISSUE", "two"),
        ],
    );
    let outcome = crate::commands::plan::plan_resolved(
        &context,
        changed,
        &[],
        &RebuildRequest::default(),
        crate::commands::variants::ReuseScope::Build,
        None,
    );
    let CommandOutcome::Planned { plan, .. } = outcome else {
        panic!("the plan of a valid build succeeds");
    };
    let image = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image operation");
    let gaia_plan::OperationReuse::Execute(reason) = &image.reuse else {
        panic!("a changed override executes the image build");
    };
    assert!(
        reason
            .message
            .contains("config_overrides changed (BR2_TARGET_GENERIC_ISSUE)"),
        "{}",
        reason.message
    );
    assert!(
        !reason.message.contains("no detail recorded"),
        "{}",
        reason.message
    );
}

/// A variant's shared operations were recorded by the shared store, so their
/// inputs are read from there; its own operations from its own sidecar.
#[test]
fn a_variant_reads_the_recorded_inputs_of_its_shared_operations() {
    use crate::commands::variants::ReuseScope;
    let scratch = scratch_root("variant-shared-components");
    let root = scratch.join("root");
    let context = AppContext::with_defaults();
    let mut spec = buildroot_spec(&root, &[("BR2_TARGET_GENERIC_ISSUE", "one")]);
    spec.selection.variant = Some("v2".into());
    let shared_spec = crate::commands::variants::shared_store_spec(&spec);
    let baseline = gaia_plan::plan_build(
        &shared_spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    record_finished_run(&shared_spec, &baseline);
    let state = gaia_plan::ReuseState::default();

    let shared = std::collections::BTreeSet::from(["image:build".to_string()]);
    let recorded =
        recorded_for_explanation(&spec, Some(&state), ReuseScope::Variant { shared: &shared });
    assert!(
        recorded.contains_key("image:build"),
        "the shared store's record of a shared operation is read: {:?}",
        recorded.keys().collect::<Vec<_>>()
    );

    let own = std::collections::BTreeSet::new();
    let recorded =
        recorded_for_explanation(&spec, Some(&state), ReuseScope::Variant { shared: &own });
    assert!(
        !recorded.contains_key("image:build"),
        "an operation that is not shared is read from the variant's own sidecar"
    );
}
