use super::*;

fn ran(calls: &str, step: &str) -> bool {
    calls.lines().any(|line| line == step)
}

#[test]
fn the_finalize_marker_holds_only_for_the_config_it_was_recorded_with() {
    let output = temp_path("gaia-finalize-marker");
    fs::create_dir_all(&output).expect("output");
    assert!(!finalized_for(&output, "d1"));
    record_finalized(&output, "d1");
    assert!(finalized_for(&output, "d1"));
    assert!(!finalized_for(&output, "d2"));
    invalidate_finalized(&output);
    assert!(!finalized_for(&output, "d1"));
    let _ = fs::remove_dir_all(output);
}

#[test]
fn a_tree_builds_nothing_only_when_every_package_with_a_build_dir_is_installed() {
    let output = temp_path("gaia-finalize-installed");
    fs::create_dir_all(output.join("build/foo-1")).expect("build dir");
    let mut graph = PackageGraph::default();
    graph.packages.insert(
        "foo".to_string(),
        PackageInfo {
            stamp_dir: Some("build/foo-1".to_string()),
            ..PackageInfo::default()
        },
    );
    graph
        .packages
        .insert("virtual-thing".to_string(), PackageInfo::default());
    assert!(!all_packages_installed(&output, &graph));
    fs::write(output.join("build/foo-1/.stamp_installed"), "").expect("stamp");
    assert!(all_packages_installed(&output, &graph));
    let _ = fs::remove_dir_all(output);
}

/// The split build (parallel packages): a make that finalizes, then images.
#[test]
fn an_unchanged_built_tree_skips_the_repeated_finalize_of_the_build_operation() {
    let buildroot_dir = temp_path("gaia-finalize-skip-source");
    let output_dir = temp_path("gaia-finalize-skip-output");
    let workspace = temp_path("gaia-finalize-skip-workspace");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("graph.json"),
        "{\"foo\": {\"type\": \"target\", \"name\": \"foo\", \"virtual\": false, \
         \"version\": \"1\", \"stamp_dir\": \"build/foo-1\", \
         \"dependencies\": [], \"reverse_dependencies\": []}}\n",
    )
    .expect("graph");
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\n' > $(O)/.config\n\
olddefconfig:\n\t@true\nclean:\n\t@true\nshow-info:\n\t@cat graph.json\n\
target-finalize: $(O)/build/foo-1/.stamp_installed\n\t@echo finalize >> $(O)/calls\n\t@mkdir -p $(O)/target\n\
$(O)/build/foo-1/.stamp_installed:\n\t@mkdir -p $(dir $@) && touch $@ && echo foo-built >> $(O)/calls\n\
all:\n\t@mkdir -p $(O)/images && touch $(O)/images/rootfs.img && echo images >> $(O)/calls\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-finalize-skip");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = |value: &str| ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_overrides: vec![("BR2_PACKAGE_FOO".into(), value.into())],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
    };
    let execution = test_execution();
    let mut policy = ImageExecutionPolicy {
        parallel_packages: true,
        ..ImageExecutionPolicy::default()
    };
    policy.package_cache.enabled = false;
    let run = |value: &str| {
        let _ = fs::remove_file(output_dir.join("calls"));
        let messages = run_buildroot_with(
            BuildrootRunRequest {
                spec: &spec,
                image: &image(value),
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

    // First run builds foo and finalizes; later runs with nothing changed
    // skip the finalize once the images are current.
    let (_, first) = run("y");
    assert!(
        ran(&first, "foo-built") && ran(&first, "finalize"),
        "{first}"
    );
    let (_, second) = run("y");
    let (_, third) = run("y");
    let (messages, skipped) = run("y");
    assert!(
        !ran(&skipped, "finalize") && !ran(&skipped, "foo-built"),
        "unchanged tree re-finalized (second={second:?} third={third:?}):\n{skipped}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("skipped target-finalize")),
        "{messages:?}"
    );

    // A changed config makes the finalize run again.
    let (_, changed) = run("n");
    assert!(ran(&changed, "finalize"), "{changed}");

    let _ = fs::remove_dir_all(buildroot_dir);
    let _ = fs::remove_dir_all(output_dir);
}
