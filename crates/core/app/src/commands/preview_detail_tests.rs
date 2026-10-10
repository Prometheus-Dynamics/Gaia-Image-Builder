//! Tests of the fingerprint explanations `gaia preview` gives: named inputs
//! that changed, and the fallback for state recorded without them.

use super::tests::{record_reuse_state, scratch_root};
use super::*;
use gaia_spec::{BuildrootImageSpec, ImageDefinition};
use std::fs;
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
    let root = scratch_root("override-key");
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
    let report = preview_resolved(&context, &changed, &[], false, false)
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
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_state_recorded_without_details_says_no_detail_recorded() {
    let root = scratch_root("no-details");
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
    let report = preview_resolved(&context, &changed, &[], false, false)
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
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_changed_build_env_key_is_previewed_by_name() {
    let root = scratch_root("build-env");
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
            "lemnosd",
            gaia_spec::ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                build_target: "lemnosd".into(),
                build_args: Vec::new(),
                build_command: vec!["gradle".into(), "build".into()],
                build_env: vec![
                    ("GRADLE_USER_HOME".into(), gradle_home.into()),
                    ("JAVA_OPTS".into(), "-Xmx1g".into()),
                ],
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/lemnosd.jar".into(),
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

    let report = preview_resolved(&context, &java("/cache/b"), &[], false, false)
        .expect("the preview of a valid build succeeds");

    let artifact = report
        .operations
        .iter()
        .find(|operation| operation.id == "artifact:lemnosd")
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
    let _ = fs::remove_dir_all(&root);
}
