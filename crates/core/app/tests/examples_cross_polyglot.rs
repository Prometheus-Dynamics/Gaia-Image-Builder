pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use support::{
    polyglot_example_build_path, starting_point_cross_target_git_example_build_path,
    starting_point_polyglot_git_example_build_path,
};

#[test]
fn polyglot_buildroot_example_validates_and_plans_cleanly() {
    let build_path = polyglot_example_build_path();

    let validate = run_with_args(AppArgs {
        command: AppCommand::Validate,
        build: build_path.clone(),
        ..AppArgs::default()
    });
    match validate {
        CommandOutcome::Validated { spec, validation } => {
            assert!(
                validation.errors.is_empty(),
                "expected no validation errors"
            );
            assert!(
                validation.warnings.is_empty(),
                "expected no validation warnings"
            );
            assert_eq!(spec.sources.len(), 6);
            assert_eq!(spec.artifacts.len(), 5);
            assert_eq!(spec.install.entries.len(), 5);
            assert_eq!(spec.stage.files.len(), 2);
            assert_eq!(spec.stage.env_sets.len(), 1);
            assert_eq!(spec.stage.services.len(), 1);
            let gaia_spec::ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
                panic!("expected buildroot image definition");
            };
            assert_eq!(
                buildroot.source.as_ref().map(|source| source.as_str()),
                Some("buildroot-source")
            );
            assert_eq!(
                buildroot.defconfig_path.as_deref(),
                Some("@assets/buildroot-polyglot-squashfs.defconfig")
            );
            assert_eq!(buildroot.expected_images.len(), 1);
            assert_eq!(buildroot.expected_images[0].name, "rootfs.squashfs");
            assert_eq!(
                buildroot.expected_images[0].format,
                gaia_spec::BuildrootExpectedImageFormatSpec::Squashfs
            );
        }
        other => panic!("expected validated outcome, got {other:?}"),
    }

    let plan = run_with_args(AppArgs {
        command: AppCommand::Plan,
        build: build_path,
        ..AppArgs::default()
    });
    match plan {
        CommandOutcome::Planned {
            spec,
            plan,
            diagnostics,
        } => {
            assert!(diagnostics.is_empty(), "expected no plan diagnostics");
            assert_eq!(spec.image.feed.install_entries.len(), 5);
            assert_eq!(spec.image.feed.stage_files.len(), 2);
            assert_eq!(spec.image.feed.stage_env_sets.len(), 1);
            assert_eq!(spec.image.feed.stage_services.len(), 1);
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:build")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}

#[test]
fn starting_point_cross_target_git_example_validates_and_plans_cleanly() {
    let build_path = starting_point_cross_target_git_example_build_path();

    let validate = run_with_args(AppArgs {
        command: AppCommand::Validate,
        build: build_path.clone(),
        ..AppArgs::default()
    });
    match validate {
        CommandOutcome::Validated { spec, validation } => {
            assert!(
                validation.errors.is_empty(),
                "expected no validation errors"
            );
            assert!(
                validation.warnings.is_empty(),
                "expected no validation warnings"
            );
            assert_eq!(spec.sources.len(), 3);
            assert_eq!(spec.artifacts.len(), 2);
            assert_eq!(spec.install.entries.len(), 2);
            assert_eq!(spec.stage.files.len(), 1);
            let rust_artifact = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id.as_str() == "rust-app")
                .expect("rust artifact");
            let go_artifact = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id.as_str() == "go-app")
                .expect("go artifact");
            assert_eq!(
                rust_artifact.target.as_deref(),
                Some("aarch64-unknown-linux-gnu")
            );
            assert_eq!(go_artifact.target.as_deref(), Some("linux/arm64"));
        }
        other => panic!("expected validated outcome, got {other:?}"),
    }

    let plan = run_with_args(AppArgs {
        command: AppCommand::Plan,
        build: build_path,
        ..AppArgs::default()
    });
    match plan {
        CommandOutcome::Planned {
            spec,
            plan,
            diagnostics,
        } => {
            assert!(diagnostics.is_empty(), "expected no plan diagnostics");
            assert_eq!(spec.image.feed.install_entries.len(), 2);
            assert_eq!(spec.image.feed.stage_files.len(), 1);
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:build")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}

#[test]
fn starting_point_polyglot_git_example_validates_and_plans_cleanly() {
    let build_path = starting_point_polyglot_git_example_build_path();

    let validate = run_with_args(AppArgs {
        command: AppCommand::Validate,
        build: build_path.clone(),
        ..AppArgs::default()
    });
    match validate {
        CommandOutcome::Validated { spec, validation } => {
            assert!(
                validation.errors.is_empty(),
                "expected no validation errors"
            );
            assert_eq!(spec.sources.len(), 6);
            assert_eq!(spec.artifacts.len(), 5);
            assert_eq!(spec.install.entries.len(), 5);
            assert_eq!(spec.stage.files.len(), 1);
            assert_eq!(spec.stage.env_sets.len(), 1);
            assert_eq!(spec.stage.services.len(), 1);
            let gaia_spec::ImageDefinition::StartingPoint(starting_point) = &spec.image.definition
            else {
                panic!("expected starting-point image definition");
            };
            assert_eq!(
                starting_point.source.as_ref().map(|source| source.as_str()),
                Some("base-rootfs")
            );
            assert_eq!(
                starting_point.source_path.as_deref(),
                Some("content/base-rootfs.tar")
            );
        }
        other => panic!("expected validated outcome, got {other:?}"),
    }

    let plan = run_with_args(AppArgs {
        command: AppCommand::Plan,
        build: build_path,
        ..AppArgs::default()
    });
    match plan {
        CommandOutcome::Planned {
            spec,
            plan,
            diagnostics,
        } => {
            assert!(diagnostics.is_empty(), "expected no plan diagnostics");
            assert_eq!(spec.image.feed.install_entries.len(), 5);
            assert_eq!(spec.image.feed.stage_files.len(), 1);
            assert_eq!(spec.image.feed.stage_env_sets.len(), 1);
            assert_eq!(spec.image.feed.stage_services.len(), 1);
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "source:rust-source")
            );
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "source:python-source")
            );
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:build")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}
