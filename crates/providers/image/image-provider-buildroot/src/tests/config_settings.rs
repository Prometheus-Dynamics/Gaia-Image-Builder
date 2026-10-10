use super::*;

/// A fake Buildroot whose olddefconfig records the `.config` it was given,
/// and whose defconfig writes a `.config` with a `# ... is not set` line.
const FAKE_BUILDROOT: &str = ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@echo defconfig >> $(O)/calls\n\
\t@printf '# BR2_PACKAGE_BUSYBOX is not set\\nBR2_TARGET_ROOTFS_TAR=y\\n' > $(O)/.config\n\
olddefconfig:\n\t@echo olddefconfig >> $(O)/calls\n\t@cat $(O)/.config >> $(O)/olddefconfig-input\n\t@echo --- >> $(O)/olddefconfig-input\n\
clean:\n\t@true\n\
target-finalize:\n\t@true\n\
all: target-finalize\n\t@true\n";

fn image(overrides: Vec<(String, String)>, fragments: Vec<String>) -> ImageSpec {
    ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_fragments: fragments,
            config_overrides: overrides,
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    }
}

/// The `.config` texts the olddefconfig runs saw, one per run, and the calls.
fn runs(output_dir: &Path) -> (Vec<String>, Vec<String>) {
    let seen = fs::read_to_string(output_dir.join("olddefconfig-input")).unwrap_or_default();
    let calls = fs::read_to_string(output_dir.join("calls")).unwrap_or_default();
    (
        seen.split("---\n")
            .filter(|block| !block.is_empty())
            .map(str::to_string)
            .collect(),
        calls.lines().map(str::to_string).collect(),
    )
}

#[test]
fn overrides_and_cache_settings_take_one_olddefconfig_run_with_the_old_merge() {
    let workspace = temp_path("gaia-config-settings-workspace");
    let buildroot_dir = temp_path("gaia-config-settings-source");
    let output_dir = temp_path("gaia-config-settings-output");
    fs::create_dir_all(workspace.join("assets")).expect("assets dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(buildroot_dir.join("Makefile"), FAKE_BUILDROOT).expect("makefile");
    fs::write(
        workspace.join("assets/fragment.cfg"),
        "BR2_PACKAGE_DROPBEAR=y\n",
    )
    .expect("fragment");
    let mut spec = ResolvedBuildSpec::new("buildroot-config-settings");
    spec.workspace.root_dir = workspace.display().to_string();
    let execution = test_execution();
    let policy = ImageExecutionPolicy {
        parallel_packages: true,
        ..ImageExecutionPolicy::default()
    };
    // An override of a fragment's value, a `# ... is not set` line it
    // replaces, and the cache setting Gaia adds last.
    let overrides = vec![
        ("BR2_PACKAGE_BUSYBOX".to_string(), "y".to_string()),
        ("BR2_PACKAGE_DROPBEAR".to_string(), "n".to_string()),
    ];
    run_buildroot_with(
        BuildrootRunRequest {
            spec: &spec,
            image: &image(overrides, vec!["assets/fragment.cfg".into()]),
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        },
        BuildrootMakeOptions::default(),
    )
    .expect("buildroot run");

    let (seen, calls) = runs(&output_dir);
    // defconfig, the fragments' olddefconfig, then one run for the overrides
    // and the cache settings together.
    assert_eq!(
        calls,
        ["defconfig", "olddefconfig", "olddefconfig"],
        "{calls:?}"
    );
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(
        seen[1],
        "BR2_TARGET_ROOTFS_TAR=y\n\nBR2_PACKAGE_BUSYBOX=y\nBR2_PACKAGE_DROPBEAR=n\nBR2_PER_PACKAGE_DIRECTORIES=y\n"
    );
    assert_eq!(
        fs::read_to_string(output_dir.join(".config")).expect("config"),
        seen[1]
    );

    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn without_overrides_or_cache_settings_no_olddefconfig_runs() {
    let buildroot_dir = temp_path("gaia-config-settings-none-source");
    let output_dir = temp_path("gaia-config-settings-none-output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(buildroot_dir.join("Makefile"), FAKE_BUILDROOT).expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-config-settings-none");
    spec.workspace.root_dir = temp_path("gaia-config-settings-none-workspace")
        .display()
        .to_string();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    run_buildroot_with(
        BuildrootRunRequest {
            spec: &spec,
            image: &image(Vec::new(), Vec::new()),
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        },
        BuildrootMakeOptions::default(),
    )
    .expect("buildroot run");

    let (seen, calls) = runs(&output_dir);
    assert_eq!(calls, ["defconfig"], "{calls:?}");
    assert!(seen.is_empty(), "{seen:?}");

    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}
