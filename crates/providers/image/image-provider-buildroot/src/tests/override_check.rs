use super::single_pass_shared::{
    SHARED_MAKEFILE, SINGLE_PASS_MAKEFILE, feed_spec, run_build, squashfs_image,
    write_buildroot_source,
};
use super::*;
use gaia_spec::BuildrootOverrideCheckSpec;

/// A fake `olddefconfig` that behaves like Kconfig with unmet dependencies:
/// OpenJDK disappears and Mesa is reset to "is not set".
const DROPPING_OLDDEFCONFIG: &str = "olddefconfig:\n\t@sed -i -e '/BR2_PACKAGE_OPENJDK/d' -e 's/^BR2_PACKAGE_MESA3D=y/# BR2_PACKAGE_MESA3D is not set/' $(O)/.config\n";

fn overrides() -> Vec<(String, String)> {
    vec![
        ("BR2_PACKAGE_OPENJDK".into(), "y".into()),
        ("BR2_PACKAGE_MESA3D".into(), "y".into()),
        ("BR2_PACKAGE_BUSYBOX".into(), "y".into()),
    ]
}

fn image_with_overrides(overrides: Vec<(String, String)>) -> ImageSpec {
    let mut image = squashfs_image("buildroot-source", &[]);
    if let ImageDefinition::Buildroot(buildroot) = &mut image.definition {
        buildroot.config_overrides = overrides;
    }
    image
}

fn policy(check: BuildrootOverrideCheckSpec, shared_output: bool) -> ImageExecutionPolicy {
    ImageExecutionPolicy {
        override_check: check,
        shared_output,
        ..ImageExecutionPolicy::default()
    }
}

#[test]
fn mismatches_cover_dropped_changed_and_equivalent_values() {
    let config = "\
BR2_PACKAGE_MESA3D=y
# BR2_PACKAGE_XORG7 is not set
BR2_TARGET_GENERIC_HOSTNAME=\"photon\"
BR2_KERNEL_HEADERS_VERSION=\"6.6\"
BR2_CCACHE_DIR=\"/cache\"
BR2_TOOLCHAIN_EXTERNAL_GCC_ARCH_OFFSET=0x1F
BR2_JLEVEL=8
";
    let requested = [
        ("BR2_PACKAGE_OPENJDK", "y"),
        ("BR2_PACKAGE_XORG7", "y"),
        ("BR2_TARGET_GENERIC_HOSTNAME", "photon"),
        ("BR2_KERNEL_HEADERS_VERSION", "\"6.1\""),
        ("BR2_PACKAGE_MESA3D", "n"),
        ("BR2_PACKAGE_MESA3D", "y"),
        ("BR2_PACKAGE_UNUSED", "n"),
        ("BR2_EMPTY_STRING", "\"\""),
        ("BR2_CCACHE_DIR", "\"/elsewhere\""),
        ("BR2_TOOLCHAIN_EXTERNAL_GCC_ARCH_OFFSET", "0x1f"),
        ("BR2_JLEVEL", "08"),
    ]
    .map(|(key, value)| (key.to_string(), value.to_string()));

    let mismatches = find_override_mismatches(config, &requested);

    assert_eq!(
        mismatches,
        vec![
            OverrideMismatch {
                key: "BR2_PACKAGE_OPENJDK".into(),
                requested: "y".into(),
                actual: None,
            },
            OverrideMismatch {
                key: "BR2_PACKAGE_XORG7".into(),
                requested: "y".into(),
                actual: Some("n".into()),
            },
            OverrideMismatch {
                key: "BR2_KERNEL_HEADERS_VERSION".into(),
                requested: "\"6.1\"".into(),
                actual: Some("\"6.6\"".into()),
            },
        ]
    );
}

#[test]
fn requested_n_fails_when_the_symbol_stays_enabled() {
    let mismatches = find_override_mismatches(
        "BR2_PACKAGE_DROPBEAR=y\n",
        &[("BR2_PACKAGE_DROPBEAR".into(), "n".into())],
    );
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].actual.as_deref(), Some("y"));
}

#[test]
fn error_policy_fails_before_make_and_lists_every_dropped_symbol() {
    let workspace = temp_path("gaia-override-check-error");
    let buildroot_dir = write_buildroot_source(
        &workspace,
        "build",
        &format!("{SINGLE_PASS_MAKEFILE}{DROPPING_OLDDEFCONFIG}"),
    );
    let spec = feed_spec(&workspace, "override-error", "build");
    let output_dir = workspace.join("output");
    let execution = test_execution();
    let policy = policy(BuildrootOverrideCheckSpec::Error, false);

    let error = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image_with_overrides(overrides()),
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect_err("dropped overrides must fail");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("2 config_overrides entries"));
    assert!(
        error.message.contains(
            "BR2_PACKAGE_OPENJDK dropped: requested BR2_PACKAGE_OPENJDK=y, final missing"
        )
    );
    assert!(
        error.message.contains(
            "BR2_PACKAGE_MESA3D dropped: requested BR2_PACKAGE_MESA3D=y, final is not set"
        )
    );
    assert!(!error.message.contains("BR2_PACKAGE_BUSYBOX"));
    assert!(error.message.contains(
        "usually an unmet `depends on`; check menuconfig for BR2_PACKAGE_OPENJDK, BR2_PACKAGE_MESA3D"
    ));
    assert!(
        !output_dir.join("pack-count").exists(),
        "the long make must not run"
    );
}

#[test]
fn warn_policy_builds_and_reports_warnings_on_the_result() {
    let workspace = temp_path("gaia-override-check-warn");
    write_buildroot_source(
        &workspace,
        "build",
        &format!("{SINGLE_PASS_MAKEFILE}{DROPPING_OLDDEFCONFIG}"),
    );
    let spec = feed_spec(&workspace, "override-warn", "build");

    let result = run_build(
        &spec,
        &image_with_overrides(overrides()),
        &policy(BuildrootOverrideCheckSpec::Warn, false),
    )
    .expect("warn policy builds");

    assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
    assert!(result.warnings[0].starts_with("buildroot config_overrides: BR2_PACKAGE_OPENJDK"));
    assert!(result.warnings[1].contains("check menuconfig for BR2_PACKAGE_MESA3D"));
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.starts_with(OVERRIDE_CHECK_WARNING_PREFIX))
    );
}

#[test]
fn off_policy_and_honored_overrides_produce_no_warnings() {
    let workspace = temp_path("gaia-override-check-off");
    write_buildroot_source(
        &workspace,
        "build",
        &format!("{SINGLE_PASS_MAKEFILE}{DROPPING_OLDDEFCONFIG}"),
    );
    let spec = feed_spec(&workspace, "override-off", "build");

    let off = run_build(
        &spec,
        &image_with_overrides(overrides()),
        &policy(BuildrootOverrideCheckSpec::Off, false),
    )
    .expect("off policy builds");
    assert!(off.warnings.is_empty());

    let honored = run_build(
        &spec,
        &image_with_overrides(vec![("BR2_PACKAGE_BUSYBOX".into(), "y".into())]),
        &policy(BuildrootOverrideCheckSpec::Error, false),
    )
    .expect("honored overrides build");
    assert!(honored.warnings.is_empty());
    assert!(honored.messages.iter().any(|message| {
        message == "verified 1 buildroot config_overrides entry against the final .config"
    }));
}

#[test]
fn shared_output_tree_is_checked_too() {
    let workspace = temp_path("gaia-override-check-shared");
    write_buildroot_source(
        &workspace,
        "build",
        &format!("{SHARED_MAKEFILE}{DROPPING_OLDDEFCONFIG}"),
    );
    let spec = feed_spec(&workspace, "override-shared", "build");

    let error = run_build(
        &spec,
        &image_with_overrides(overrides()),
        &policy(BuildrootOverrideCheckSpec::Error, true),
    )
    .expect_err("shared trees fail on dropped overrides");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("BR2_PACKAGE_OPENJDK dropped"));
    assert!(error.message.contains("/.gaia/cache/buildroot/shared/"));
}

#[test]
fn override_check_warnings_are_deduplicated() {
    let warning = format!("{OVERRIDE_CHECK_WARNING_PREFIX}BR2_X dropped");
    let warnings = override_check_warnings(&[
        "ran make".to_string(),
        warning.clone(),
        warning,
        "warning: something else".to_string(),
    ]);
    assert_eq!(warnings, vec!["buildroot config_overrides: BR2_X dropped"]);
}
