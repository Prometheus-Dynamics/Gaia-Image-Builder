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
fn checkpoints_inject_restore_capture_tasks_when_enabled() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
"#,
    )
    .unwrap();

    let plan = plan_for(value).expect("plan should succeed");
    let ids = plan.tasks().map(|t| t.id.clone()).collect::<Vec<_>>();
    assert!(
        ids.iter()
            .any(|id| id == "checkpoints.restore.buildroot-build")
    );
    assert!(
        ids.iter()
            .any(|id| id == "checkpoints.capture.buildroot-build")
    );
}

#[test]
fn checkpoints_tasks_not_injected_when_disabled() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = false

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
"#,
    )
    .unwrap();

    let plan = plan_for(value).expect("plan should succeed");
    let ids = plan.tasks().map(|t| t.id.clone()).collect::<Vec<_>>();
    assert!(!ids.iter().any(|id| id.starts_with("checkpoints.")));
}
