use super::*;

/// A fake Buildroot: the config steps record their calls, `source` writes an
/// archive into the download cache (as a real `make source` does), and `all`
/// records that a build was asked for.
const FAKE_PREFETCH: &str = ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@echo defconfig >> $(O)/calls\n\
\t@printf '# BR2_PACKAGE_BUSYBOX is not set\\nBR2_TARGET_ROOTFS_TAR=y\\n' > $(O)/.config\n\
olddefconfig:\n\t@echo olddefconfig >> $(O)/calls\n\t@cat $(O)/.config >> $(O)/olddefconfig-input\n\t@echo --- >> $(O)/olddefconfig-input\n\
source:\n\t@echo source >> $(O)/calls\n\t@mkdir -p $(BR2_DL_DIR)/busybox\n\t@echo archive > $(BR2_DL_DIR)/busybox/busybox-1.0.tar.gz\n\
clean:\n\t@true\n\
target-finalize:\n\t@true\n\
all: target-finalize\n\t@echo all >> $(O)/calls\n";

/// A fake Buildroot whose download of busybox fails, as an offline make does
/// when the cache lacks the archive.
const FAKE_MISSING_DOWNLOAD: &str = ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\
\t@printf '# BR2_PACKAGE_BUSYBOX is not set\\nBR2_TARGET_ROOTFS_TAR=y\\n' > $(O)/.config\n\
olddefconfig:\n\t@true\n\
clean:\n\t@true\n\
target-finalize:\n\t@echo '>>> busybox 1.0 Downloading'\n\t@exit 1\n\
all: target-finalize\n\t@true\n";

fn image() -> ImageSpec {
    ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("test_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        signing: None,
        layers: None,
        checks: gaia_spec::ImageChecksSpec::default(),
        hardware: Vec::new(),
    }
}

/// A spec whose workspace is `scratch`, with the policy's download cache in
/// `scratch/dl`.
fn spec_in(scratch: &TempDir, offline: bool) -> ResolvedBuildSpec {
    let workspace = scratch.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let mut spec = ResolvedBuildSpec::new("offline-prefetch");
    spec.workspace.root_dir = workspace.display().to_string();
    spec.policy.execution.offline = offline;
    spec
}

fn policy_with_cache(scratch: &TempDir) -> ImageExecutionPolicy {
    ImageExecutionPolicy {
        download_dir: Some(scratch.join("dl").display().to_string()),
        ..ImageExecutionPolicy::default()
    }
}

fn buildroot_tree(scratch: &TempDir, makefile: &str) -> PathBuf {
    let dir = scratch.join("buildroot_dir");
    fs::create_dir_all(&dir).expect("buildroot dir");
    fs::write(dir.join("Makefile"), makefile).expect("makefile");
    dir
}

#[test]
fn prefetch_configures_then_downloads_every_source_and_builds_nothing() {
    let scratch = TempDir::new("gaia-prefetch-buildroot");
    let buildroot_dir = buildroot_tree(&scratch, FAKE_PREFETCH);
    let output_dir = scratch.join("output_dir");
    let spec = spec_in(&scratch, false);
    let execution = test_execution();
    let policy = policy_with_cache(&scratch);
    let image = image();

    let messages = prefetch_buildroot_sources(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("prefetch");

    assert!(
        scratch.join("dl/busybox/busybox-1.0.tar.gz").is_file(),
        "make source downloads into BR2_DL_DIR"
    );
    let calls = fs::read_to_string(output_dir.join("calls")).unwrap_or_default();
    assert!(calls.contains("defconfig"), "{calls}");
    assert!(calls.contains("source"), "{calls}");
    assert!(!calls.contains("all"), "a prefetch never builds: {calls}");
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("prefetch: 1 new source file(s)")),
        "{messages:?}"
    );
}

#[test]
fn offline_points_the_download_at_the_cache_and_keeps_the_output_fingerprint() {
    let scratch = TempDir::new("gaia-offline-config");
    let buildroot_dir = buildroot_tree(&scratch, FAKE_PREFETCH);
    let execution = test_execution();
    let policy = policy_with_cache(&scratch);
    let image = image();

    let run = |offline: bool, name: &str| {
        let spec = spec_in(&scratch, offline);
        let output_dir = scratch.join(name);
        run_buildroot_with(
            BuildrootRunRequest {
                spec: &spec,
                image: &image,
                buildroot_dir: &buildroot_dir,
                output_dir: &output_dir,
                command: test_command_context(&execution, &policy),
            },
            BuildrootMakeOptions::default(),
        )
        .expect("buildroot run");
        let config = fs::read_to_string(output_dir.join(".config")).expect("config");
        let digest = buildroot_config_digest(&output_dir).expect("digest");
        (config, digest)
    };

    let (online_config, online_digest) = run(false, "online");
    let (offline_config, offline_digest) = run(true, "offline");

    assert!(
        !online_config.contains("BR2_PRIMARY_SITE"),
        "{online_config}"
    );
    assert!(
        offline_config.contains("BR2_PRIMARY_SITE=\"file://"),
        "{offline_config}"
    );
    assert!(
        offline_config.contains("BR2_PRIMARY_SITE_ONLY=y"),
        "{offline_config}"
    );
    assert!(
        offline_config.contains("BR2_BACKUP_SITE=\"\""),
        "{offline_config}"
    );
    assert_eq!(
        online_digest, offline_digest,
        "offline download settings must not change the output fingerprint"
    );
}

#[test]
fn offline_download_failure_names_the_package_and_suggests_prefetch() {
    let scratch = TempDir::new("gaia-offline-missing");
    let buildroot_dir = buildroot_tree(&scratch, FAKE_MISSING_DOWNLOAD);
    let output_dir = scratch.join("output_dir");
    let spec = spec_in(&scratch, true);
    let execution = test_execution();
    let policy = policy_with_cache(&scratch);
    let image = image();

    let error = run_buildroot_with(
        BuildrootRunRequest {
            spec: &spec,
            image: &image,
            buildroot_dir: &buildroot_dir,
            output_dir: &output_dir,
            command: test_command_context(&execution, &policy),
        },
        BuildrootMakeOptions::default(),
    )
    .expect_err("a missing download must fail offline");

    assert!(
        error
            .message
            .contains("offline: 'busybox 1.0' is not in the download cache"),
        "{}",
        error.message
    );
    assert!(error.message.contains("gaia prefetch"), "{}", error.message);
}
