use super::*;

/// A fake Buildroot: `make` installs a base file, runs the post-build
/// scripts named on the command line (as Buildroot's target-finalize does),
/// then "packs" the target into a listing and counts the packs.
pub(super) const SINGLE_PASS_MAKEFILE: &str = "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_TARGET_ROOTFS_SQUASHFS=y\\n' > $(O)/.config\nall:\n\t@mkdir -p $(O)/target/usr/bin $(O)/images\n\t@printf base > $(O)/target/usr/bin/base\n\t@for s in $(BR2_ROOTFS_POST_BUILD_SCRIPT); do $$s $(O)/target; done\n\t@(cd $(O)/target && find . -mindepth 1 | LC_ALL=C sort) > $(O)/images/rootfs.squashfs\n\t@printf x >> $(O)/pack-count\nclean:\n\t@:\n";

/// Like [`SINGLE_PASS_MAKEFILE`] but ignores post-build scripts, so Gaia
/// must fall back to refreshing the images after make.
const NO_POST_BUILD_MAKEFILE: &str = "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_TARGET_ROOTFS_SQUASHFS=y\\n' > $(O)/.config\nall:\n\t@mkdir -p $(O)/target/usr/bin $(O)/images\n\t@printf base > $(O)/target/usr/bin/base\n\t@(cd $(O)/target && find . -mindepth 1 | LC_ALL=C sort) > $(O)/images/rootfs.squashfs\n\t@printf x >> $(O)/pack-count\ntarget-post-image:\n\t@(cd $(O)/target && find . -mindepth 1 | LC_ALL=C sort) > $(O)/images/rootfs.squashfs\n\t@printf r >> $(O)/pack-count\n";

/// A fake Buildroot for shared trees: a full `make` compiles (logs `all`),
/// installs a target with Buildroot's warning file, and generates a
/// fakeroot script that packs `build/buildroot-fs/squashfs/target` into
/// `images/rootfs.squashfs`. `target-finalize` only logs.
pub(super) const SHARED_MAKEFILE: &str = "%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_TARGET_ROOTFS_SQUASHFS=y\\nBR2_ROOTFS_POST_IMAGE_SCRIPT=\"board/post-image.sh\"\\n' > $(O)/.config\nall:\n\t@printf 'all\\n' >> $(O)/make-log\n\t@mkdir -p $(O)/target/usr/bin $(O)/images $(O)/host/bin $(O)/build/buildroot-fs/squashfs\n\t@printf base > $(O)/target/usr/bin/base\n\t@printf warn > $(O)/target/THIS_IS_NOT_YOUR_ROOT_FILESYSTEM\n\t@printf kernel > $(O)/images/Image\n\t@printf '#!/bin/sh\\n[ \"$$1\" = -- ] && shift\\nexec \"$$@\"\\n' > $(O)/host/bin/fakeroot\n\t@chmod +x $(O)/host/bin/fakeroot\n\t@printf '#!/bin/sh\\nset -e\\n(cd $(O)/build/buildroot-fs/squashfs/target && find . -mindepth 1 | LC_ALL=C sort) > $(O)/images/rootfs.squashfs\\n' > $(O)/build/buildroot-fs/squashfs/fakeroot\n\t@printf pristine > $(O)/images/rootfs.squashfs\ntarget-finalize:\n\t@printf 'finalize\\n' >> $(O)/make-log\n";

pub(super) fn squashfs_image(source: &str, stage_files: &[&str]) -> ImageSpec {
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        source: Some(SourceId::new(source)),
        defconfig: Some("fake_defconfig".into()),
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "rootfs.squashfs".into(),
            format: BuildrootExpectedImageFormatSpec::Squashfs,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.feed.stage_files = stage_files.iter().map(|id| (*id).into()).collect();
    image
}

pub(super) fn feed_spec(workspace_root: &Path, name: &str, build_dir: &str) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new(name);
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.workspace.build_dir = build_dir.into();
    spec.workspace.out_dir = workspace_root
        .join(format!("out-{name}"))
        .display()
        .to_string();
    let assets = workspace_root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    for (id, dest, mode) in [
        ("motd", "/etc/motd", Some(0o600)),
        ("old", "/usr/local/bin/old", None),
        ("a-only", "/opt/a-only", None),
        ("b-only", "/opt/b-only", None),
    ] {
        fs::write(assets.join(id), format!("{id}-content")).expect("asset");
        spec.stage.files.push(gaia_spec::StageFileSpec {
            id: id.into(),
            src: format!("assets/{id}"),
            dest: dest.into(),
            mode,
            origin: gaia_spec::StageContentOriginSpec::StaticAsset,
        });
    }
    spec
}

pub(super) fn write_buildroot_source(
    workspace_root: &Path,
    build_dir: &str,
    makefile: &str,
) -> PathBuf {
    let source_dir = workspace_root
        .join(build_dir)
        .join("sources")
        .join("buildroot-source");
    fs::create_dir_all(source_dir.join("board")).expect("source dir");
    fs::write(source_dir.join("Makefile"), makefile).expect("makefile");
    // Same Buildroot tree for every build, but per-build fields differ.
    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        format!("materialized_tree_digest=tree-1234\nbuild_version={build_dir}\n"),
    )
    .expect("source state");
    source_dir
}

pub(super) fn run_build(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    policy: &ImageExecutionPolicy,
) -> Result<ImageExecutionResult, ImageProviderError> {
    let collect_dir = PathBuf::from(&spec.workspace.out_dir).join("images");
    BuildrootImageProvider.execute_image(
        spec,
        image,
        &ImageOutputContract {
            collect_dir: Some(collect_dir.display().to_string()),
            archive_name: None,
            emit_report: false,
        },
        policy,
        None,
        None,
    )
}

fn listing(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("image listing")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn private_build_packs_feed_in_a_single_make() {
    let workspace_root = temp_path("gaia-buildroot-single-pass");
    write_buildroot_source(&workspace_root, "build", SINGLE_PASS_MAKEFILE);
    let spec = feed_spec(&workspace_root, "single", "build");
    let output_dir = workspace_root.join("build/image/buildroot-output");
    // An existing destination symlink must be replaced, never written
    // through.
    let outside = workspace_root.join("outside-motd");
    fs::write(&outside, "outside").expect("outside file");
    fs::create_dir_all(output_dir.join("target/etc")).expect("target etc");
    std::os::unix::fs::symlink(&outside, output_dir.join("target/etc/motd")).expect("symlink");

    let result = run_build(
        &spec,
        &squashfs_image("buildroot-source", &["motd", "old"]),
        &ImageExecutionPolicy::default(),
    )
    .expect("first build");

    let image = listing(&output_dir.join("images/rootfs.squashfs"));
    assert!(image.contains(&"./etc/motd".to_string()), "{image:?}");
    assert!(
        image.contains(&"./usr/local/bin/old".to_string()),
        "{image:?}"
    );
    assert!(image.contains(&"./usr/bin/base".to_string()), "{image:?}");
    assert_eq!(
        fs::read_to_string(output_dir.join("pack-count")).expect("packs"),
        "x"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.contains("post-build script")),
        "{:?}",
        result.messages
    );
    let motd = output_dir.join("target/etc/motd");
    assert!(!fs::symlink_metadata(&motd).expect("motd").is_symlink());
    assert_eq!(fs::read_to_string(&motd).expect("motd"), "motd-content");
    assert_eq!(
        fs::metadata(&motd).expect("motd").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_to_string(&outside).expect("outside"), "outside");
    assert!(!staged_image_feed_dir(&output_dir).exists());

    run_build(
        &spec,
        &squashfs_image("buildroot-source", &["motd"]),
        &ImageExecutionPolicy::default(),
    )
    .expect("second build");
    let image = listing(&output_dir.join("images/rootfs.squashfs"));
    assert!(image.contains(&"./etc/motd".to_string()), "{image:?}");
    assert!(
        !image.iter().any(|entry| entry.contains("old")),
        "stale feed file survived: {image:?}"
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("pack-count")).expect("packs"),
        "xx"
    );
}

#[test]
fn private_build_falls_back_when_post_build_script_does_not_run() {
    let workspace_root = temp_path("gaia-buildroot-single-pass-fallback");
    write_buildroot_source(&workspace_root, "build", NO_POST_BUILD_MAKEFILE);
    let spec = feed_spec(&workspace_root, "fallback", "build");
    let output_dir = workspace_root.join("build/image/buildroot-output");

    let result = run_build(
        &spec,
        &squashfs_image("buildroot-source", &["motd"]),
        &ImageExecutionPolicy::default(),
    )
    .expect("build");

    let image = listing(&output_dir.join("images/rootfs.squashfs"));
    assert!(image.contains(&"./etc/motd".to_string()), "{image:?}");
    assert_eq!(
        fs::read_to_string(output_dir.join("pack-count")).expect("packs"),
        "xr"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.contains("did not run the image feed")),
        "{:?}",
        result.messages
    );
}

#[test]
fn feed_post_build_script_quotes_paths_and_restores_modes() {
    let rootfs = temp_path("gaia-buildroot-feed-script");
    fs::create_dir_all(rootfs.join("opt/it's here")).expect("dirs");
    fs::write(rootfs.join("opt/it's here/file"), "x").expect("file");
    let script = feed_post_build_script(
        &rootfs,
        &[("/opt/it's here/file".into(), 0o640)],
        Path::new("/tmp/marker"),
        "nonce",
    )
    .expect("script");
    assert!(script.contains("mkdir -p \"$T\"'/opt'"), "{script}");
    assert!(script.contains("'/opt/it'\\''s here/file'"), "{script}");
    assert!(script.contains("chmod 640 "), "{script}");
    assert!(script.contains("rm -rf --"), "{script}");
}

#[test]
fn shared_output_key_ignores_build_identity_but_tracks_buildroot_inputs() {
    let workspace_root = temp_path("gaia-buildroot-shared-key");
    let source_a = write_buildroot_source(&workspace_root, "build-a", SHARED_MAKEFILE);
    let source_b = write_buildroot_source(&workspace_root, "build-b", SHARED_MAKEFILE);
    let spec_a = feed_spec(&workspace_root, "helios-base-os", "build-a");
    let spec_b = feed_spec(&workspace_root, "helios-full", "build-b");
    let policy = ImageExecutionPolicy::default();
    let execution = test_execution();
    let image = squashfs_image("buildroot-source", &["a-only"]);
    let other_feed = squashfs_image("buildroot-source", &["b-only"]);

    let key_a = shared_buildroot_output(&spec_a, &image, &source_a, &policy, &execution)
        .expect("key a")
        .key;
    let key_b = shared_buildroot_output(&spec_b, &other_feed, &source_b, &policy, &execution)
        .expect("key b")
        .key;
    assert_eq!(key_a, key_b, "feed and build identity must not split trees");

    let mut changed = image.clone();
    if let ImageDefinition::Buildroot(buildroot) = &mut changed.definition {
        buildroot
            .config_overrides
            .push(("BR2_PACKAGE_FOO".into(), "y".into()));
    }
    let key_changed = shared_buildroot_output(&spec_a, &changed, &source_a, &policy, &execution)
        .expect("key changed")
        .key;
    assert_ne!(key_a, key_changed);
}

#[test]
fn shared_output_builds_once_and_keeps_feeds_private() {
    let workspace_root = temp_path("gaia-buildroot-shared-build");
    for build_dir in ["build-a", "build-b"] {
        let source = write_buildroot_source(&workspace_root, build_dir, SHARED_MAKEFILE);
        write_executable(
            &source.join("board/post-image.sh"),
            "#!/bin/sh\nset -e\n[ \"$1\" = \"$BINARIES_DIR\" ]\nprintf '%s' \"$TARGET_DIR\" > \"$BINARIES_DIR/post-image-target\"\n",
        );
    }
    let spec_a = feed_spec(&workspace_root, "helios-base-os", "build-a");
    let spec_b = feed_spec(&workspace_root, "helios-full", "build-b");
    let policy = ImageExecutionPolicy {
        shared_output: true,
        ..ImageExecutionPolicy::default()
    };

    let result_a = run_build(
        &spec_a,
        &squashfs_image("buildroot-source", &["a-only"]),
        &policy,
    )
    .expect("build a");
    let result_b = run_build(
        &spec_b,
        &squashfs_image("buildroot-source", &["b-only"]),
        &policy,
    )
    .expect("build b");

    let state = |result: &ImageExecutionResult, key: &str| {
        result
            .state_details
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .expect("state detail")
    };
    let shared_dir = PathBuf::from(state(&result_a, "buildroot_shared_output_dir"));
    assert_eq!(
        state(&result_a, "buildroot_shared_key"),
        state(&result_b, "buildroot_shared_key")
    );
    assert!(shared_dir.starts_with(fs::canonicalize(&workspace_root).expect("root")));
    assert_eq!(
        fs::read_to_string(shared_dir.join("make-log")).expect("make log"),
        "all\nfinalize\n",
        "the second build must reuse the shared tree without a full make"
    );

    let output_a = workspace_root.join("build-a/image/buildroot-output");
    let output_b = workspace_root.join("build-b/image/buildroot-output");
    let image_a = listing(&output_a.join("images/rootfs.squashfs"));
    let image_b = listing(&output_b.join("images/rootfs.squashfs"));
    assert!(image_a.contains(&"./opt/a-only".to_string()), "{image_a:?}");
    assert!(!image_a.iter().any(|entry| entry.contains("b-only")));
    assert!(image_b.contains(&"./opt/b-only".to_string()), "{image_b:?}");
    assert!(!image_b.iter().any(|entry| entry.contains("a-only")));
    for image in [&image_a, &image_b] {
        assert!(image.contains(&"./usr/bin/base".to_string()));
        assert!(!image.iter().any(|entry| entry.contains("THIS_IS_NOT")));
    }
    assert!(!shared_dir.join("target/opt").exists(), "feed leaked");
    assert_eq!(
        fs::read_to_string(shared_dir.join("images/rootfs.squashfs")).expect("shared image"),
        "pristine"
    );
    assert_eq!(
        fs::read_to_string(output_b.join("images/Image")).expect("kernel"),
        "kernel"
    );
    assert_eq!(
        fs::read_link(output_b.join("host")).expect("host link"),
        shared_dir.join("host")
    );
    assert_eq!(
        fs::read_to_string(output_b.join("images/post-image-target")).expect("post-image"),
        output_b.join("target").display().to_string()
    );
    assert!(!output_b.join(".gaia-pack").exists());
    assert!(
        workspace_root
            .join("out-helios-full/images/rootfs.squashfs")
            .is_file()
    );

    // Leaving shared mode removes the view; the tree survives while another
    // build still uses it and is removed with its last user.
    leave_shared_view(&output_a).expect("leave a");
    assert!(!output_a.exists());
    assert!(shared_dir.is_dir());
    let messages = leave_shared_view(&output_b).expect("leave b");
    assert!(!shared_dir.exists(), "{messages:?}");
}

#[test]
fn shared_output_lock_waits_and_honors_cancellation() {
    let workspace_root = temp_path("gaia-buildroot-shared-lock");
    let source = write_buildroot_source(&workspace_root, "build", SHARED_MAKEFILE);
    let spec = feed_spec(&workspace_root, "locked", "build");
    let shared = shared_buildroot_output(
        &spec,
        &squashfs_image("buildroot-source", &[]),
        &source,
        &ImageExecutionPolicy::default(),
        &test_execution(),
    )
    .expect("shared");
    let held = lock_shared_output(&shared, None, None).expect("first lock");
    let cancel: ProcessCancelCheck = std::sync::Arc::new(|| true);
    let waiting = std::thread::spawn({
        let shared = shared.clone();
        move || lock_shared_output(&shared, Some(&cancel), None).map(|_| ())
    });
    let error = waiting
        .join()
        .expect("thread")
        .expect_err("second lock must wait");
    assert_eq!(error.kind, ImageProviderErrorKind::Cancelled);
    drop(held);
    lock_shared_output(&shared, None, None).expect("lock after release");
}

#[test]
fn shared_output_rejects_initramfs() {
    let error =
        shared_rootfs_types("BR2_TARGET_ROOTFS_SQUASHFS=y\nBR2_TARGET_ROOTFS_INITRAMFS=y\n")
            .expect_err("initramfs is unsupported");
    assert!(error.message.contains("INITRAMFS"));
    assert_eq!(
        shared_rootfs_types("BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_TAR=y\n").expect("types"),
        vec!["ext2", "tar"]
    );
}

#[test]
fn squashfs_compression_change_does_not_clean_the_output_tree() {
    let workspace_root = temp_path("gaia-buildroot-squashfs-comp");
    let output_dir = workspace_root.join("output");
    fs::create_dir_all(&output_dir).expect("output");
    let xz = "BR2_TARGET_ROOTFS_SQUASHFS=y\nBR2_TARGET_ROOTFS_SQUASHFS4_XZ=y\n";
    let zstd = "BR2_TARGET_ROOTFS_SQUASHFS=y\nBR2_TARGET_ROOTFS_SQUASHFS4_ZSTD=y\n";
    fs::write(output_dir.join(".config"), xz).expect("config");
    let digest_xz = buildroot_config_digest(&output_dir).expect("digest");
    fs::write(output_dir.join(".config"), zstd).expect("config");
    assert_eq!(
        buildroot_config_digest(&output_dir),
        Some(digest_xz.clone())
    );
    // Disabling squashfs itself is still a real config change.
    fs::write(
        output_dir.join(".config"),
        "BR2_TARGET_ROOTFS_SQUASHFS4_XZ=y\n",
    )
    .expect("config");
    assert_ne!(buildroot_config_digest(&output_dir), Some(digest_xz));
}
