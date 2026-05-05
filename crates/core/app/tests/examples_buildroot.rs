pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use support::{
    raspberrypi4_go_example_build_path, rust_aarch64_example_build_path,
    sdcard_smoke_example_build_path, smoke_example_build_path, squashfs_smoke_example_build_path,
};

#[test]
fn rust_buildroot_smoke_example_validates_and_plans_cleanly() {
    let build_path = smoke_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            assert!(spec.stage.env_sets.is_empty());
            assert!(spec.stage.services.is_empty());
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
fn rust_buildroot_aarch64_smoke_example_validates_and_plans_cleanly() {
    let build_path = rust_aarch64_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            assert_eq!(
                spec.artifacts[0].target.as_deref(),
                Some("aarch64-unknown-linux-gnu")
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
fn rust_buildroot_squashfs_smoke_example_validates_and_plans_cleanly() {
    let build_path = squashfs_smoke_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::Buildroot
            );
            let gaia_spec::ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
                panic!("expected buildroot image definition");
            };
            assert_eq!(
                buildroot.defconfig_path.as_deref(),
                Some("@assets/buildroot-squashfs-smoke.defconfig")
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
fn rust_buildroot_sdcard_smoke_example_validates_and_plans_cleanly() {
    let build_path = sdcard_smoke_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::Buildroot
            );
            let gaia_spec::ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
                panic!("expected buildroot image definition");
            };
            assert_eq!(
                buildroot.defconfig_path.as_deref(),
                Some("@assets/buildroot-sdcard-smoke.defconfig")
            );
            assert_eq!(buildroot.expected_images.len(), 1);
            assert_eq!(buildroot.expected_images[0].name, "sdcard.img");
            assert_eq!(
                buildroot.expected_images[0].format,
                gaia_spec::BuildrootExpectedImageFormatSpec::Raw
            );
            assert!(
                spec.image.assembly.is_some(),
                "sdcard example should use typed assembly for raw disk output"
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
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:build")
            );
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:assembly")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}

#[test]
fn go_buildroot_raspberrypi4_smoke_example_validates_and_plans_cleanly() {
    let build_path = raspberrypi4_go_example_build_path();

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
            assert_eq!(spec.sources.len(), 2);
            assert_eq!(spec.artifacts.len(), 1);
            assert_eq!(spec.artifacts[0].target.as_deref(), Some("linux/arm64"));
            assert_eq!(spec.install.entries.len(), 1);
            assert_eq!(spec.stage.files.len(), 1);
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::Buildroot
            );
            let gaia_spec::ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
                panic!("expected buildroot image definition");
            };
            assert!(
                buildroot.defconfig_path.is_some(),
                "expected board-style example to use defconfig_path"
            );
            assert_eq!(
                buildroot.config_fragments,
                vec!["@assets/buildroot-board.fragment".to_string()]
            );
            assert_eq!(
                buildroot.config_overrides,
                vec![
                    ("BR2_PACKAGE_HOST_GENIMAGE".to_string(), "y".to_string()),
                    (
                        "BR2_TARGET_ROOTFS_EXT2_SIZE".to_string(),
                        "\"64M\"".to_string()
                    ),
                ]
            );
            assert_eq!(buildroot.expected_images.len(), 1);
            assert_eq!(buildroot.expected_images[0].name, "sdcard.img");
            assert_eq!(
                buildroot.expected_images[0].format,
                gaia_spec::BuildrootExpectedImageFormatSpec::Raw
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
            let gaia_spec::ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
                panic!("expected buildroot image definition");
            };
            assert!(
                buildroot.defconfig_path.is_some(),
                "expected board-style example to keep custom defconfig_path"
            );
            assert_eq!(
                buildroot.config_fragments,
                vec!["@assets/buildroot-board.fragment".to_string()]
            );
            assert_eq!(buildroot.config_overrides.len(), 2);
            assert_eq!(buildroot.expected_images[0].name, "sdcard.img");
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id.as_str() == "image:build")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}
