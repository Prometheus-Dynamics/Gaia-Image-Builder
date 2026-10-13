pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use gaia_testkit::TempDir;
use std::fs;
use std::path::PathBuf;
use support::write_temp_build;

#[test]
fn help_and_version_dispatch_without_config_resolution() {
    match run_with_args(AppArgs::parse_from(["--help"])) {
        CommandOutcome::Help { text } => {
            assert!(text.contains("Usage:"));
            assert!(text.contains("gaia --version"));
        }
        outcome => panic!("expected help outcome, got {outcome:?}"),
    }

    match run_with_args(AppArgs::parse_from(["--version"])) {
        CommandOutcome::Version { text } => {
            assert!(text.starts_with("gaia "));
        }
        outcome => panic!("expected version outcome, got {outcome:?}"),
    }
}

#[test]
fn invalid_build_path_is_reported_as_failed_command() {
    let scratch = TempDir::new("gaia-cli-missing-build");
    let missing = scratch.join("missing-build.toml").display().to_string();

    let outcome = run_with_args(AppArgs::parse_from(["validate", missing.as_str()]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("failed to locate build config"));
            assert!(message.contains(&missing));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[test]
fn malformed_build_config_is_reported_as_failed_command() {
    let scratch = TempDir::new("gaia-cli-malformed-build");
    let path = write_temp_build(
        &scratch,
        r#"
build_name = "broken"
[workspace
root_dir = "."
"#,
    );

    let outcome = run_with_args(AppArgs::parse_from(["validate", path.as_str()]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("failed to parse build config"));
            assert!(message.contains(&path));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[test]
fn plan_command_refuses_invalid_config_before_execution() {
    let scratch = TempDir::new("gaia-cli-invalid-plan");
    let out_dir = scratch.join("out").display().to_string();
    let build_dir = scratch.join("build").display().to_string();
    let path = write_temp_build(
        &scratch,
        &format!(
            r#"
build_name = "invalid-plan"

[workspace]
root_dir = "."
build_dir = "{build_dir}"
out_dir = "{out_dir}"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
install_entries = ["missing-install"]
"#,
        ),
    );

    let outcome = run_with_args(AppArgs::parse_from(["plan", path.as_str()]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("refusing to plan build"));
            assert!(message.contains("validation error"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
    assert!(!PathBuf::from(&out_dir).join(".gaia/reports").exists());
}

#[test]
fn run_command_refuses_invalid_config_before_execution() {
    let scratch = TempDir::new("gaia-cli-invalid-run");
    let out_dir = scratch.join("out").display().to_string();
    let build_dir = scratch.join("build").display().to_string();
    let path = write_temp_build(
        &scratch,
        &format!(
            r#"
build_name = "invalid-run"

[workspace]
root_dir = "."
build_dir = "{build_dir}"
out_dir = "{out_dir}"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
stage_files = ["missing-stage-file"]
"#,
        ),
    );

    let outcome = run_with_args(AppArgs::parse_from(["run", path.as_str()]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("refusing to run build"));
            assert!(message.contains("validation error"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
    assert!(!PathBuf::from(&build_dir).exists());
    assert!(PathBuf::from(&out_dir).join(".gaia/reports").exists());
}

#[test]
fn run_command_reports_report_write_failure_without_panicking() {
    let scratch = TempDir::new("gaia-cli-report-write-failure");
    let out_file = scratch.join("out-file").display().to_string();
    let build_dir = scratch.join("build").display().to_string();
    fs::write(&out_file, "not a directory").expect("out file");
    let path = write_temp_build(
        &scratch,
        &format!(
            r#"
build_name = "report-write-failure"

[workspace]
root_dir = "."
build_dir = "{build_dir}"
out_dir = "{out_file}"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
stage_files = ["missing-stage-file"]
"#,
        ),
    );

    let outcome = run_with_args(AppArgs::parse_from(["run", path.as_str()]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("failed to write report outputs"));
            assert!(message.contains("report-write-failure"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[test]
fn usage_errors_fail_before_resolving_the_build() {
    let outcome = run_with_args(AppArgs::parse_from(["validate", "--no-such-flag"]));

    assert_eq!(outcome.exit_code(), 1);
    match outcome {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("unknown flag '--no-such-flag'"));
            assert!(message.contains("gaia --help"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}
