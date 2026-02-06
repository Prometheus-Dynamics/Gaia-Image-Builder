use std::path::PathBuf;

use gaia_image_builder::config::ConfigDoc;

#[test]
fn disabling_target_module_does_not_break_buildroot_deps() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
[buildroot.steps.fetch]
enabled = true

[buildroot.rpi]
enabled = false
"#,
    )
    .unwrap();

    let doc = ConfigDoc {
        path: PathBuf::from("<mem>"),
        value,
    };

    let mut plan = gaia_image_builder::planner::Plan::default();
    for m in gaia_image_builder::modules::builtin_modules() {
        if m.detect(&doc) {
            m.plan(&doc, &mut plan).unwrap();
        }
    }
    plan.finalize_default().unwrap();

    // If buildroot.fetch incorrectly depends on a non-optional target token when
    // buildroot.rpi is disabled, ordering will fail.
    plan.ordered().unwrap();
}

#[test]
fn buildroot_stage_barrier_dep_is_optional_without_finalize() {
    let value: toml::Value = toml::from_str(
        r#"
[buildroot]
enabled = true
defconfig = "x86_64_defconfig"
"#,
    )
    .unwrap();

    let doc = ConfigDoc {
        path: PathBuf::from("<mem>"),
        value,
    };

    let mut plan = gaia_image_builder::planner::Plan::default();
    for m in gaia_image_builder::modules::builtin_modules() {
        if m.detect(&doc) {
            m.plan(&doc, &mut plan).unwrap();
        }
    }

    // TUI previously ordered before finalize_default(); this should still resolve.
    plan.ordered().unwrap();
}
