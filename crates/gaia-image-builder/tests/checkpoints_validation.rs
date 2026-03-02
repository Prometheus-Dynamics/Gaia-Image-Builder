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
    gaia_image_builder::checkpoints::validate_against_plan(&doc, &plan)?;
    plan.finalize_default()?;
    Ok(plan)
}

#[test]
fn checkpoints_unknown_anchor_rejected() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.collect"
"#,
    )
    .unwrap();

    let err = plan_for(value).unwrap_err().to_string();
    assert!(
        err.contains("not supported") || err.contains("unknown anchor_task"),
        "unexpected err: {err}"
    );
}

#[test]
fn checkpoints_duplicate_id_rejected() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
"#,
    )
    .unwrap();

    let err = plan_for(value).unwrap_err().to_string();
    assert!(
        err.contains("duplicate checkpoint id"),
        "unexpected err: {err}"
    );
}

#[test]
fn checkpoints_valid_base_anchor_accepted() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
use_policy = "auto"
"#,
    )
    .unwrap();

    let plan = plan_for(value).expect("valid checkpoint config should plan");
    assert!(plan.tasks().any(|t| t.id == "buildroot.build"));
}

#[test]
fn checkpoints_disabled_keeps_legacy_configs() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"
"#,
    )
    .unwrap();

    let plan = plan_for(value).expect("legacy config should still plan");
    assert!(plan.tasks().any(|t| t.id == "buildroot.collect"));
}
