pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use support::starting_point_raw_image_example_build_path;

#[test]
fn preview_plans_a_build_and_reports_providers_without_a_preview() {
    let build = starting_point_raw_image_example_build_path();
    let outcome = run_with_args(AppArgs {
        command: AppCommand::Preview,
        build: build.clone(),
        fail_on_clean: true,
        ..AppArgs::default()
    });
    let CommandOutcome::Previewed { report } = &outcome else {
        panic!("expected a preview, got {outcome:?}");
    };
    assert!(report.operations.iter().any(|operation| operation.executes));
    assert!(!report.tripped(), "{report:?}");
    assert!(
        report
            .images
            .iter()
            .any(|image| image.note.as_deref() == Some("this provider has no preview")),
        "{:?}",
        report.images
    );
    assert_eq!(outcome.exit_code(), 0);

    // `gaia run --dry-run` is the same preview.
    let alias = AppArgs::parse_from(["run", build.as_str(), "--dry-run"]);
    assert_eq!(alias.command, AppCommand::Preview);
    assert!(matches!(
        run_with_args(alias),
        CommandOutcome::Previewed { .. }
    ));
}

#[test]
fn preview_only_flags_are_refused_for_other_commands() {
    let outcome = run_with_args(AppArgs::parse_from([
        "plan",
        starting_point_raw_image_example_build_path().as_str(),
        "--json",
    ]));
    assert!(
        matches!(&outcome, CommandOutcome::Failed { message } if message.contains("--json")),
        "{outcome:?}"
    );
}

#[test]
fn first_preview_has_no_earlier_run_to_invalidate() {
    let outcome = run_with_args(AppArgs {
        command: AppCommand::Preview,
        build: starting_point_raw_image_example_build_path(),
        explicit_overrides: vec![
            (
                "workspace.build_dir".into(),
                support::unique_dir("cli-preview-build"),
            ),
            (
                "workspace.out_dir".into(),
                support::unique_dir("cli-preview-out"),
            ),
        ],
        ..AppArgs::default()
    });
    let CommandOutcome::Previewed { report } = &outcome else {
        panic!("expected a preview, got {outcome:?}");
    };
    assert!(report.invalidation.is_none(), "{report:?}");
    let executing = report.operations.iter().filter(|op| op.executes).count();
    assert_eq!(
        report.invalidation_line(),
        format!("no earlier run recorded: {executing} operation(s) run")
    );
}
