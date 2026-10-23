use super::*;

#[test]
fn run_buildroot_reports_make_failures() {
    let spec = ResolvedBuildSpec::new("buildroot-test");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-test-dir");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-test-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
            buildroot_dir.join("Makefile"),
            "bad_defconfig:\n\t@echo defconfig failed 1>&2\n\t@false\ntarget-finalize: all\nall:\n\t@echo build failed 1>&2\n\t@false\n",
        )
        .expect("makefile");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("bad_defconfig".into()),
            external_tree: None,
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
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
    .expect_err("failing buildroot make should error");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("buildroot defconfig failed"));
}

#[test]
fn run_command_reports_missing_tool() {
    let error = run_command(
        Command::new("gaia-missing-buildroot-tool"),
        "buildroot archive",
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect_err("missing tool should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::ToolStart);
    assert!(error.message.contains("failed to start buildroot archive"));
}

#[test]
fn run_command_times_out_long_running_processes() {
    let policy = ImageExecutionPolicy {
        timeout_seconds: 1,
        ..ImageExecutionPolicy::default()
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 5");

    let started = Instant::now();
    let error = run_command(
        command,
        "buildroot timeout smoke",
        &test_execution(),
        &policy,
        None,
        None,
    )
    .expect_err("sleep should time out");

    assert_eq!(error.kind, ImageProviderErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "timeout should interrupt before the child sleep completes"
    );
}

#[test]
fn make_jobs_are_provider_local_and_not_forced_by_scheduler_jobs() {
    let mut no_local_jobs = Command::new("make");
    append_make_jobs(&mut no_local_jobs, 0);
    assert!(
        no_local_jobs
            .get_args()
            .any(|arg| arg.to_string_lossy().starts_with("-j"))
    );

    let mut local_jobs = Command::new("make");
    append_make_jobs(&mut local_jobs, 2);
    assert!(local_jobs.get_args().any(|arg| arg == "-j2"));
}

#[test]
fn buildroot_cache_policy_sets_make_environment_and_creates_dirs() {
    let workspace_scratch = TempDir::new("gaia-buildroot-cache-workspace");
    let workspace = workspace_scratch.join("workspace");
    let mut spec = ResolvedBuildSpec::new("buildroot-cache-test");
    spec.workspace.root_dir = workspace.display().to_string();
    let policy = ImageExecutionPolicy {
        download_dir: Some(".gaia/cache/buildroot/dl".into()),
        ccache_enabled: true,
        ccache_dir: Some(".gaia/cache/buildroot/ccache".into()),
        ..ImageExecutionPolicy::default()
    };
    let mut command = Command::new("make");

    apply_buildroot_policy_env(&mut command, &spec, &policy).expect("cache env");

    let envs = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().to_string(),
                value
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert!(envs.iter().any(|(key, _)| key == "BR2_DL_DIR"));
    assert!(envs.iter().any(|(key, _)| key == "BR2_CCACHE_DIR"));
    assert!(workspace.join(".gaia/cache/buildroot/dl").is_dir());
    assert!(workspace.join(".gaia/cache/buildroot/ccache").is_dir());
}

#[test]
fn buildroot_default_caches_are_shared_by_every_workspace() {
    let policy = ImageExecutionPolicy {
        ccache_enabled: true,
        ..ImageExecutionPolicy::default()
    };
    let envs = |workspace: &str| {
        let mut spec = ResolvedBuildSpec::new("buildroot-shared-cache");
        spec.workspace.root_dir = std::env::temp_dir().join(workspace).display().to_string();
        let mut command = Command::new("make");
        apply_buildroot_policy_env(&mut command, &spec, &policy).expect("cache env");
        command
            .get_envs()
            .filter_map(|(key, value)| {
                Some((key.to_string_lossy().to_string(), PathBuf::from(value?)))
            })
            .collect::<BTreeMap<_, _>>()
    };
    let first = envs("gaia-buildroot-shared-cache-a");
    let second = envs("gaia-buildroot-shared-cache-b");
    let root = user_cache_root().expect("user cache root");
    let root = fs::canonicalize(&root).unwrap_or(root);
    assert_eq!(first["BR2_DL_DIR"], root.join("buildroot/dl"));
    assert_eq!(first["BR2_CCACHE_DIR"], root.join("buildroot/ccache"));
    assert_eq!(first, second);
}

#[test]
fn ccache_max_size_is_written_to_the_cache_config() {
    let dir_scratch = TempDir::new("gaia-ccache-max-size");
    let dir = dir_scratch.join("dir");
    fs::create_dir_all(&dir).expect("dir");
    fs::write(
        dir.join("ccache.conf"),
        "max_size = 5G\nsloppiness = time_macros\n",
    )
    .expect("conf");
    ensure_ccache_max_size(&dir, "51200M").expect("default size");
    assert_eq!(
        fs::read_to_string(dir.join("ccache.conf")).expect("conf"),
        "sloppiness = time_macros\nmax_size = 51200M\n"
    );
    ensure_ccache_max_size(&dir, "120G").expect("size");
    assert!(
        fs::read_to_string(dir.join("ccache.conf"))
            .expect("conf")
            .ends_with("\nmax_size = 120G\n")
    );
    assert!(ensure_ccache_max_size(&dir, "lots").is_err());
    assert!(ensure_ccache_max_size(&dir, "50X").is_err());
}

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn the_default_ccache_size_follows_the_free_space() {
    // clamp(free / 2 + current, 2 GiB, 40 GiB), in MiB.
    assert_eq!(ccache_default_max_size(Some(20 * GIB), 4 * GIB), "14336M");
    assert_eq!(ccache_default_max_size(Some(100 * GIB), 10 * GIB), "40960M");
    assert_eq!(ccache_default_max_size(Some(100 * GIB), 0), "40960M");
    assert_eq!(ccache_default_max_size(Some(200 * GIB), 60 * GIB), "40960M");
    assert_eq!(ccache_default_max_size(Some(0), 0), "2048M");
    assert_eq!(ccache_default_max_size(None, 0), "40960M");
    // An explicit size is used as given, whatever the free space.
    let scratch = TempDir::new("gaia-ccache-explicit");
    assert_eq!(
        ccache_max_size_for_run(&scratch.join("ccache"), Some(" 50G ")),
        "50G"
    );
}

#[test]
fn the_ccache_size_is_run_time_only_and_not_in_the_config() {
    let spec = ResolvedBuildSpec::new("buildroot-ccache-size");
    let scratch = TempDir::new("gaia-ccache-config");
    let overrides = |max_size: Option<&str>| {
        let policy = ImageExecutionPolicy {
            ccache_enabled: true,
            ccache_dir: Some(scratch.join("ccache").display().to_string()),
            ccache_max_size: max_size.map(str::to_string),
            ..ImageExecutionPolicy::default()
        };
        buildroot_cache_overrides(&spec, &policy, true)
            .expect("overrides")
            .0
    };
    let default = overrides(None);
    let explicit = overrides(Some("3G"));
    // The size is in ccache.conf only: the .config overrides, which the
    // config digest and the fingerprint are built from, are the same for the
    // default and for any explicit size.
    assert_eq!(default, explicit);
    assert!(
        default.iter().all(|(key, _)| !key.contains("MAX_SIZE")),
        "{default:?}"
    );
}

#[test]
fn an_explicit_ccache_size_warns_only_when_it_exceeds_the_free_space() {
    let spec = ResolvedBuildSpec::new("buildroot-ccache-warn");
    let scratch = TempDir::new("gaia-ccache-warn");
    let warn = |max_size: Option<&str>| {
        let policy = ImageExecutionPolicy {
            ccache_enabled: true,
            ccache_dir: Some(scratch.join("ccache").display().to_string()),
            ccache_max_size: max_size.map(str::to_string),
            ..ImageExecutionPolicy::default()
        };
        buildroot_cache_overrides(&spec, &policy, true)
            .expect("overrides")
            .1
    };
    assert!(warn(None).is_empty(), "the default never warns");
    assert!(warn(Some("1K")).is_empty());
    assert!(
        !warn(Some("1000000000G")).is_empty(),
        "an explicit size far beyond the free space warns"
    );
}

#[test]
fn ccache_hit_rate_counts_this_runs_compilations() {
    let log = "# a.c\ncache_miss\ndirect_cache_miss\nlocal_storage_miss\n\
               # a.c\ndirect_cache_hit\nlocal_storage_hit\n\
               # b.c\ndirect_cache_miss\npreprocessed_cache_hit\n\
               # c.c\ndirect_cache_hit\n";
    assert_eq!(
        ccache_hit_rate(log).as_deref(),
        Some("buildroot ccache: 3/4 compilations from cache (75.0%)")
    );
    assert_eq!(ccache_hit_rate(""), None);
}

#[test]
fn buildroot_parallel_packages_and_ccache_configure_and_report() {
    let workspace_scratch = TempDir::new("gaia-buildroot-parallel-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-parallel-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-parallel-output");
    let output_dir = output_dir_scratch.join("output_dir");
    let ccache_dir_scratch = TempDir::new("gaia-buildroot-parallel-ccache");
    let ccache_dir = ccache_dir_scratch.join("ccache_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    // Like Buildroot: `target-finalize` builds (recording make's flags, a
    // ccache stats log like two compilations, one from the cache, and the
    // target tree from `app-version`); `all` also writes the images.
    fs::write(buildroot_dir.join("app-version"), "1").expect("app version");
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\n' > $(O)/.config\n\
         olddefconfig:\n\t@true\ntarget-finalize:\n\t@echo \"$(MAKEFLAGS)\" > $(O)/makeflags\n\
         \t@printf '# a.c\\ncache_miss\\n# b.c\\ndirect_cache_hit\\n' > \"$$CCACHE_STATSLOG\"\n\
         \t@mkdir -p $(O)/target/usr/bin && cp app-version $(O)/target/usr/bin/app\n\
         all: target-finalize\n\t@mkdir -p $(O)/images && cp $(O)/target/usr/bin/app $(O)/images/rootfs.img\n\
         \t@echo images >> $(O)/images-ran\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-parallel");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy {
        local_jobs: 6,
        parallel_packages: true,
        ccache_enabled: true,
        ccache_dir: Some(ccache_dir.display().to_string()),
        ..ImageExecutionPolicy::default()
    };
    let messages = run_buildroot_with(
        BuildrootRunRequest {
            spec: &spec,
            image: &image,
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        },
        // The build operation, which makes the images.
        BuildrootMakeOptions::default(),
    )
    .expect("buildroot run");

    let config = fs::read_to_string(output_dir.join(".config")).expect("config");
    for setting in [
        "BR2_PER_PACKAGE_DIRECTORIES=y",
        "BR2_CCACHE=y",
        "BR2_CCACHE_USE_BASEDIR=y",
    ] {
        assert!(
            config.contains(setting),
            "{setting} missing from:\n{config}"
        );
    }
    assert!(config.contains(&format!(
        "BR2_CCACHE_DIR=\"{}\"",
        fs::canonicalize(&ccache_dir).expect("ccache dir").display()
    )));
    // The default is computed from the free space at run start, so only its
    // shape is fixed here: whole MiB within the default's clamp.
    let conf = fs::read_to_string(ccache_dir.join("ccache.conf")).expect("ccache conf");
    let size = conf
        .strip_prefix("max_size = ")
        .and_then(|rest| rest.strip_suffix("M\n"))
        .and_then(|mib| mib.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("unexpected ccache.conf: {conf:?}"));
    assert!((2048..=40960).contains(&size), "{conf:?}");
    let makeflags = fs::read_to_string(output_dir.join("makeflags")).expect("makeflags");
    assert!(!makeflags.contains("-l"), "{makeflags}");
    assert!(
        messages
            .iter()
            .any(|message| message
                == "summary: buildroot ccache: 1/2 compilations from cache (50.0%)"),
        "{messages:?}"
    );
    let images_ran = || fs::read_to_string(output_dir.join("images-ran")).expect("images ran");
    assert_eq!(images_ran(), "images\n");

    // Nothing the images read changed: the image step is skipped.
    let run_again = || {
        run_buildroot_with(
            BuildrootRunRequest {
                spec: &spec,
                image: &image,
                buildroot_dir: &buildroot_dir,
                output_dir: &output_dir,
                command: test_command_context(&execution, &policy),
            },
            // The build operation, which makes the images.
            BuildrootMakeOptions::default(),
        )
        .expect("buildroot run")
    };
    let messages = run_again();
    assert_eq!(images_ran(), "images\n");
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("skipped buildroot filesystem images")),
        "{messages:?}"
    );
    // The target changed: the images are made again.
    fs::write(buildroot_dir.join("app-version"), "2").expect("app version");
    run_again();
    assert_eq!(images_ran(), "images\nimages\n");
    assert_eq!(
        fs::read_to_string(output_dir.join("images/rootfs.img")).expect("image"),
        "2"
    );
}

#[test]
fn buildroot_kconfig_string_value_escapes_quotes_and_backslashes() {
    let value = kconfig_string_value(r#"/tmp/cache/"quoted"\dir"#).expect("kconfig string");

    assert_eq!(value, r#""/tmp/cache/\"quoted\"\\dir""#);
}

#[test]
fn buildroot_kconfig_string_value_rejects_newlines() {
    let error = kconfig_string_value("/tmp/cache\nbad").expect_err("newline should be rejected");

    assert!(error.message.contains("newline"), "{error:?}");
}

#[test]
fn buildroot_cache_config_escapes_kconfig_paths() {
    let workspace_scratch = TempDir::new("gaia-buildroot-cache-escaping-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-cache-escaping-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-cache-escaping-output");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(output_dir.join(".config"), "BR2_TARGET_ROOTFS_TAR=y\n").expect("config");
    fs::write(buildroot_dir.join("Makefile"), "olddefconfig:\n\t@true\n").expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-cache-escaping-test");
    spec.workspace.root_dir = workspace.display().to_string();
    let policy = ImageExecutionPolicy {
        download_dir: Some(r#".gaia/cache/buildroot/"dl"\dir"#.into()),
        ..ImageExecutionPolicy::default()
    };
    let execution = test_execution();

    apply_buildroot_config_settings(BuildrootSettingsRequest {
        spec: &spec,
        output_dir: &output_dir,
        overrides: &[],
        external_tree: None,
        buildroot_dir: &buildroot_dir,
        command: test_command_context(&execution, &policy),
        dry_run: false,
    })
    .expect("cache config");

    let config = fs::read_to_string(output_dir.join(".config")).expect("config");
    assert!(config.contains(r#"BR2_DL_DIR=".gaia"#) || config.contains("BR2_DL_DIR="));
    assert!(config.contains(r#"\"dl\""#), "{config}");
    assert!(config.contains(r#"\\dir"#), "{config}");
}

#[test]
fn buildroot_package_overrides_are_staged_as_external_tree() {
    let workspace_scratch = TempDir::new("gaia-buildroot-package-overrides-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-package-overrides-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-package-overrides-output");
    let output_dir = output_dir_scratch.join("output_dir");
    let user_external = workspace.join("user-external");
    let override_pkg = workspace.join("gaia/assets/buildroot/packages/foo");

    fs::create_dir_all(&override_pkg).expect("override package dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&user_external).expect("user external dir");
    fs::write(override_pkg.join("Config.in"), "config BR2_PACKAGE_FOO\n").expect("override config");
    fs::write(override_pkg.join("foo.mk"), "FOO_VERSION = gaia\n").expect("override mk");
    fs::write(
        buildroot_dir.join("Makefile"),
        "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf '%s' \"$$BR2_EXTERNAL\" > $(O)/br2_external_defconfig\n\t@touch $(O)/.config\ntarget-finalize: all\nall:\n\t@printf '%s' \"$$BR2_EXTERNAL\" > $(O)/br2_external_make\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-package-overrides");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            external_tree: Some(user_external.display().to_string()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let messages = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("buildroot run");

    let generated_external = output_dir.join("gaia-buildroot-external");
    assert!(generated_external.join("external.desc").is_file());
    assert_eq!(
        fs::read_to_string(generated_external.join("external.desc")).expect("external desc"),
        "name: GAIA_GENERATED\ndesc: Gaia generated Buildroot package overrides\n"
    );
    assert_eq!(
        fs::read_to_string(generated_external.join("package/foo/foo.mk")).expect("staged package"),
        "FOO_VERSION = gaia\n"
    );
    assert!(
        fs::read_to_string(generated_external.join("Config.in"))
            .expect("generated Config.in")
            .contains("BR2_EXTERNAL_GAIA_GENERATED_PATH/package/foo/Config.in")
    );
    assert!(
        fs::read_to_string(generated_external.join("external.mk"))
            .expect("generated external.mk")
            .contains("BR2_EXTERNAL_GAIA_GENERATED_PATH")
    );
    let expected_external = format!(
        "{}:{}",
        user_external.display(),
        generated_external.display()
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("br2_external_defconfig")).expect("defconfig external"),
        expected_external
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("br2_external_make")).expect("make external"),
        expected_external
    );
    assert!(messages.iter().any(|message| {
        message.contains("staged 1 generated Buildroot external package override(s)")
    }));
}

#[test]
fn buildroot_package_overrides_replace_existing_source_packages() {
    let workspace_scratch = TempDir::new("gaia-buildroot-package-replacements-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-package-replacements-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-package-replacements-output");
    let output_dir = output_dir_scratch.join("output_dir");
    let override_pkg = workspace.join("gaia/assets/buildroot/packages/foo");
    let source_pkg = buildroot_dir.join("package/foo");

    fs::create_dir_all(&override_pkg).expect("override package dir");
    fs::create_dir_all(&source_pkg).expect("source package dir");
    fs::create_dir_all(output_dir.join("target")).expect("stale target dir");
    fs::write(override_pkg.join("Config.in"), "config BR2_PACKAGE_FOO\n").expect("override config");
    fs::write(override_pkg.join("foo.mk"), "FOO_VERSION = gaia\n").expect("override mk");
    fs::write(
        source_pkg.join("Config.in"),
        "config BR2_PACKAGE_FOO_SOURCE\n",
    )
    .expect("source config");
    fs::write(
        buildroot_dir.join("Makefile"),
        "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf '%s' \"$${BR2_EXTERNAL-unset}\" > $(O)/br2_external_defconfig\n\t@touch $(O)/.config\nclean:\n\t@printf clean > $(O)/cleaned\ntarget-finalize: all\nall:\n\t@printf '%s' \"$${BR2_EXTERNAL-unset}\" > $(O)/br2_external_make\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-package-replacements");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            external_tree: None,
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    spec.policy.providers.buildroot.erofs_workers = gaia_spec::ErofsWorkersSpec::Off;
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let messages = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("buildroot run");

    assert_eq!(
        fs::read_to_string(source_pkg.join("Config.in")).expect("source package config"),
        "config BR2_PACKAGE_FOO\n"
    );
    assert_eq!(
        fs::read_to_string(source_pkg.join("foo.mk")).expect("source package mk"),
        "FOO_VERSION = gaia\n"
    );
    assert!(!output_dir.join("gaia-buildroot-external").exists());
    assert_eq!(
        fs::read_to_string(output_dir.join("br2_external_defconfig")).expect("defconfig external"),
        "unset"
    );
    // A tree recorded by no Gaia version: its config is unknown.
    assert_eq!(
        fs::read_to_string(output_dir.join("cleaned")).expect("clean marker"),
        "clean"
    );
    assert!(
        output_dir
            .join(".gaia-buildroot-package-replacements-state")
            .is_file()
    );
    assert!(
        fs::read_to_string(output_dir.join(".gaia-buildroot-package-overrides.digests"))
            .expect("override digests")
            .starts_with("foo ")
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("replaced 1 Buildroot source package definition"))
    );
}

#[test]
fn buildroot_config_changes_clean_existing_output_before_make() {
    let workspace_scratch = TempDir::new("gaia-buildroot-config-clean-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-config-clean-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-config-clean-output");
    let output_dir = output_dir_scratch.join("output_dir");

    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(output_dir.join("target")).expect("stale target dir");
    fs::write(output_dir.join(".config"), "BR2_PACKAGE_FOO=n\n").expect("stale config");
    fs::write(output_dir.join("target/stale"), "stale\n").expect("stale target file");
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@test -f $(O)/.config || printf 'BR2_PACKAGE_FOO=n\\n' > $(O)/.config\nolddefconfig:\n\t@true\nclean:\n\t@rm -rf $(O)/target\n\t@printf clean > $(O)/cleaned\ntarget-finalize: all\nall:\n\t@mkdir -p $(O)/target\n\t@printf built > $(O)/target/current\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-config-clean");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_overrides: vec![("BR2_PACKAGE_FOO".into(), "y".into())],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let messages = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("buildroot run");

    let config = fs::read_to_string(output_dir.join(".config")).expect("config");
    assert!(config.contains("BR2_PACKAGE_FOO=y"));
    assert_eq!(
        fs::read_to_string(output_dir.join("cleaned")).expect("clean marker"),
        "clean"
    );
    assert!(!output_dir.join("target/stale").exists());
    assert_eq!(
        fs::read_to_string(output_dir.join("target/current")).expect("rebuilt target"),
        "built"
    );
    assert!(output_dir.join(".gaia-buildroot-config-state").is_file());
    assert!(messages.iter().any(|message| {
        message.contains("cleaned Buildroot output: effective config changed (no snapshot")
    }));
}

#[test]
fn buildroot_retry_after_failed_post_image_does_not_clean() {
    let workspace_scratch = TempDir::new("gaia-buildroot-post-image-retry-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-post-image-retry-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-post-image-retry-output");
    let output_dir = output_dir_scratch.join("output_dir");

    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    // `all` builds packages, then fails in "post-image" while the marker
    // exists, the way a failing BR2_ROOTFS_POST_IMAGE_SCRIPT ends make.
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=n\\nBR2_ROOTFS_POST_IMAGE_SCRIPT=\"post-image.sh\"\\n' > $(O)/.config\nolddefconfig:\n\t@true\nclean:\n\t@printf clean >> $(O)/cleaned\ntarget-finalize: all\nall:\n\t@mkdir -p $(O)/target $(O)/build\n\t@printf built > $(O)/target/current\n\t@test ! -f $(O)/post-image-fails\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-post-image-retry");
    spec.workspace.root_dir = workspace.display().to_string();
    let image_with = |value: &str| ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_overrides: vec![("BR2_PACKAGE_FOO".into(), value.into())],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let run = |image: &ImageSpec| {
        run_buildroot(BuildrootRunRequest {
            spec: &spec,
            image,
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        })
    };

    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(output_dir.join("post-image-fails"), "").expect("failure marker");
    assert!(
        run(&image_with("y")).is_err(),
        "post-image failure fails make"
    );
    fs::remove_file(output_dir.join("post-image-fails")).expect("fix post-image");
    run(&image_with("y")).expect("retry");
    assert!(
        !output_dir.join("cleaned").exists(),
        "a retry with the same config must resume, not clean"
    );

    let messages = run(&image_with("n")).expect("changed config");
    assert_eq!(
        fs::read_to_string(output_dir.join("cleaned")).expect("clean marker"),
        "clean"
    );
    // This fake Buildroot has no `show-info`, so the change cannot be
    // narrowed to packages.
    assert!(messages.iter().any(|message| {
        message.starts_with("cleaned Buildroot output: BR2_PACKAGE_FOO, ")
            && message.ends_with("did not report its package graph")
    }));
}

#[test]
fn buildroot_option_change_rebuilds_only_the_package_and_its_dependents() {
    let workspace_scratch = TempDir::new("gaia-buildroot-targeted-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-targeted-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-targeted-output");
    let output_dir = output_dir_scratch.join("output_dir");

    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    // bar depends on foo; baz on nothing.
    let package = |name: &str, dependencies: &str, reverse: &str| {
        format!(
            "\"{name}\": {{\"type\": \"target\", \"name\": \"{name}\", \"virtual\": false, \
             \"version\": \"1\", \"stamp_dir\": \"build/{name}-1\", \
             \"dependencies\": [{dependencies}], \"reverse_dependencies\": [{reverse}]}}"
        )
    };
    fs::write(
        buildroot_dir.join("graph.json"),
        format!(
            "{{{}, {}, {}}}\n",
            package("foo", "", "\"bar\""),
            package("bar", "\"foo\"", ""),
            package("baz", "", "")
        ),
    )
    .expect("graph");
    // Each package builds once (until its build directory is removed),
    // installing usr/bin/<name> and recording it in its file list.
    fs::write(
        buildroot_dir.join("Makefile"),
        ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=y\\nBR2_PACKAGE_BAR=y\\nBR2_PACKAGE_BAZ=y\\n' > $(O)/.config\n\
         olddefconfig:\n\t@true\nclean:\n\t@printf clean >> $(O)/cleaned\nshow-info:\n\t@cat graph.json\n\
         target-finalize: all\nall:\n\t@for p in foo bar baz; do test -d $(O)/build/$$p-1 || { mkdir -p $(O)/build/$$p-1 $(O)/target/usr/bin; \
         echo $$p,./usr/bin/$$p > $(O)/build/$$p-1/.files-list.txt; echo $$p > $(O)/target/usr/bin/$$p; echo $$p >> $(O)/built; }; done\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-targeted");
    spec.workspace.root_dir = workspace.display().to_string();
    let image_with = |value: &str| ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            config_overrides: vec![("BR2_PACKAGE_FOO_OPTION".into(), value.into())],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let run = |image: &ImageSpec| {
        run_buildroot(BuildrootRunRequest {
            spec: &spec,
            image,
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        })
        .expect("buildroot run")
    };
    let built = || fs::read_to_string(output_dir.join("built")).unwrap_or_default();

    run(&image_with("n"));
    assert_eq!(built(), "foo\nbar\nbaz\n");
    assert!(output_dir.join(".gaia-buildroot-packages.json").is_file());
    // A file foo installed in an earlier build, which its rebuild will not.
    fs::write(output_dir.join("target/usr/bin/foo-old"), "old").expect("old file");
    fs::write(
        output_dir.join("build/foo-1/.files-list.txt"),
        "foo,./usr/bin/foo\nfoo,./usr/bin/foo-old\n",
    )
    .expect("file list");

    let messages = run(&image_with("y"));
    assert!(!output_dir.join("cleaned").exists(), "no full clean");
    assert_eq!(built(), "foo\nbar\nbaz\nfoo\nbar\n");
    assert!(!output_dir.join("target/usr/bin/foo-old").exists());
    assert!(output_dir.join("target/usr/bin/baz").is_file());
    assert!(
        messages
            .iter()
            .any(|message| message == "buildroot rebuild of 2 package(s): bar, foo"),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| { message == "foo changed: BR2_PACKAGE_FOO_OPTION unset -> y" }),
        "{messages:?}"
    );

    // Nothing changed: nothing is rebuilt and show-info is not run again.
    fs::write(buildroot_dir.join("graph.json"), "not json\n").expect("graph");
    run(&image_with("y"));
    assert_eq!(built(), "foo\nbar\nbaz\nfoo\nbar\n");
}

#[test]
fn buildroot_package_override_missing_config_in_fails_before_make() {
    let workspace_scratch =
        TempDir::new("gaia-buildroot-package-overrides-missing-config-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch =
        TempDir::new("gaia-buildroot-package-overrides-missing-config-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-package-overrides-missing-config-output");
    let output_dir = output_dir_scratch.join("output_dir");
    let override_pkg = workspace.join("gaia/assets/buildroot/packages/foo");

    fs::create_dir_all(&override_pkg).expect("override package dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(override_pkg.join("foo.mk"), "FOO_VERSION = gaia\n").expect("override mk");
    fs::write(
        buildroot_dir.join("Makefile"),
        "%_defconfig:\n\t@touch should-not-run\ntarget-finalize: all\nall:\n\t@touch should-not-run\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-package-overrides-missing-config");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
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
    .expect_err("missing Config.in should fail before make");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("missing required Config.in"));
    assert!(
        !buildroot_dir.join("should-not-run").exists(),
        "make should not run after package validation fails"
    );
}

#[test]
fn buildroot_package_override_detects_generated_external_name_conflict() {
    let workspace_scratch = TempDir::new("gaia-buildroot-package-overrides-conflict-workspace");
    let workspace = workspace_scratch.join("workspace");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-package-overrides-conflict-source");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-package-overrides-conflict-output");
    let output_dir = output_dir_scratch.join("output_dir");
    let user_external = workspace.join("user-external");
    let override_pkg = workspace.join("gaia/assets/buildroot/packages/foo");

    fs::create_dir_all(&override_pkg).expect("override package dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&user_external).expect("user external dir");
    fs::write(override_pkg.join("Config.in"), "config BR2_PACKAGE_FOO\n").expect("override config");
    fs::write(override_pkg.join("foo.mk"), "FOO_VERSION = gaia\n").expect("override mk");
    fs::write(
        user_external.join("external.desc"),
        "name: GAIA_GENERATED\ndesc: user tree\n",
    )
    .expect("user external desc");
    fs::write(
        buildroot_dir.join("Makefile"),
        "%_defconfig:\n\t@touch should-not-run\ntarget-finalize: all\nall:\n\t@touch should-not-run\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-package-overrides-conflict");
    spec.workspace.root_dir = workspace.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            external_tree: Some(user_external.display().to_string()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
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
    .expect_err("reserved generated external name should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("reserved generated external name"));
    assert!(
        !buildroot_dir.join("should-not-run").exists(),
        "make should not run after external name validation fails"
    );
}

#[test]
fn buildroot_state_digest_ignores_unrelated_build_tree_files() {
    let output_dir_scratch = TempDir::new("gaia-buildroot-digest-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join(".config"), "BR2_TARGET_ROOTFS_TAR=y\n").expect("config");
    fs::write(output_dir.join("images/rootfs.tar"), "rootfs").expect("image");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.tar".into(),
                format: BuildrootExpectedImageFormatSpec::Tar,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    let before = buildroot_state_digest(&image, &output_dir);

    fs::create_dir_all(output_dir.join("build/package")).expect("build dir");
    fs::write(output_dir.join("build/package/stamp"), "changed").expect("stamp");

    assert_eq!(before, buildroot_state_digest(&image, &output_dir));
}

#[test]
fn command_for_execution_mounts_parent_dir_for_output_files() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-docker-parent");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    let output_path = workspace_root.join("out/rootfs.tar");
    fs::create_dir_all(workspace_root.join("out")).expect("out dir");
    let execution = ImageExecutionContext {
        workspace_root: workspace_root.clone(),
        docker_image: Some("docker.io/library/debian:stable-slim".to_string()),
    };
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(&output_path)
        .arg("-C")
        .arg(&workspace_root)
        .arg(".");

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let mount = format!(
        "{}:{}",
        workspace_root.join("out").display(),
        workspace_root.join("out").display()
    );

    assert!(args.contains(&mount));
    assert!(!args.contains(&format!(
        "{}:{}",
        output_path.display(),
        output_path.display()
    )));
}

#[test]
fn buildroot_local_jobs_are_rendered_as_make_jobs() {
    let mut command = Command::new("make");
    append_make_jobs(&mut command, 2);

    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert!(args.contains(&"-j2".to_string()));
}

#[test]
fn buildroot_zero_local_jobs_uses_available_parallelism() {
    let mut command = Command::new("make");
    append_make_jobs(&mut command, 0);

    assert!(
        command
            .get_args()
            .any(|arg| arg.to_string_lossy().starts_with("-j"))
    );
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_runs_target_post_image_for_non_tar_outputs() {
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-post-image-dir");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-post-image-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
            buildroot_dir.join("Makefile"),
            "target-post-image:\n\t@mkdir -p $(O)/images\n\t@printf 'refreshed' > $(O)/images/rootfs.squashfs\n",
        )
        .expect("makefile");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.squashfs".into(),
                format: BuildrootExpectedImageFormatSpec::Squashfs,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };

    let messages = refresh_buildroot_images_after_feed_overlay(
        &ResolvedBuildSpec::new("buildroot-post-image-test"),
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("target-post-image refresh should succeed");

    assert!(
        output_dir.join("images/rootfs.squashfs").is_file(),
        "expected target-post-image to refresh non-tar image"
    );
    assert!(
        messages
            .iter()
            .all(|message| message.starts_with(gaia_process::STEP_TIME_PREFIX))
    );
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_disables_legacy_before_target_post_image() {
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-post-image-legacy-dir");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-post-image-legacy-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(output_dir.join(".config"), "BR2_LEGACY=y\n").expect("buildroot config");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@if grep -q 'BR2_LEGACY=y' $(O)/.config; then exit 12; fi\n\t@mkdir -p $(O)/images\n\t@printf raw > $(O)/images/sdcard.img\n",
    )
    .expect("makefile");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            config_overrides: vec![("BR2_LEGACY".into(), "n".into())],
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "sdcard.img".into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };

    refresh_buildroot_images_after_feed_overlay(
        &ResolvedBuildSpec::new("buildroot-post-image-legacy-test"),
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("target-post-image refresh should succeed");

    let config = fs::read_to_string(output_dir.join(".config")).expect("config");
    assert!(config.contains("# BR2_LEGACY is not set"));
    assert!(output_dir.join("images/sdcard.img").is_file());
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_absolutizes_external_tree() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-post-image-external-workspace");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    let buildroot_dir = workspace_root.join("buildroot");
    let output_dir = workspace_root.join("out");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(workspace_root.join("external")).expect("external tree");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@mkdir -p $(O)/images\n\t@printf '%s' \"$(BR2_EXTERNAL)\" > $(O)/images/br2_external\n\t@printf raw > $(O)/images/sdcard.img\n",
    )
    .expect("makefile");
    let mut spec = ResolvedBuildSpec::new("buildroot-post-image-external-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            external_tree: Some("external".into()),
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "sdcard.img".into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };

    refresh_buildroot_images_after_feed_overlay(
        &spec,
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("target-post-image refresh should succeed");

    let external = fs::read_to_string(output_dir.join("images/br2_external")).expect("external");
    assert_eq!(
        external,
        workspace_root.join("external").display().to_string()
    );
}

#[test]
fn refresh_buildroot_post_image_direct_runs_configured_script_with_buildroot_env() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-direct-post-image-workspace");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    let buildroot_dir = workspace_root.join("buildroot");
    let output_dir = workspace_root.join("out/buildroot-output");
    let script_path = workspace_root.join("post-image.sh");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::create_dir_all(output_dir.join("target")).expect("target dir");
    fs::create_dir_all(output_dir.join("build")).expect("build dir");
    fs::create_dir_all(output_dir.join("host")).expect("host dir");
    fs::write(
        output_dir.join(".config"),
        format!(
            "BR2_ROOTFS_POST_IMAGE_SCRIPT=\"{}\"\n",
            script_path.display()
        ),
    )
    .expect("buildroot config");
    write_executable(
        &script_path,
        "#!/bin/sh\nset -e\n[ -d \"$BINARIES_DIR\" ]\n[ -d \"$TARGET_DIR\" ]\nprintf raw > \"$BINARIES_DIR/sdcard.img\"\n",
    );

    let messages = refresh_buildroot_post_image_direct(
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("direct post-image should not error")
    .expect("direct post-image should run");

    assert_eq!(
        fs::read_to_string(output_dir.join("images/sdcard.img")).expect("raw image"),
        "raw"
    );
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("refreshed buildroot post-image outputs directly") })
    );
}

#[test]
fn materialize_defconfig_support_files_copies_sibling_assets_into_output_dir() {
    let assets_dir_scratch = TempDir::new("gaia-buildroot-defconfig-assets");
    let assets_dir = assets_dir_scratch.join("assets_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-defconfig-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&assets_dir).expect("assets dir");
    let defconfig_path = assets_dir.join("custom.defconfig");
    fs::write(
        &defconfig_path,
        "BR2_x86_64=y\nBR2_ROOTFS_POST_SCRIPT_ARGS=\"-c genimage.cfg\"\nBR2_PACKAGE_X=\"overlays/config.txt\"\n",
    )
    .expect("defconfig");
    fs::write(assets_dir.join("genimage.cfg"), "image test {}\n").expect("genimage");
    fs::create_dir_all(assets_dir.join("overlays")).expect("overlay dir");
    fs::write(assets_dir.join("overlays/config.txt"), "dtoverlay=test\n").expect("overlay");

    materialize_defconfig_support_files(&defconfig_path, &output_dir, &[])
        .expect("support files should copy");

    assert!(!output_dir.join("custom.defconfig").exists());
    assert_eq!(
        fs::read_to_string(output_dir.join("genimage.cfg")).expect("copied genimage"),
        "image test {}\n"
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("overlays/config.txt")).expect("copied overlay"),
        "dtoverlay=test\n"
    );
}

#[test]
fn materialize_defconfig_support_files_at_a_project_root_copies_only_referenced_files() {
    let scratch = TempDir::new("gaia-buildroot-defconfig-root");
    let root = scratch.join("workspace");
    let output_dir = root.join("output");
    fs::create_dir_all(root.join("src/deep")).expect("source dir");
    fs::create_dir_all(output_dir.join("target")).expect("target dir");
    fs::write(
        root.join("custom.defconfig"),
        "BR2_x86_64=y\nBR2_ROOTFS_POST_SCRIPT_ARGS=\"-c genimage.cfg\"\nBR2_HOSTNAME=\"target\"\n",
    )
    .expect("defconfig");
    fs::write(root.join("genimage.cfg"), "image test {}\n").expect("genimage");
    fs::write(root.join("src/deep/big.txt"), "not referenced").expect("source file");
    fs::write(root.join("Makefile"), "project makefile\n").expect("project makefile");
    // Buildroot's own entries in the output dir, which must survive.
    fs::write(output_dir.join("Makefile"), "buildroot make\n").expect("buildroot makefile");
    fs::write(output_dir.join("target/keep.txt"), "keep\n").expect("target file");
    // A settings file named like a Buildroot entry must not replace it.
    fs::write(root.join("target"), "a file named target\n").expect("clashing file");

    materialize_defconfig_support_files(&root.join("custom.defconfig"), &output_dir, &[])
        .expect("support files should copy");

    assert_eq!(
        fs::read_to_string(output_dir.join("genimage.cfg")).expect("referenced file"),
        "image test {}\n"
    );
    assert!(!output_dir.join("src").exists(), "unreferenced tree copied");
    assert!(!output_dir.join("custom.defconfig").exists());
    assert_eq!(
        fs::read_to_string(output_dir.join("Makefile")).expect("buildroot makefile"),
        "buildroot make\n"
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("target/keep.txt")).expect("target file"),
        "keep\n"
    );
    assert!(
        output_dir.join("target").is_dir(),
        "target replaced by a file"
    );
    let mut names = fs::read_dir(&output_dir)
        .expect("output dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["Makefile", "genimage.cfg", "target"]);
}

#[test]
fn materialize_defconfig_support_files_reads_fragment_references_and_skips_unchanged_copies() {
    let scratch = TempDir::new("gaia-buildroot-defconfig-fragment");
    let assets = scratch.join("assets");
    let output_dir = scratch.join("output");
    fs::create_dir_all(&assets).expect("assets dir");
    let defconfig = assets.join("custom.defconfig");
    fs::write(&defconfig, "BR2_x86_64=y\n").expect("defconfig");
    let fragment = assets.join("extra.cfg");
    fs::write(
        &fragment,
        "BR2_ROOTFS_POST_SCRIPT_ARGS=\"-c genimage.cfg\"\n",
    )
    .expect("fragment");
    fs::write(assets.join("genimage.cfg"), "image test {}\n").expect("genimage");

    materialize_defconfig_support_files(&defconfig, &output_dir, std::slice::from_ref(&fragment))
        .expect("support files should copy");
    assert_eq!(
        fs::read_to_string(output_dir.join("genimage.cfg")).expect("copied genimage"),
        "image test {}\n"
    );

    // Same size and mtime as the source: the copy is kept, not rewritten.
    let copy = output_dir.join("genimage.cfg");
    let modified = fs::metadata(assets.join("genimage.cfg"))
        .expect("source metadata")
        .modified()
        .expect("mtime");
    fs::write(&copy, "IMAGE TEST {}\n").expect("same-size edit");
    fs::File::options()
        .write(true)
        .open(&copy)
        .and_then(|file| file.set_modified(modified))
        .expect("restore mtime");
    materialize_defconfig_support_files(&defconfig, &output_dir, std::slice::from_ref(&fragment))
        .expect("second copy");
    assert_eq!(
        fs::read_to_string(&copy).expect("kept copy"),
        "IMAGE TEST {}\n"
    );
}

#[test]
fn apply_buildroot_config_fragments_merges_fragments_and_runs_olddefconfig() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-fragment-workspace");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-fragment-buildroot");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-fragment-output");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(workspace_root.join("assets")).expect("assets dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(
        output_dir.join(".config"),
        "BR2_TARGET_ROOTFS_TAR=y\nBR2_PACKAGE_BUSYBOX=n\n",
    )
    .expect("base config");
    fs::write(
        workspace_root.join("assets/fragment-a.cfg"),
        "BR2_PACKAGE_BUSYBOX=y\n",
    )
    .expect("fragment a");
    fs::write(
        workspace_root.join("assets/fragment-b.cfg"),
        "BR2_PACKAGE_DROPBEAR=y\n",
    )
    .expect("fragment b");
    fs::write(
        buildroot_dir.join("Makefile"),
        "olddefconfig:\n\t@printf 'ran\\n' > $(O)/olddefconfig.marker\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-fragment-merge");
    spec.workspace.root_dir = workspace_root.display().to_string();

    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let messages = apply_buildroot_config_fragments(
        &spec,
        &buildroot_dir,
        &output_dir,
        &[
            "assets/fragment-a.cfg".to_string(),
            "assets/fragment-b.cfg".to_string(),
        ],
        None,
        ImageCommandContext {
            execution: &execution,
            policy: &policy,
            log_sink: None,
            cancel_check: None,
        },
    )
    .expect("fragment merge should succeed");

    let merged = fs::read_to_string(output_dir.join(".config")).expect("merged config");
    assert!(
        messages
            .iter()
            .all(|message| message.starts_with(gaia_process::STEP_TIME_PREFIX))
    );
    assert!(merged.contains("BR2_TARGET_ROOTFS_TAR=y"));
    assert!(merged.contains("BR2_PACKAGE_BUSYBOX=y"));
    assert!(merged.contains("BR2_PACKAGE_DROPBEAR=y"));
    assert!(
        output_dir.join("olddefconfig.marker").is_file(),
        "expected olddefconfig to run after merging fragments"
    );
}

#[test]
fn apply_buildroot_config_settings_merges_overrides_and_runs_olddefconfig() {
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-override-buildroot");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-override-output");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(
        output_dir.join(".config"),
        "BR2_TARGET_ROOTFS_TAR=y\nBR2_PACKAGE_BUSYBOX=n\n",
    )
    .expect("base config");
    fs::write(
        buildroot_dir.join("Makefile"),
        "olddefconfig:\n\t@printf 'ran\\n' > $(O)/olddefconfig.marker\n",
    )
    .expect("makefile");

    let spec = ResolvedBuildSpec::new("override-test");
    let overrides = [
        ("BR2_PACKAGE_BUSYBOX".to_string(), "y".to_string()),
        (
            "BR2_TARGET_GENERIC_HOSTNAME".to_string(),
            "\"gaia\"".to_string(),
        ),
    ];
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let messages = apply_buildroot_config_settings(BuildrootSettingsRequest {
        spec: &spec,
        output_dir: &output_dir,
        overrides: &overrides,
        external_tree: None,
        buildroot_dir: &buildroot_dir,
        command: test_command_context(&execution, &policy),
        dry_run: false,
    })
    .expect("override merge should succeed");

    let merged = fs::read_to_string(output_dir.join(".config")).expect("merged config");
    assert!(
        messages
            .iter()
            .all(|message| message.starts_with(gaia_process::STEP_TIME_PREFIX))
    );
    assert!(merged.contains("BR2_PACKAGE_BUSYBOX=y"));
    assert!(!merged.contains("BR2_PACKAGE_BUSYBOX=n"));
    assert!(merged.contains("BR2_TARGET_GENERIC_HOSTNAME=\"gaia\""));
    assert!(
        output_dir.join("olddefconfig.marker").is_file(),
        "expected olddefconfig to run after applying overrides"
    );
}

#[test]
fn normalize_global_patch_dir_resolves_workspace_relative_entries() {
    let workspace_root_scratch = TempDir::new("gaia-buildroot-global-patch-workspace");
    let workspace_root = workspace_root_scratch.join("workspace_root");
    let patch_dir = workspace_root.join("gaia/assets/buildroot/patches");
    fs::create_dir_all(&patch_dir).expect("patch dir");
    let mut spec = ResolvedBuildSpec::new("global-patch-dir");
    spec.workspace.root_dir = workspace_root.display().to_string();

    let normalized = normalize_global_patch_dir_value(
        &spec,
        "\"board/raspberrypi/patches gaia/assets/buildroot/patches\"",
    );

    assert_eq!(
        normalized,
        format!("\"board/raspberrypi/patches {}\"", patch_dir.display())
    );
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_skips_tar_only_outputs() {
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-post-image-skip-dir");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-post-image-skip-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@false\n",
    )
    .expect("makefile");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.tar".into(),
                format: BuildrootExpectedImageFormatSpec::Tar,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };

    let messages = refresh_buildroot_images_after_feed_overlay(
        &ResolvedBuildSpec::new("buildroot-post-image-skip-test"),
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("tar-only outputs should skip refresh");

    assert!(messages.is_empty());
    assert!(!output_dir.exists());
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_reports_target_post_image_failure() {
    let buildroot_dir_scratch = TempDir::new("gaia-buildroot-post-image-fail-dir");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-buildroot-post-image-fail-out");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@echo broken post-image 1>&2\n\t@false\n",
    )
    .expect("makefile");
    let image = ImageSpec {
        signing: None,
        layers: None,
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.squashfs".into(),
                format: BuildrootExpectedImageFormatSpec::Squashfs,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            disk: None,
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };

    let error = refresh_buildroot_images_after_feed_overlay(
        &ResolvedBuildSpec::new("buildroot-post-image-failure-test"),
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect_err("broken target-post-image should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("broken post-image"));
}
