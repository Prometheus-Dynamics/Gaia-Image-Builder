//! An image assembly that reads an artifact's output itself, outside the
//! image feed (an app layer), waits for that artifact. The image build does
//! not, so an app-only change leaves Buildroot alone.

pub mod support;

use gaia_plan::{ExecutionPlan, plan_build};
use gaia_testkit::TempDir;
use std::fs;
use support::provider_catalogs;

fn plan_with_entry(scratch: &TempDir, entry: &str) -> ExecutionPlan {
    let root = scratch.join("workspace");
    fs::create_dir_all(&root).expect("workspace");
    let config = scratch.join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
build_name = "sim"
version = "1.0.0"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "out"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
output_path = "out/artifacts/app"

[[artifacts]]
id = "tool"
kind = "rust"
package = "tool"
output_path = "out/artifacts/tool"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "app"
path = "$assembly.work/app"

[[image.assembly.files]]
tree = "app"
{entry}
"#,
            root = root.display(),
        ),
    )
    .expect("config");
    let spec = gaia_config::resolve_config(config.to_str().expect("utf-8 path"));
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
}

fn dependencies(plan: &ExecutionPlan, id: &str) -> Vec<String> {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id}"))
        .depends_on
        .iter()
        .map(|dependency| dependency.as_str().to_string())
        .collect()
}

#[test]
fn an_assembly_reading_an_artifact_output_waits_for_that_artifact_only() {
    let scratch = TempDir::new("plan-assembly-artifacts");
    let plan = plan_with_entry(
        &scratch,
        r#"src = "out/artifacts/app"
dest = "usr/bin/app"
mode = "0755""#,
    );
    let assembly = dependencies(&plan, "image:assembly");
    assert!(
        assembly.contains(&"artifact:app".to_string()),
        "{assembly:?}"
    );
    assert!(
        !assembly.contains(&"artifact:tool".to_string()),
        "{assembly:?}"
    );
    let image = dependencies(&plan, "image:build");
    assert!(
        image
            .iter()
            .all(|dependency| !dependency.starts_with("artifact:")),
        "{image:?}"
    );
}

#[test]
fn a_glob_or_directory_over_artifact_outputs_waits_for_each_artifact_under_it() {
    let scratch = TempDir::new("plan-assembly-artifacts-glob");
    let plan = plan_with_entry(
        &scratch,
        r#"src_glob = "out/artifacts/*"
dest = "usr/bin""#,
    );
    let assembly = dependencies(&plan, "image:assembly");
    assert!(
        assembly.contains(&"artifact:app".to_string()),
        "{assembly:?}"
    );
    assert!(
        assembly.contains(&"artifact:tool".to_string()),
        "{assembly:?}"
    );
}
