//! `examples/profiles`: the one build file validates and plans for both of
//! its device profiles (`--set build.target=...`).

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};

fn example_build_path() -> String {
    format!(
        "{}/../../../examples/profiles/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn args(command: AppCommand, target: Option<&str>) -> AppArgs {
    AppArgs {
        command,
        build: example_build_path(),
        explicit_overrides: target
            .map(|target| vec![("build.target".to_string(), target.to_string())])
            .unwrap_or_default(),
        ..AppArgs::default()
    }
}

fn operation_ids(plan: &gaia_plan::ExecutionPlan) -> Vec<String> {
    plan.operations
        .iter()
        .map(|operation| operation.id.as_str().to_string())
        .collect()
}

#[test]
fn both_profiles_of_the_example_validate() {
    for target in [None, Some("systemcore")] {
        match run_with_args(args(AppCommand::Validate, target)) {
            CommandOutcome::Validated { validation, .. } => {
                assert!(
                    validation.errors.is_empty(),
                    "{target:?}: {:?}",
                    validation.errors
                );
                assert!(
                    validation.warnings.is_empty(),
                    "{target:?}: {:?}",
                    validation.warnings
                );
            }
            other => panic!("expected validated outcome for {target:?}, got {other:?}"),
        }
    }
}

#[test]
fn the_raze_plan_builds_the_image_and_the_systemcore_plan_assembles_the_package() {
    match run_with_args(args(AppCommand::Plan, None)) {
        CommandOutcome::Planned {
            plan, diagnostics, ..
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let ids = operation_ids(&plan);
            assert!(ids.iter().any(|id| id == "image:build"), "{ids:?}");
            assert!(ids.iter().any(|id| id == "artifact:helios-api"), "{ids:?}");
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }

    match run_with_args(args(AppCommand::Plan, Some("systemcore"))) {
        CommandOutcome::Planned {
            plan,
            diagnostics,
            spec,
            ..
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let ids = operation_ids(&plan);
            assert!(ids.iter().any(|id| id == "image:assembly"), "{ids:?}");
            assert!(
                ids.iter().all(|id| !id.starts_with("image:build")
                    && !id.starts_with("image:prepare")
                    && !id.starts_with("image:check")),
                "{ids:?}"
            );
            assert!(ids.iter().any(|id| id == "artifact:helios-api"), "{ids:?}");
            assert!(
                spec.metadata
                    .config_notes
                    .iter()
                    .any(|note| note.starts_with("image kind none (profiles/systemcore.toml)")),
                "{:?}",
                spec.metadata.config_notes
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}
