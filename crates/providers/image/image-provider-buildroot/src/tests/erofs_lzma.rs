use super::*;

const EROFS_LZMA_CONFIG: &str = "BR2_TARGET_ROOTFS_TAR=y\n\
BR2_TARGET_ROOTFS_EROFS=y\n\
# BR2_TARGET_ROOTFS_EROFS_NONE is not set\n\
# BR2_TARGET_ROOTFS_EROFS_LZ4HC is not set\n\
BR2_TARGET_ROOTFS_EROFS_LZMA=y\n\
# BR2_TARGET_ROOTFS_EROFS_CUSTOM_COMPRESSION is not set\n\
BR2_TARGET_ROOTFS_EROFS_LZMA_LEVEL=9\n\
BR2_TARGET_ROOTFS_EROFS_DEDUPE=y\n";

#[test]
fn erofs_lzma_choice_becomes_custom_lzma_with_the_same_level() {
    let rewritten = erofs_lzma_as_custom_compression(EROFS_LZMA_CONFIG).expect("rewrite");

    assert!(
        rewritten.contains("# BR2_TARGET_ROOTFS_EROFS_LZMA is not set\n"),
        "{rewritten}"
    );
    assert!(
        !rewritten
            .lines()
            .any(|line| line == "BR2_TARGET_ROOTFS_EROFS_LZMA=y"),
        "{rewritten}"
    );
    assert!(
        rewritten
            .lines()
            .any(|line| line == "BR2_TARGET_ROOTFS_EROFS_CUSTOM_COMPRESSION=y"),
        "{rewritten}"
    );
    assert!(
        rewritten
            .lines()
            .any(|line| line == "BR2_TARGET_ROOTFS_EROFS_COMPRESSION_ALGORITHMS=\"lzma,9\""),
        "{rewritten}"
    );
    assert!(
        !rewritten.contains("# BR2_TARGET_ROOTFS_EROFS_CUSTOM_COMPRESSION is not set"),
        "{rewritten}"
    );
    assert!(rewritten.contains("BR2_TARGET_ROOTFS_EROFS_DEDUPE=y"));
    assert!(rewritten.contains("BR2_TARGET_ROOTFS_TAR=y"));
    // Rewriting is one-way: the result no longer has the collision.
    assert_eq!(erofs_lzma_as_custom_compression(&rewritten), None);
}

#[test]
fn erofs_lzma_rewrite_defaults_the_level_to_six() {
    let config = "BR2_TARGET_ROOTFS_EROFS=y\nBR2_TARGET_ROOTFS_EROFS_LZMA=y\n";

    let rewritten = erofs_lzma_as_custom_compression(config).expect("rewrite");

    assert!(
        rewritten
            .lines()
            .any(|line| line == "BR2_TARGET_ROOTFS_EROFS_COMPRESSION_ALGORITHMS=\"lzma,6\""),
        "{rewritten}"
    );
}

#[test]
fn configs_without_the_erofs_lzma_choice_are_not_rewritten() {
    // EROFS with lz4hc, EROFS disabled, squashfs and ext2 (which keep
    // Buildroot's generic compression on purpose) all pass through.
    let lz4hc = "BR2_TARGET_ROOTFS_EROFS=y\nBR2_TARGET_ROOTFS_EROFS_LZ4HC=y\n\
BR2_TARGET_ROOTFS_EROFS_LZ4HC_LEVEL=9\n";
    let no_erofs = "BR2_TARGET_ROOTFS_EROFS_LZMA=y\n";
    let squashfs = "BR2_TARGET_ROOTFS_SQUASHFS=y\nBR2_TARGET_ROOTFS_SQUASHFS4_LZMA=y\n";
    let ext2 = "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_LZMA=y\n";

    for config in [lz4hc, no_erofs, squashfs, ext2] {
        assert_eq!(erofs_lzma_as_custom_compression(config), None, "{config}");
    }
}

#[test]
fn settings_step_rewrites_erofs_lzma_and_reports_it() {
    let buildroot_dir_scratch = TempDir::new("gaia-erofs-lzma-buildroot");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-erofs-lzma-output");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(output_dir.join(".config"), EROFS_LZMA_CONFIG).expect("config");
    fs::write(buildroot_dir.join("Makefile"), "olddefconfig:\n\t@true\n").expect("makefile");
    let spec = ResolvedBuildSpec::new("erofs-lzma-test");
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let messages = apply_buildroot_config_settings(BuildrootSettingsRequest {
        spec: &spec,
        output_dir: &output_dir,
        overrides: &[],
        external_tree: None,
        buildroot_dir: &buildroot_dir,
        command: test_command_context(&execution, &policy),
        dry_run: false,
    })
    .expect("settings");

    let config = fs::read_to_string(output_dir.join(".config")).expect("config");
    assert!(
        messages.iter().any(|message| message == EROFS_LZMA_NOTE),
        "{messages:?}"
    );
    assert!(
        config.contains("BR2_TARGET_ROOTFS_EROFS_COMPRESSION_ALGORITHMS=\"lzma,9\""),
        "{config}"
    );
}

#[test]
fn settings_step_leaves_other_configs_alone() {
    let buildroot_dir_scratch = TempDir::new("gaia-erofs-untouched-buildroot");
    let buildroot_dir = buildroot_dir_scratch.join("buildroot_dir");
    let output_dir_scratch = TempDir::new("gaia-erofs-untouched-output");
    let output_dir = output_dir_scratch.join("output_dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    let original = "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_LZMA=y\n";
    fs::write(output_dir.join(".config"), original).expect("config");
    fs::write(buildroot_dir.join("Makefile"), "olddefconfig:\n\t@true\n").expect("makefile");
    let spec = ResolvedBuildSpec::new("erofs-untouched-test");
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let messages = apply_buildroot_config_settings(BuildrootSettingsRequest {
        spec: &spec,
        output_dir: &output_dir,
        overrides: &[],
        external_tree: None,
        buildroot_dir: &buildroot_dir,
        command: test_command_context(&execution, &policy),
        dry_run: false,
    })
    .expect("settings");

    assert!(!messages.iter().any(|message| message == EROFS_LZMA_NOTE));
    assert_eq!(
        fs::read_to_string(output_dir.join(".config")).expect("config"),
        original
    );
}
