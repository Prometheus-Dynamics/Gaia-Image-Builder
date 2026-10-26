//! The `kind = "none"` example (`examples/systemcore-package`): it validates
//! and plans with no image operation.

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};

fn example_build_path() -> String {
    format!(
        "{}/../../../examples/systemcore-package/build.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn the_systemcore_package_example_validates_and_plans_with_no_image() {
    let build = example_build_path();

    match run_with_args(AppArgs {
        command: AppCommand::Validate,
        build: build.clone(),
        ..AppArgs::default()
    }) {
        CommandOutcome::Validated { spec, validation } => {
            assert!(validation.errors.is_empty(), "{:?}", validation.errors);
            assert!(validation.warnings.is_empty(), "{:?}", validation.warnings);
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::None
            );
            assert!(spec.artifacts.is_empty());
            assert_eq!(spec.sources.len(), 1);
        }
        other => panic!("expected validated outcome, got {other:?}"),
    }

    match run_with_args(AppArgs {
        command: AppCommand::Plan,
        build,
        ..AppArgs::default()
    }) {
        CommandOutcome::Planned {
            plan, diagnostics, ..
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let ids: Vec<&str> = plan
                .operations
                .iter()
                .map(|operation| operation.id.as_str())
                .collect();
            assert!(ids.contains(&"image:assembly"), "{ids:?}");
            assert!(
                ids.iter().all(|id| !id.starts_with("image:build")
                    && !id.starts_with("image:prepare")
                    && !id.starts_with("image:check")),
                "{ids:?}"
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}
