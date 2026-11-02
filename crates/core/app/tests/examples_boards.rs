//! The board examples (`examples/boards/`) and `examples/sim-x86_64`
//! validate with no errors or warnings and plan an image build, so a config
//! change cannot break them unnoticed. Building them needs the vendor SDKs;
//! this only reads the configs.
//!
//! The Luckfox Pico Pro imports Atlas's device package from its public git
//! repository, so its first run fetches Atlas into the example's
//! (git-ignored) `.gaia/cache`.

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};

const EXAMPLES: &[&str] = &[
    "boards/rock-5c",
    "boards/rock-4d",
    "boards/k230d-zero",
    "boards/luckfox-pico-pro",
    "boards/cast-receiver/cm4",
    "boards/cast-receiver/pi5",
    "sim-x86_64",
];

fn args(command: AppCommand, example: &str) -> AppArgs {
    AppArgs {
        command,
        build: format!(
            "{}/../../../examples/{example}/build.toml",
            env!("CARGO_MANIFEST_DIR")
        ),
        ..AppArgs::default()
    }
}

#[test]
fn every_board_example_validates_cleanly() {
    for example in EXAMPLES {
        match run_with_args(args(AppCommand::Validate, example)) {
            CommandOutcome::Validated { validation, .. } => {
                assert!(
                    validation.errors.is_empty(),
                    "{example}: {:?}",
                    validation.errors
                );
                assert!(
                    validation.warnings.is_empty(),
                    "{example}: {:?}",
                    validation.warnings
                );
            }
            other => panic!("{example}: expected a validated outcome, got {other:?}"),
        }
    }
}

#[test]
fn every_board_example_plans_an_image_build() {
    for example in EXAMPLES {
        match run_with_args(args(AppCommand::Plan, example)) {
            CommandOutcome::Planned {
                plan, diagnostics, ..
            } => {
                assert!(diagnostics.is_empty(), "{example}: {diagnostics:?}");
                let ids = plan
                    .operations
                    .iter()
                    .map(|operation| operation.id.as_str().to_string())
                    .collect::<Vec<_>>();
                for wanted in ["image:build", "image:assembly"] {
                    assert!(
                        ids.iter().any(|id| id == wanted),
                        "{example}: no {wanted} in {ids:?}"
                    );
                }
            }
            other => panic!("{example}: expected a planned outcome, got {other:?}"),
        }
    }
}
