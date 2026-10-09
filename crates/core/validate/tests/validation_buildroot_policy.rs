pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, ValidationReport, validate_spec};
use std::fs;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "buildroot-policy-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

fn report_for(buildroot_settings: &str) -> ValidationReport {
    let path = write_temp_config(&format!("{BASE}\n{buildroot_settings}"));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
}

fn has_code(report: &ValidationReport, severity: DiagnosticSeverity, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.severity == severity && d.code == code)
}

#[test]
fn ram_budget_must_parse_and_be_positive() {
    let report = report_for("[providers.buildroot]\nram_budget = \"lots\"\n");
    assert!(has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_ram_budget_invalid"
    ));
    assert!(report.errors.iter().any(|message| {
        message.contains("providers.buildroot.ram_budget") && message.contains("'lots'")
    }));

    let report = report_for("[providers.buildroot]\nram_budget = \"0G\"\n");
    assert!(report.errors.iter().any(|message| {
        message.contains("providers.buildroot.ram_budget") && message.contains("greater than zero")
    }));

    let report = report_for("[providers.buildroot]\nram_budget = \"\"\n");
    assert!(has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_ram_budget_invalid"
    ));
}

#[test]
fn valid_ram_budget_is_accepted() {
    let report = report_for("[providers.buildroot]\nwork_dir = \"ram\"\nram_budget = \"60G\"\n");
    assert!(!has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_ram_budget_invalid"
    ));
}

#[test]
fn work_dir_accepts_disk_ram_and_paths_but_not_empty() {
    for value in ["disk", "ram", "build/ram-tree", "/var/tmp/gaia"] {
        let report = report_for(&format!("[providers.buildroot]\nwork_dir = \"{value}\"\n"));
        assert!(
            !has_code(
                &report,
                DiagnosticSeverity::Error,
                "buildroot_work_dir_empty"
            ),
            "work_dir {value:?} should be accepted"
        );
    }

    let report = report_for("[providers.buildroot]\nwork_dir = \"\"\n");
    assert!(has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_work_dir_empty"
    ));
}

#[test]
fn invalid_host_tool_policy_string_is_an_error() {
    let report = report_for("[providers.buildroot.host_tools]\nccache = \"system,host\"\n");
    assert!(has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_host_tools_invalid"
    ));
    assert!(report.errors.iter().any(|message| message
        == "providers.buildroot.host_tools.ccache: expected a comma-separated list of system, build, fail; got 'system,host'"));

    let report = report_for("[providers.buildroot.host_tools]\ndefault = \"\"\n");
    assert!(report.errors.iter().any(|message| {
        message.starts_with("providers.buildroot.host_tools.default: expected")
    }));
}

#[test]
fn valid_host_tool_policy_has_no_errors() {
    let report = report_for(
        "[providers.buildroot.host_tools]\ndefault = \"build\"\nccache = \"system,build\"\n",
    );
    assert!(!has_code(
        &report,
        DiagnosticSeverity::Error,
        "buildroot_host_tools_invalid"
    ));
    assert!(!has_code(
        &report,
        DiagnosticSeverity::Warning,
        "buildroot_host_tools_unreachable_step"
    ));
}

#[test]
fn steps_after_build_or_fail_are_unreachable_warnings() {
    let report = report_for(
        "[providers.buildroot.host_tools]\nccache = \"fail,build\"\npkgconf = \"build,system\"\n",
    );
    let unreachable: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "buildroot_host_tools_unreachable_step")
        .collect();
    assert_eq!(unreachable.len(), 2, "{report:?}");
    assert!(
        unreachable
            .iter()
            .all(|d| d.severity == DiagnosticSeverity::Warning)
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|message| message.contains("host_tools.ccache") && message.contains("'fail'"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|message| message.contains("host_tools.pkgconf") && message.contains("'build'"))
    );
    assert!(!report.errors.iter().any(|m| m.contains("host_tools")));
}

#[test]
fn system_before_fail_is_not_warned() {
    let report = report_for("[providers.buildroot.host_tools]\nccache = \"system,fail\"\n");
    assert!(!has_code(
        &report,
        DiagnosticSeverity::Warning,
        "buildroot_host_tools_unreachable_step"
    ));
}
