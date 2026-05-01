pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{OperationOptionality, plan_build};
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, unique_dir};

#[test]
fn checkpoint_can_anchor_to_install_operation() {
    let root_dir = unique_dir("gaia-plan-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "checkpoint-anchor"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia-app"
artifact = "gaia-app"
dest = "/usr/bin/gaia"

[[checkpoints]]
id = "after-install"
backend = "local"
anchor = "install:install-gaia-app"
use_policy = "auto"
upload_policy = "off"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let checkpoint = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "checkpoint:after-install")
        .expect("checkpoint operation");
    assert!(
        checkpoint
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "install:install-gaia-app")
    );
}

#[test]
fn checkpoint_policies_flow_into_operation_optionality() {
    let root_dir = unique_dir("gaia-plan-optional-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "checkpoint-optionality"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[checkpoints]]
id = "required-checkpoint"
backend = "local"
use_policy = "always"
upload_policy = "off"

[[checkpoints]]
id = "conditional-checkpoint"
backend = "local"
use_policy = "auto"
upload_policy = "off"

[[checkpoints]]
id = "best-effort-checkpoint"
backend = "local"
use_policy = "off"
upload_policy = "off"
"#,
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "checkpoint:required-checkpoint"
            && operation.optionality == OperationOptionality::Required
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "checkpoint:conditional-checkpoint"
            && operation.optionality == OperationOptionality::Conditional
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "checkpoint:best-effort-checkpoint"
            && operation.optionality == OperationOptionality::BestEffort
    }));

    let report = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "report:emit")
        .expect("report operation");
    assert!(
        !report
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "checkpoint:best-effort-checkpoint")
    );
}
