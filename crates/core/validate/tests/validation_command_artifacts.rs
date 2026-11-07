pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, ValidationReport, validate_spec};
use std::fs;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "command-artifact-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[sources]]
id = "helios"
kind = "path"
path = "."
"#;

fn report_for(artifact: &str) -> ValidationReport {
    let path = write_temp_config(&format!("{BASE}\n{artifact}"));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
}

fn error_codes(report: &ValidationReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error && d.code.starts_with("command_"))
        .map(|d| d.code)
        .collect()
}

#[test]
fn a_host_command_with_a_collect_validates() {
    let report = report_for(
        r#"
[[artifacts]]
id = "helios-ui"
kind = "command"
source = "helios"
command = ["bun", "run", "build"]
collect = "ui/dist"
collect_exclude = ["mock"]
output_path = "out/helios-ui"
"#,
    );
    assert!(error_codes(&report).is_empty(), "{:?}", report.errors);
}

#[test]
fn docker_execution_an_escaping_collect_and_an_exclude_without_collect_fail() {
    let report = report_for(
        r#"
[[artifacts]]
id = "in-docker"
kind = "command"
source = "helios"
command = ["make"]
collect = "dist"
output_path = "out/in-docker"

[artifacts.execution]
backend = "docker"

[artifacts.execution.docker]
image = "alpine:3.20"

[[artifacts]]
id = "escaping"
kind = "command"
source = "helios"
command = ["make"]
collect = "../elsewhere"
output_path = "out/escaping"

[[artifacts]]
id = "stray-exclude"
kind = "command"
source = "helios"
command = ["make"]
collect_exclude = ["mock"]
output_path = "out/stray"

[[artifacts]]
id = "nothing-to-run"
kind = "command"
source = "helios"
command = []
output_path = "out/nothing"
"#,
    );
    let codes = error_codes(&report);
    for code in [
        "command_artifact_docker_unsupported",
        "command_artifact_collect_outside",
        "command_artifact_exclude_without_collect",
        "command_artifact_command_empty",
    ] {
        assert!(codes.contains(&code), "{code} missing from {codes:?}");
    }
}
