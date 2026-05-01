pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use support::{unique_dir, write_temp_build};

#[test]
fn run_command_refuses_invalid_buildroot_defconfig_path_before_execution() {
    let build = write_temp_build(
        r#"
build_name = "bad-buildroot-defconfig"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "/definitely/missing/gaia-buildroot.defconfig"

[[image.expected_images]]
name = "rootfs.tar"
format = "tar"
required = true
"#,
    );

    let run = run_with_args(AppArgs::parse_from(vec!["run".to_string(), build]));

    assert_eq!(run.exit_code(), 1);
    match run {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("refusing to run build"));
            assert!(message.contains("validation error"));
        }
        outcome => panic!("expected failed outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_fails_when_buildroot_backend_is_missing_and_fallback_is_not_enabled() {
    let build = write_temp_build(
        r#"
build_name = "missing-buildroot-backend"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.expected_images]]
name = "rootfs.tar"
format = "tar"
required = true
"#,
    );

    let run = run_with_args(AppArgs::parse_from(vec!["run".to_string(), build]));

    assert_eq!(run.exit_code(), 4);
    match run {
        CommandOutcome::Ran {
            report,
            execution_errors,
            ..
        } => {
            assert!(!execution_errors.is_empty(), "expected execution failure");
            assert!(
                execution_errors
                    .iter()
                    .any(|error| error.code == "image_execution_failed")
            );
            assert!(
                report
                    .summary
                    .failure_classes
                    .iter()
                    .any(|entry| { format!("{:?}", entry.class) == "OutputMissing" })
            );
        }
        outcome => panic!("expected ran outcome with execution failure, got {outcome:?}"),
    }
}

#[test]
fn run_command_allows_explicit_buildroot_fallback_and_reports_it() {
    let run_out_dir = unique_dir("gaia-cli-fallback-out");
    let build = write_temp_build(&format!(
        r#"
build_name = "explicit-buildroot-fallback"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "{}"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
allow_fallback = true

[[image.expected_images]]
name = "rootfs.tar"
format = "tar"
required = true
"#,
        run_out_dir
    ));

    let run = run_with_args(AppArgs::parse_from(vec!["run".to_string(), build]));

    assert_eq!(run.exit_code(), 0);
    match run {
        CommandOutcome::Ran {
            report,
            execution_errors,
            ..
        } => {
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            assert_eq!(
                report
                    .summary
                    .primary_image_output
                    .as_deref()
                    .map(|path| path.ends_with("out/images/buildroot")),
                Some(true)
            );
            assert!(report.summary.failure_classes.is_empty());
        }
        outcome => panic!("expected successful ran outcome, got {outcome:?}"),
    }
}
