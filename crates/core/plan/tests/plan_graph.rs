pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{
    OperationOptionality, OperationParallelismDomain, OperationParallelismMode, OperationReuse,
    plan_build,
};
use std::fs;
use std::path::PathBuf;
use support::{default_config_path, provider_catalogs, unique_dir};

#[test]
fn default_plan_has_valid_operations_and_rebuild_reasons() {
    let spec = resolve_config(&default_config_path());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();

    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(plan.validate().is_empty());
    assert_eq!(plan.operations.len(), 11);
    assert!(
        plan.operations
            .iter()
            .all(|operation| matches!(operation.reuse, OperationReuse::Execute(_)))
    );
    assert!(plan.operations.iter().any(|operation| {
        matches!(&operation.reuse, OperationReuse::Execute(reason) if reason.code == "artifact_build_required")
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "source:gaia-upstream"
            && operation.parallelism.domain == OperationParallelismDomain::Sources
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && operation.parallelism.domain == OperationParallelismDomain::Artifacts
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "install:install-gaia-app"
            && operation.parallelism.domain == OperationParallelismDomain::Runtime
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "image:build"
            && operation.parallelism.domain == OperationParallelismDomain::Images
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "checkpoint:base-image"
            && operation.parallelism.domain == OperationParallelismDomain::Checkpoints
            && operation.parallelism.mode == OperationParallelismMode::Parallelizable
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "report:emit"
            && operation.parallelism.domain == OperationParallelismDomain::Reporting
            && operation.parallelism.mode == OperationParallelismMode::Exclusive
    }));
}

#[test]
fn starting_point_image_depends_on_declared_source() {
    let root_dir = unique_dir("gaia-plan-starting-point-source-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "starting-point-source"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "base-rootfs"
kind = "git"
repo = "https://example.invalid/base-rootfs.git"
branch = "main"

[providers.git]
allow_remote_resolution = false

[image]
kind = "starting-point"
source = "base-rootfs"
source_path = "rootfs"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let image = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image operation");
    assert!(
        image
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "source:base-rootfs")
    );
}

#[test]
fn buildroot_image_prepare_can_run_before_artifact_installs() {
    let root_dir = unique_dir("gaia-plan-buildroot-split-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "buildroot-split"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "buildroot-source"
kind = "path"
path = "."

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-app"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[image]
kind = "buildroot"
source = "buildroot-source"
defconfig = "qemu_aarch64_virt_defconfig"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let image_prepare = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:prepare")
        .expect("image prepare operation");
    let image_build = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:build")
        .expect("image build operation");

    assert!(
        image_prepare
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "source:buildroot-source")
    );
    assert!(
        !image_prepare
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
    assert!(
        image_build
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "image:prepare")
    );
    assert!(
        image_build
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
}

#[test]
fn required_operation_depends_on_best_effort_is_a_plan_error() {
    let plan = gaia_plan::ExecutionPlan {
        build_id: gaia_spec::BuildId::new("bad-plan"),
        operations: vec![
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::checkpoint(&gaia_spec::CheckpointId::new("best-effort")),
                gaia_plan::OperationKind::CaptureCheckpoint {
                    checkpoint_id: gaia_spec::CheckpointId::new("best-effort"),
                },
            )
            .with_optionality(OperationOptionality::BestEffort),
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::report(),
                gaia_plan::OperationKind::EmitReport,
            )
            .with_optionality(OperationOptionality::Required)
            .with_dependency(gaia_plan::OperationId::checkpoint(
                &gaia_spec::CheckpointId::new("best-effort"),
            )),
        ],
    };

    let diagnostics = plan.validate();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "required_depends_on_best_effort" })
    );
}
