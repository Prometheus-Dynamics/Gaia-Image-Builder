//! `[image] kind = "none"`: the plan has no image provider operation. The
//! assembly, when there is one, stands in for the image and waits for what
//! the build made; without one, the report waits for everything.

pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{ExecutionPlan, plan_build};
use gaia_testkit::TempDir;
use std::fs;
use std::path::PathBuf;
use support::provider_catalogs;

fn plan_of(extra: &str) -> ExecutionPlan {
    let scratch = TempDir::new("gaia-plan-image-none");
    let root_dir = scratch.join("root").display().to_string();
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "image-none"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[reporting]
summary = true

[[sources]]
id = "files"
kind = "path"
path = "files"

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
kind = "none"

{extra}
"#
        ),
    )
    .expect("config");
    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
}

fn operation<'plan>(plan: &'plan ExecutionPlan, id: &str) -> &'plan gaia_plan::PlannedOperation {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("no {id} in {:?}", ids(plan)))
}

fn ids(plan: &ExecutionPlan) -> Vec<&str> {
    plan.operations
        .iter()
        .map(|operation| operation.id.as_str())
        .collect()
}

fn depends_on(plan: &ExecutionPlan, id: &str, dependency: &str) -> bool {
    operation(plan, id)
        .depends_on
        .iter()
        .any(|dependency_id| dependency_id.as_str() == dependency)
}

#[test]
fn kind_none_without_an_assembly_plans_no_image_and_the_report_waits_for_the_build() {
    let plan = plan_of("");
    let ids = ids(&plan);
    for absent in [
        "image:prepare",
        "image:build",
        "image:check",
        "image:assembly",
    ] {
        assert!(!ids.contains(&absent), "{absent} in {ids:?}");
    }
    assert!(ids.contains(&"source:files"), "{ids:?}");
    assert!(ids.contains(&"artifact:gaia-app"), "{ids:?}");
    assert!(depends_on(&plan, "report:emit", "source:files"), "{ids:?}");
    assert!(
        depends_on(&plan, "report:emit", "artifact:gaia-app"),
        "{ids:?}"
    );
    assert!(
        depends_on(&plan, "report:emit", "install:install-gaia-app"),
        "{ids:?}"
    );
    assert!(plan.validate().is_empty(), "{:?}", plan.validate());
}

#[test]
fn kind_none_assembly_waits_for_the_artifacts_installs_and_its_sources() {
    let plan = plan_of(
        r#"
[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "pkg"
path = "$assembly.work/pkg"

[[image.assembly.files]]
tree = "pkg"
src = "${source.files.path}/a"
dest = "usr/bin/a"
"#,
    );
    let ids = ids(&plan);
    assert!(!ids.contains(&"image:build"), "{ids:?}");
    assert!(
        depends_on(&plan, "image:assembly", "artifact:gaia-app"),
        "{ids:?}"
    );
    assert!(
        depends_on(&plan, "image:assembly", "install:install-gaia-app"),
        "{ids:?}"
    );
    assert!(
        depends_on(&plan, "image:assembly", "source:files"),
        "{ids:?}"
    );
    assert!(
        depends_on(&plan, "report:emit", "image:assembly"),
        "{ids:?}"
    );
    assert!(plan.validate().is_empty(), "{:?}", plan.validate());
}

#[test]
fn a_checkpoint_anchored_at_the_image_anchors_at_the_assembly() {
    let plan = plan_of(
        r#"
[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "pkg"
path = "$assembly.work/pkg"

[[checkpoints]]
id = "snap"
backend = "local"
anchor = "image"
use_policy = "auto"
upload_policy = "off"
"#,
    );
    assert!(
        depends_on(&plan, "checkpoint:snap", "image:assembly"),
        "{:?}",
        ids(&plan)
    );
    assert!(plan.validate().is_empty(), "{:?}", plan.validate());
}
