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
         all:\n\t@exit 3\n",
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
