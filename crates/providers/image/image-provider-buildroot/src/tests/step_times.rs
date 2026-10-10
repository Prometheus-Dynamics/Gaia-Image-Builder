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
