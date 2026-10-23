use super::*;
use gaia_spec::ErofsWorkersSpec;

fn buildroot_spec(workers: ErofsWorkersSpec) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("erofs-workers-test");
    spec.image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec::default()),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec::default(),
        assembly: None,
        signing: None,
        layers: None,
        checks: gaia_spec::ImageChecksSpec::default(),
    };
    spec.policy.providers.buildroot.erofs_workers = workers;
    spec
}

#[test]
fn erofs_lines_are_absent_when_workers_are_off() {
    assert_eq!(
        erofs_make_lines(&buildroot_spec(ErofsWorkersSpec::Off)),
        None
    );
    assert!(generated_package_digests(&buildroot_spec(ErofsWorkersSpec::Off)).is_empty());
}

#[test]
fn erofs_lines_enable_multithreading_only_for_erofs_images() {
    let lines = erofs_make_lines(&buildroot_spec(ErofsWorkersSpec::Count(4))).expect("lines");
    assert!(lines.contains("GAIA_EROFS_WORKERS = 4\n"), "{lines}");
    assert!(
        lines.contains("ifeq ($(BR2_TARGET_ROOTFS_EROFS),y)\n"),
        "{lines}"
    );
    assert!(
        lines.contains("HOST_EROFS_UTILS_CONF_OPTS += --enable-multithreading\n"),
        "{lines}"
    );
    assert!(
        lines.contains("ROOTFS_EROFS_ARGS += --workers=$(GAIA_EROFS_WORKERS)\n"),
        "{lines}"
    );
    assert!(lines.trim_end().ends_with("endif"), "{lines}");

    // auto is computed by make (the smaller of 8 and the cores), so the file
    // does not depend on the host that wrote it.
    let auto = erofs_make_lines(&buildroot_spec(ErofsWorkersSpec::Auto)).expect("auto");
    assert!(auto.contains("GAIA_EROFS_WORKERS ?= $(shell "), "{auto}");
    assert!(
        auto.contains("if [ $$n -gt 8 ]; then echo 8; else echo $$n; fi"),
        "{auto}"
    );
}

#[test]
fn a_tree_without_overrides_still_gets_the_erofs_lines() {
    let scratch = TempDir::new("gaia-erofs-tree");
    let buildroot_dir = scratch.join("buildroot");
    let output_dir = scratch.join("output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot");
    fs::create_dir_all(&output_dir).expect("output");
    let spec = buildroot_spec(ErofsWorkersSpec::Auto);

    let materialized = materialize_buildroot_package_overrides(&spec, &buildroot_dir, &output_dir)
        .expect("materialize");
    let tree = materialized
        .generated_external_tree
        .expect("generated tree");
    assert_eq!(tree.package_count, 0);
    let external_mk = fs::read_to_string(tree.path.join("external.mk")).expect("external.mk");
    assert!(external_mk.contains("HOST_EROFS_UTILS_CONF_OPTS += --enable-multithreading"));
    assert_eq!(
        buildroot_external_tree_value(&spec, None, Some(&tree.path)),
        Some(tree.path.display().to_string())
    );

    let off = buildroot_spec(ErofsWorkersSpec::Off);
    let materialized = materialize_buildroot_package_overrides(&off, &buildroot_dir, &output_dir)
        .expect("materialize off");
    assert!(materialized.generated_external_tree.is_none());
}

#[test]
fn the_erofs_setting_rebuilds_host_erofs_utils_through_the_package_digests() {
    let on = generated_package_digests(&buildroot_spec(ErofsWorkersSpec::Count(2)));
    let auto = generated_package_digests(&buildroot_spec(ErofsWorkersSpec::Auto));
    // Worker counts do not change the host build, so they do not rebuild it.
    assert_eq!(on, auto);
    assert_eq!(
        on.keys().map(String::as_str).collect::<Vec<_>>(),
        ["host-erofs-utils"]
    );
    assert!(generated_package_digests(&buildroot_spec(ErofsWorkersSpec::Off)).is_empty());
}
