pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, ValidationReport, validate_spec};
use std::fs;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "java-policy-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

fn report_for(java_settings: &str) -> ValidationReport {
    let path = write_temp_config(&format!("{BASE}\n{java_settings}"));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
}

fn has_error(report: &ValidationReport, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error && d.code == code)
}

#[test]
fn gradle_home_accepts_workspace_and_user_cache() {
    for value in ["workspace", "user-cache"] {
        let report = report_for(&format!("[providers.java]\ngradle_home = \"{value}\"\n"));
        assert!(
            !has_error(&report, "java_gradle_home_invalid"),
            "{value}: {:?}",
            report.errors
        );
    }
}

#[test]
fn gradle_home_rejects_unknown_values() {
    for value in ["user_cache", "cache", ""] {
        let report = report_for(&format!("[providers.java]\ngradle_home = \"{value}\"\n"));
        assert!(
            has_error(&report, "java_gradle_home_invalid"),
            "{value} should fail: {:?}",
            report.errors
        );
        assert!(report.errors.iter().any(|message| {
            message.contains("providers.java.gradle_home") && message.contains("'workspace'")
        }));
    }
}

#[test]
fn gradle_home_under_another_provider_warns_and_names_it() {
    let report = report_for("[providers.go]\ngradle_home = \"user-cache\"\n");
    let warnings = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "provider_gradle_home_ignored")
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(warnings[0].severity, DiagnosticSeverity::Warning);
    assert!(warnings[0].message.contains("providers.go.gradle_home"));
    assert!(warnings[0].message.contains("[providers.java]"));
    assert!(!has_error(&report, "provider_gradle_home_ignored"));
}

#[test]
fn gradle_home_under_java_does_not_warn() {
    let report = report_for("[providers.java]\ngradle_home = \"user-cache\"\n");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == "provider_gradle_home_ignored"),
        "{:?}",
        report.diagnostics
    );
}
