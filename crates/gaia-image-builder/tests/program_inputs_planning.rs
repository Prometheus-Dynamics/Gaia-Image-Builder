use gaia_image_builder::config::ConfigDoc;

fn plan_for(
    mut doc: ConfigDoc,
    sets: &[&str],
) -> gaia_image_builder::Result<gaia_image_builder::planner::Plan> {
    let sets = sets.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    gaia_image_builder::build_inputs::apply_cli_overrides(&mut doc, &sets)?;

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

fn doc_for(src: &str) -> ConfigDoc {
    ConfigDoc {
        path: "inline.toml".into(),
        value: toml::from_str(src).expect("valid toml"),
    }
}

#[test]
fn program_tasks_are_omitted_when_inputs_disable_all_artifacts() {
    let doc = doc_for(
        r#"
[inputs.options.mode]
type = "string"
default = "off"
choices = ["off", "on"]

[program]
enabled = true

[program.custom]
enabled = true
workspace_dir = "."

[[program.custom.artifacts]]
id = "pv-jar"
enabled_if = ["mode=on"]
mode = "prebuilt"
prebuilt_path = "examples/helios/assets/etc/hostname"

[program.install]
enabled = true

[[program.install.items]]
artifact = "pv-jar"
enabled_if = ["mode=on"]
dest = "/opt/pv/pv.jar"
mode = 420
"#,
    );

    let plan = plan_for(doc, &[]).expect("plan with defaults");
    let ids = plan.tasks().map(|t| t.id.as_str()).collect::<Vec<_>>();
    assert!(
        !ids.contains(&"program.custom.artifacts"),
        "program.custom should be omitted when nothing is selected: {ids:?}"
    );
    assert!(
        !ids.contains(&"program.install.stage"),
        "program.install should be omitted when nothing is selected: {ids:?}"
    );
}

#[test]
fn program_tasks_appear_when_inputs_enable_artifacts() {
    let doc = doc_for(
        r#"
[inputs.options.mode]
type = "string"
default = "off"
choices = ["off", "on"]

[program]
enabled = true

[program.custom]
enabled = true
workspace_dir = "."

[[program.custom.artifacts]]
id = "pv-jar"
enabled_if = ["mode=on"]
mode = "prebuilt"
prebuilt_path = "examples/helios/assets/etc/hostname"

[program.install]
enabled = true

[[program.install.items]]
artifact = "pv-jar"
enabled_if = ["mode=on"]
dest = "/opt/pv/pv.jar"
mode = 420
"#,
    );

    let plan = plan_for(doc, &["mode=on"]).expect("plan with mode=on");
    let ids = plan.tasks().map(|t| t.id.as_str()).collect::<Vec<_>>();
    assert!(
        ids.contains(&"program.custom.artifacts"),
        "program.custom should exist when mode=on: {ids:?}"
    );
    assert!(
        ids.contains(&"program.install.stage"),
        "program.install should exist when mode=on: {ids:?}"
    );
}
