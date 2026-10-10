pub mod support;

use gaia_plan::{OperationReuse, ReuseState, plan_build};
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec};

fn materialize_reusable_test_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream"))
        .expect("gaia-upstream source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
        "ok",
    )
    .expect("gaia-upstream source marker");
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root"))
        .expect("workspace-root source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
        "ok",
    )
    .expect("workspace-root source marker");
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir");
    fs::create_dir_all(
        PathBuf::from(&spec.workspace.build_dir).join("image/buildroot-output/target"),
    )
    .expect("buildroot target dir");
    fs::write(
        PathBuf::from(&collect_dir).join("image-provider.txt"),
        "image",
    )
    .expect("image marker");
    let archive_name = spec
        .image
        .output
        .archive_name
        .clone()
        .expect("image archive name");
    fs::write(PathBuf::from(&collect_dir).join(archive_name), "archive").expect("image archive");
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\ndest=/etc/motd\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia.service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\n",
    )
    .expect("checkpoint runtime state");
}

fn fully_recorded_state(
    spec: &gaia_spec::ResolvedBuildSpec,
    baseline_plan: &gaia_plan::ExecutionPlan,
    reused_ids: &[&str],
) -> ReuseState {
    let mut state = support::reuse_state_for_ids(spec, baseline_plan, reused_ids);
    state.operation_input_signatures = baseline_plan
        .operations
        .iter()
        .filter(|operation| reused_ids.contains(&operation.id.as_str()))
        .map(|operation| {
            (
                operation.id.as_str().to_string(),
                gaia_plan::operation_input_signature(spec, baseline_plan, operation),
            )
        })
        .collect();
    state
}

const REUSABLE_IDS: [&str; 9] = [
    "source:gaia-upstream",
    "source:workspace-root",
    "artifact:gaia-app",
    "install:install-gaia-app",
    "stage:file:motd",
    "stage:env:runtime-env",
    "stage:service:gaia-service",
    "image:build",
    "checkpoint:base-image",
];

fn reuse_of<'plan>(plan: &'plan gaia_plan::ExecutionPlan, id: &str) -> &'plan OperationReuse {
    &plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
        .reuse
}

fn rebuild_of(ids: &[&str], packages: &[&str]) -> gaia_plan::RebuildRequest {
    gaia_plan::RebuildRequest {
        targets: ids
            .iter()
            .map(|id| gaia_plan::PlanTarget::Operation(id.to_string()))
            .collect(),
        packages: packages.iter().map(|name| name.to_string()).collect(),
    }
}

#[test]
fn rebuild_executes_a_reusable_operation_and_its_dependents_follow() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_test_outputs(&spec);
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state = fully_recorded_state(&spec, &baseline_plan, &REUSABLE_IDS);
    let rebuild = rebuild_of(&["artifact:gaia-app"], &[]);
    let plan = gaia_plan::plan_build_with_rebuilds(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
        &rebuild,
    );
    assert!(matches!(
        reuse_of(&plan, "artifact:gaia-app"),
        OperationReuse::Execute(reason)
            if reason.code == "rebuild_requested"
                && reason.message.contains("--rebuild names it")
    ));
    assert!(matches!(
        reuse_of(&plan, "install:install-gaia-app"),
        OperationReuse::Execute(reason) if reason.code == "dependency_rebuilt"
    ));
}

#[test]
fn rebuild_without_reuse_state_only_changes_the_reason() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let rebuild = rebuild_of(&["artifact:gaia-app"], &[]);
    let plan = gaia_plan::plan_build_with_rebuilds(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        None,
        &rebuild,
    );
    assert!(matches!(
        reuse_of(&plan, "artifact:gaia-app"),
        OperationReuse::Execute(reason) if reason.code == "rebuild_requested"
    ));
}

#[test]
fn rebuild_domain_and_glob_targets_force_their_operations() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let domain = gaia_plan::RebuildRequest {
        targets: vec![gaia_plan::PlanTarget::Domain(gaia_plan::PlanDomain::Image)],
        packages: Vec::new(),
    };
    let plan = gaia_plan::plan_build_with_rebuilds(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        None,
        &domain,
    );
    assert!(matches!(
        reuse_of(&plan, "image:build"),
        OperationReuse::Execute(reason) if reason.code == "rebuild_requested"
    ));
    let glob = rebuild_of(&["artifact:*"], &[]);
    assert_eq!(glob.check(&plan), Ok(()));
    let plan = gaia_plan::plan_build_with_rebuilds(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        None,
        &glob,
    );
    assert!(matches!(
        reuse_of(&plan, "artifact:gaia-app"),
        OperationReuse::Execute(reason) if reason.code == "rebuild_requested"
    ));
}

#[test]
fn unknown_rebuild_operations_are_refused_with_close_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let typo = rebuild_of(&["artifact:gaia-ap"], &[]);
    let error = typo.check(&plan).expect_err("unknown id");
    assert!(
        error.contains("unknown operation 'artifact:gaia-ap' (did you mean 'artifact:gaia-app'?)"),
        "{error}"
    );
    let missing_glob = rebuild_of(&["stage:*-nothing"], &[]);
    assert!(missing_glob.check(&plan).is_err());
}

#[test]
fn rebuild_packages_force_the_image_build_with_their_reason() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let packages = rebuild_of(&[], &["libfoo"]);
    let plan = gaia_plan::plan_build_with_rebuilds(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        None,
        &packages,
    );
    assert!(matches!(
        reuse_of(&plan, "image:build"),
        OperationReuse::Execute(reason)
            if reason.code == "rebuild_requested"
                && reason.message.contains("--rebuild-package names packages it builds: libfoo")
    ));
    assert!(matches!(
        reuse_of(&plan, "artifact:gaia-app"),
        OperationReuse::Execute(reason) if reason.code != "rebuild_requested"
    ));
}
