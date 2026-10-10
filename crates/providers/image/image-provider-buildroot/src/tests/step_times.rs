use super::*;

#[test]
fn a_failed_make_still_reports_its_step_times() {
    let workspace = temp_path("gaia-step-times-workspace");
    let buildroot_dir = temp_path("gaia-step-times-source");
    let output_dir = temp_path("gaia-step-times-output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\n' > $(O)/.config\n\
         target-finalize: all\nall:\n\t@exit 3\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("step-times");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let error = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect_err("make fails");
    let steps = error
        .step_times
        .iter()
        .filter_map(|message| gaia_process::parse_step_time(message))
        .map(|(step, _)| step)
        .collect::<Vec<_>>();
    assert!(
        steps.iter().any(|step| step == "buildroot make"),
        "{steps:?}"
    );
}

#[test]
fn prepare_finalizes_the_target_without_making_images() {
    let workspace = temp_path("gaia-buildroot-prepare-images-workspace");
    let buildroot_dir = temp_path("gaia-buildroot-prepare-images-source");
    let output_dir = temp_path("gaia-buildroot-prepare-images-output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\n' > $(O)/.config\n\
         olddefconfig:\n\t@true\ntarget-finalize:\n\t@mkdir -p $(O)/target\n\t@echo finalize >> $(O)/make-log\n\
         all: target-finalize\n\t@echo images >> $(O)/make-log\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-prepare-images");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    };
    let execution = test_execution();
    for parallel_packages in [true, false] {
        // A fresh tree each: switching parallel_packages cleans the tree.
        let output_dir = output_dir.join(parallel_packages.to_string());
        let policy = ImageExecutionPolicy {
            parallel_packages,
            ..ImageExecutionPolicy::default()
        };
        // The prepare operation: the build operation makes the images.
        run_buildroot(BuildrootRunRequest {
            spec: &spec,
            image: &image,
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        })
        .expect("prepare");
        assert_eq!(
            fs::read_to_string(output_dir.join("make-log")).expect("make log"),
            "finalize\n",
            "parallel_packages = {parallel_packages}"
        );
    }
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn a_build_operations_steps_cover_its_duration() {
    let workspace = temp_path("gaia-step-cover-workspace");
    let source = workspace.join("build/sources/fake-buildroot");
    fs::create_dir_all(&source).expect("buildroot source");
    // Every target sleeps, so the operation's time is in its steps rather
    // than in process start-up.
    fs::write(
        source.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\n' > $(O)/.config\n\t@sleep 0.1\n\
         olddefconfig:\n\t@true\ntarget-finalize:\n\t@mkdir -p $(O)/target\n\t@sleep 0.2\n\
         all: target-finalize\n\t@mkdir -p $(O)/images\n\t@sleep 0.2\n\t@printf 'rootfs' > $(O)/images/rootfs.img\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("step-cover");
    spec.workspace.root_dir = workspace.display().to_string();
    spec.workspace.build_dir = "build".into();
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            source: Some(SourceId::new("fake-buildroot")),
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.img".into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    };
    let output = ImageOutputContract {
        collect_dir: Some(workspace.join("collect").display().to_string()),
        archive_name: None,
        emit_report: false,
    };
    let policy = ImageExecutionPolicy::default();
    let started = Instant::now();
    let result = BuildrootImageProvider
        .execute_image_operation(gaia_image_providers::ImageOperationExecution {
            spec: &spec,
            image: &image,
            operation: gaia_image_providers::ImageProviderOperation::Build,
            output: &output,
            policy: &policy,
            log_sink: None,
            cancel_check: None,
        })
        .expect("build operation");
    let elapsed = started.elapsed();

    let steps = result
        .messages
        .iter()
        .filter_map(|message| gaia_process::parse_step_time(message))
        .collect::<Vec<_>>();
    let names = steps
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "work dir placement",
        "buildroot source mirror",
        "host tools probe",
        "config steps",
        "tree changes",
        "config override check",
        "trash purge",
        "package graph load",
        "clean planning",
        "build state records",
        "package cache setup",
        "buildroot make",
        "make finish",
        "kernel modules check",
        "collect expected images",
        "buildroot state digest",
        "image output files",
        "image content digests",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    // The steps that overlap the make (derived from its log) are not
    // counted; this fake tree writes no log, so none are reported anyway.
    let covered: Duration = steps
        .iter()
        .filter(|(name, _)| {
            name != "buildroot finalize and images" && !name.starts_with("buildroot package ")
        })
        .map(|(_, duration)| *duration)
        .sum();
    assert!(
        covered.as_secs_f64() >= 0.9 * elapsed.as_secs_f64(),
        "steps cover {covered:?} of {elapsed:?}: {names:?}"
    );
    let _ = fs::remove_dir_all(workspace);
}
