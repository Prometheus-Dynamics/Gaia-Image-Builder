use super::*;

/// A fake Buildroot: each config step and the finalize appends its name to
/// `calls` in the output dir.
const FAKE_BUILDROOT: &str = ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@echo defconfig >> $(O)/calls\n\t@printf 'BR2_PACKAGE_FOO=n\\n' > $(O)/.config\n\
olddefconfig:\n\t@echo olddefconfig >> $(O)/calls\n\
clean:\n\t@true\n\
target-finalize:\n\t@echo finalize >> $(O)/calls\n\
all: target-finalize\n\t@true\n";

/// Whether `step` (a whole line) ran, from the fake's call log.
fn ran(calls: &str, step: &str) -> bool {
    calls.lines().any(|line| line == step)
}

fn image(overrides: Vec<(String, String)>) -> ImageSpec {
    ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_overrides: overrides,
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    }
}

#[test]
fn unchanged_config_inputs_skip_the_config_steps_until_an_input_changes() {
    let buildroot_dir = temp_path("gaia-config-skip-source");
    let output_dir = temp_path("gaia-config-skip-output");
    let workspace = temp_path("gaia-config-skip-workspace");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(buildroot_dir.join("Makefile"), FAKE_BUILDROOT).expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-config-skip");
    spec.workspace.root_dir = workspace.display().to_string();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let run = |overrides: Vec<(String, String)>| {
        let _ = fs::remove_file(output_dir.join("calls"));
        let messages = run_buildroot_with(
            BuildrootRunRequest {
                spec: &spec,
                image: &image(overrides),
                buildroot_dir: &buildroot_dir,
                output_dir: &output_dir,
                command: test_command_context(&execution, &policy),
            },
            BuildrootMakeOptions::default(),
        )
        .expect("buildroot run");
        let calls = fs::read_to_string(output_dir.join("calls")).unwrap_or_default();
        (messages, calls)
    };
    let foo_y = vec![("BR2_PACKAGE_FOO".to_string(), "y".to_string())];

    let (_, first) = run(foo_y.clone());
    assert!(
        ran(&first, "defconfig") && ran(&first, "olddefconfig"),
        "{first}"
    );

    let (messages, second) = run(foo_y.clone());
    assert!(!ran(&second, "defconfig"), "config steps re-ran:\n{second}");
    assert!(
        ran(&second, "finalize"),
        "make itself still runs:\n{second}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("config steps skipped")),
        "{messages:?}"
    );

    // A changed override runs the steps again.
    let foo_n = vec![("BR2_PACKAGE_FOO".to_string(), "n".to_string())];
    let (_, third) = run(foo_n);
    assert!(ran(&third, "defconfig"), "{third}");

    // A `.config` edited behind Gaia's back is rebuilt from the inputs.
    fs::write(output_dir.join(".config"), "BR2_PACKAGE_FOO=y\n# edited\n").expect("edit");
    let (_, fourth) = run(vec![("BR2_PACKAGE_FOO".to_string(), "n".to_string())]);
    assert!(
        ran(&fourth, "defconfig"),
        "edited config was kept:\n{fourth}"
    );

    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn a_changed_kconfig_file_of_the_buildroot_tree_runs_the_config_steps() {
    let buildroot_dir = temp_path("gaia-config-kconfig-source");
    let output_dir = temp_path("gaia-config-kconfig-output");
    fs::create_dir_all(buildroot_dir.join("package/foo")).expect("package dir");
    fs::write(buildroot_dir.join("Makefile"), FAKE_BUILDROOT).expect("makefile");
    fs::write(
        buildroot_dir.join("package/foo/Config.in"),
        "config BR2_PACKAGE_FOO\n",
    )
    .expect("kconfig");
    let mut spec = ResolvedBuildSpec::new("buildroot-config-kconfig");
    spec.workspace.root_dir = temp_path("gaia-config-kconfig-workspace")
        .display()
        .to_string();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let run = || {
        let _ = fs::remove_file(output_dir.join("calls"));
        run_buildroot_with(
            BuildrootRunRequest {
                spec: &spec,
                image: &image(Vec::new()),
                buildroot_dir: &buildroot_dir,
                output_dir: &output_dir,
                command: test_command_context(&execution, &policy),
            },
            BuildrootMakeOptions::default(),
        )
        .expect("buildroot run");
        fs::read_to_string(output_dir.join("calls")).unwrap_or_default()
    };
    assert!(ran(&run(), "defconfig"));
    assert!(
        !ran(&run(), "defconfig"),
        "unchanged Kconfig re-ran the steps"
    );
    fs::write(
        buildroot_dir.join("package/foo/Config.in"),
        "config BR2_PACKAGE_FOO\n\tbool \"changed\"\n",
    )
    .expect("kconfig edit");
    assert!(ran(&run(), "defconfig"), "changed Kconfig kept the steps");
    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn make_gets_source_date_epoch_from_the_workspace_head_commit() {
    if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
        return; // the caller's own value wins; nothing to check here
    }
    let workspace = temp_path("gaia-epoch-workspace");
    let buildroot_dir = temp_path("gaia-epoch-source");
    let output_dir = temp_path("gaia-epoch-output");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(args)
            .env("GIT_AUTHOR_NAME", "gaia")
            .env("GIT_AUTHOR_EMAIL", "gaia@example.invalid")
            .env("GIT_COMMITTER_NAME", "gaia")
            .env("GIT_COMMITTER_EMAIL", "gaia@example.invalid")
            .env("GIT_COMMITTER_DATE", "1710000000 +0000")
            .env("GIT_AUTHOR_DATE", "1710000000 +0000")
            .status()
    };
    match git(&["init", "-q"]) {
        Ok(status) if status.success() => {}
        _ => return, // no git here
    }
    fs::write(workspace.join("file"), "x").expect("file");
    assert!(git(&["add", "file"]).is_ok_and(|s| s.success()));
    assert!(git(&["commit", "-q", "-m", "one"]).is_ok_and(|s| s.success()));
    fs::write(
        buildroot_dir.join("Makefile"),
        "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=n\\n' > $(O)/.config\n\
olddefconfig:\n\t@true\nclean:\n\t@true\n\
target-finalize:\n\t@printf '%s' \"$$SOURCE_DATE_EPOCH\" > $(O)/epoch\n\
all: target-finalize\n\t@true\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-epoch");
    spec.workspace.root_dir = workspace.display().to_string();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image(Vec::new()),
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("buildroot run");
    assert_eq!(
        fs::read_to_string(output_dir.join("epoch")).expect("epoch"),
        "1710000000"
    );
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}
