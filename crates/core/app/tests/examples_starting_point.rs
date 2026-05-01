pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use support::{
    starting_point_example_build_path, starting_point_git_project_example_build_path,
    starting_point_raw_image_example_build_path,
};

#[test]
fn starting_point_raw_image_example_validates_and_plans_cleanly() {
    let build_path = starting_point_raw_image_example_build_path();

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
            assert_eq!(spec.sources.len(), 1);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            let gaia_spec::ImageDefinition::StartingPoint(starting_point) = &spec.image.definition
            else {
                panic!("expected starting-point image definition");
            };
            assert!(starting_point.source.is_none());
            assert!(starting_point.source_path.is_none());
            assert!(starting_point.rootfs_path.ends_with("seed/base.img"));
            assert!(!starting_point.image_read_only);
            assert!(starting_point.packages.enabled);
            assert!(starting_point.packages.execute);
            assert_eq!(starting_point.packages.manager.as_deref(), Some("apk"));
            assert_eq!(starting_point.packages.install, vec!["curl".to_string()]);
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
            assert_eq!(spec.image.feed.install_entries.len(), 1);
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
fn starting_point_rootfs_smoke_example_validates_and_plans_cleanly() {
    let build_path = starting_point_example_build_path();

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
            assert!(spec.sources.is_empty());
            assert!(spec.artifacts.is_empty());
            assert!(spec.install.entries.is_empty());
            assert!(spec.stage.files.is_empty());
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::StartingPoint
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
            plan, diagnostics, ..
        } => {
            assert!(diagnostics.is_empty(), "expected no plan diagnostics");
            assert_eq!(plan.operations.len(), 3);
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
fn starting_point_git_project_example_validates_and_plans_cleanly() {
    let build_path = starting_point_git_project_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
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
            assert_eq!(spec.image.feed.install_entries.len(), 1);
            assert_eq!(spec.image.feed.stage_files.len(), 1);
            assert_eq!(spec.image.feed.stage_env_sets.len(), 1);
            assert_eq!(spec.image.feed.stage_services.len(), 1);
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "source:app-source")
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
