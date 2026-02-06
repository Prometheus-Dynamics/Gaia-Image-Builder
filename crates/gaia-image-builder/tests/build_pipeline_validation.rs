use std::path::PathBuf;

use gaia_image_builder::config::ConfigDoc;

fn plan_for(value: toml::Value) -> gaia_image_builder::Result<gaia_image_builder::planner::Plan> {
    let doc = ConfigDoc {
        path: PathBuf::from("<mem>"),
        value,
    };

    let mut plan = gaia_image_builder::planner::Plan::default();
    for m in gaia_image_builder::modules::builtin_modules() {
        if m.detect(&doc) {
            m.plan(&doc, &mut plan)?;
        }
    }
    plan.finalize_default()?;
    Ok(plan)
}

#[test]
fn rejects_removed_frontend_table() {
    let value: toml::Value = toml::from_str(
        r#"
[frontend]
enabled = true
"#,
    )
    .unwrap();

    let err = plan_for(value).unwrap_err().to_string();
    assert!(err.contains("frontend"), "unexpected err: {err}");
}

#[test]
fn allows_build_metadata_table() {
    let value: toml::Value = toml::from_str(
        r#"
[build]
version = "1.2.3"
"#,
    )
    .unwrap();

    let plan = plan_for(value).expect("build metadata table should be accepted");
    assert!(plan.tasks().any(|t| t.id == "core.init"));
}

#[test]
fn rejects_duplicate_artifact_ids_across_builders() {
    let value: toml::Value = toml::from_str(
        r#"
[program]

[program.rust]
enabled = true
workspace_dir = "."

[[program.rust.artifacts]]
id = "same"
package = "foo"

[program.java]
enabled = true
workspace_dir = "."

[[program.java.artifacts]]
id = "same"
output_path = "out.jar"
build_command = ["echo", "ok"]
"#,
    )
    .unwrap();

    let err = plan_for(value).unwrap_err().to_string();
    assert!(err.contains("artifact id 'same'"), "unexpected err: {err}");
}

#[test]
fn rejects_unknown_check_id() {
    let value: toml::Value = toml::from_str(
        r#"
[program]

[[program.checks]]
id = "known"
run = ["echo", "ok"]

[program.rust]
enabled = true
workspace_dir = "."

[[program.rust.artifacts]]
id = "a1"
package = "foo"
check_ids = ["missing"]
"#,
    )
    .unwrap();

    let err = plan_for(value).unwrap_err().to_string();
    assert!(
        err.contains("unknown check id 'missing'"),
        "unexpected err: {err}"
    );
}
