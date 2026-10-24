pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

fn codes(checks: &str) -> Vec<&'static str> {
    let contents = format!(
        r#"
build_name = "checks"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "slots-fat"
path = "$assembly.work/fat"

{checks}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let _ = fs::remove_file(path);
    validate_spec(&spec)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn assembly_checks_need_an_id_a_command_and_known_trees() {
    let good = codes(
        r#"[[image.assembly.checks]]
id = "boot-overlays"
command = ["/bin/sh", "check-boot-overlays.sh", "$assembly.tree.slots-fat"]
timeout_secs = 60
"#,
    );
    assert!(
        !good.iter().any(|code| code.starts_with("assembly_check")),
        "{good:?}"
    );
    assert!(!good.contains(&"config_unknown_key"), "{good:?}");

    let bad = codes(
        r#"[[image.assembly.checks]]
id = "a"
command = []

[[image.assembly.checks]]
id = "a"
command = ["check", "$assembly.tree.nope/config.txt"]
"#,
    );
    for code in [
        "assembly_check_command_missing",
        "assembly_check_id_invalid",
        "assembly_check_tree_unknown",
    ] {
        assert!(bad.contains(&code), "{code}: {bad:?}");
    }
}
