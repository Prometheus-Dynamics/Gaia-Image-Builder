//! When a Buildroot build plans `image:prepare`: the packages build without
//! the artifacts, so the image's own make overlaps the artifact builds.

pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{ExecutionPlan, plan_build};
use gaia_testkit::TempDir;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, scratch_child};

const BASE: &str = r#"
build_name = "buildroot-prepare"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "buildroot-source"
kind = "path"
path = "."

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
output_path = "out/app"

[image]
kind = "buildroot"
source = "buildroot-source"
defconfig = "qemu_aarch64_virt_defconfig"
"#;

const INSTALL: &str = r#"
[[install]]
id = "install-app"
artifact = "app"
dest = "/usr/bin/app"
"#;

fn plan(config: &str) -> (ExecutionPlan, TempDir) {
    let scratch = TempDir::new("gaia-plan-prepare");
    let root_dir = scratch_child(&scratch, "root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(&config_path, config).expect("config");
    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    (
        plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog),
        scratch,
    )
}

fn depends_on(plan: &ExecutionPlan, id: &str) -> Vec<String> {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not planned"))
        .depends_on
        .iter()
        .map(|dependency| dependency.as_str().to_string())
        .collect()
}

#[test]
fn an_image_that_installs_artifacts_prepares_its_packages_beside_them() {
    let (plan, _scratch) = plan(&format!("{BASE}{INSTALL}"));
    let prepare = depends_on(&plan, "image:prepare");
    assert!(prepare.contains(&"source:buildroot-source".to_string()));
    assert!(
        !prepare
            .iter()
            .any(|dependency| dependency.starts_with("install:")
                || dependency.starts_with("artifact:")),
        "prepare waits for no artifact: {prepare:?}"
    );
    let build = depends_on(&plan, "image:build");
    assert!(build.contains(&"image:prepare".to_string()));
    assert!(build.contains(&"install:install-app".to_string()));
}

#[test]
fn an_image_without_installs_builds_once() {
    let (plan, _scratch) = plan(BASE);
    assert!(
        plan.operations
            .iter()
            .all(|operation| operation.id.as_str() != "image:prepare")
    );
}
