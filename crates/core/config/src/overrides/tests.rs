use super::*;

#[test]
fn documented_override_keys_parse_as_typed_keys() {
    for key in [
        "build.name",
        "build.display_name",
        "build.version",
        "build.description",
        "build.branch",
        "build.target",
        "build.profile",
        "preset",
        "preset.name",
        "product.family",
        "product.name",
        "product.sku",
        "workspace.root_dir",
        "workspace.build_dir",
        "workspace.out_dir",
        "workspace.export_dir",
        "image.feed.install_entries",
        "image.feed.stage_files",
        "image.feed.stage_env_sets",
        "image.feed.stage_services",
        "image.buildroot.defconfig",
        "image.allow_fallback",
        "image.buildroot.allow_fallback",
        "image.buildroot.external_tree",
        "image.buildroot.source",
        "image.buildroot.external_tree_mode",
        "image.starting-point.rootfs_path",
        "image.starting-point.source",
        "image.starting-point.source_path",
        "image.starting-point.rootfs_validation_mode",
        "image.starting-point.output_mode",
        "image.output.collect_dir",
        "image.output.archive_name",
        "image.output.export_dir",
        "reporting.post_build.timeout_seconds",
        "provenance.identity.project",
        "provenance.identity.vendor",
        "provenance.identity.channel",
        "policy.failure.rollback_on_error",
        "execution.jobs",
        "policy.execution.jobs",
        "execution.docker.enabled",
        "policy.execution.docker.enabled",
        "execution.docker.image",
        "policy.execution.docker.image",
        "execution.output_retention.stdout_bytes",
        "policy.execution.output_retention.stdout_bytes",
        "execution.output_retention.stderr_bytes",
        "policy.execution.output_retention.stderr_bytes",
        "execution.output_retention.stdout_lines",
        "policy.execution.output_retention.stdout_lines",
        "execution.output_retention.stderr_lines",
        "policy.execution.output_retention.stderr_lines",
        "execution.output_retention.failure_tail_lines",
        "policy.execution.output_retention.failure_tail_lines",
        "policy.failure.preserve_failed_outputs",
        "policy.failure.rollback_domains",
        "policy.providers.rust.allow_nested_build",
        "policy.providers.rust.batch_builds",
        "policy.providers.rust.shared_target_dir",
        "policy.failure.keep_going",
        "policy.failure.rollback_completed",
        "policy.providers.rust.retry_attempts",
        "policy.providers.rust.timeout_seconds",
        "policy.providers.git.allow_remote_resolution",
        "policy.providers.git.retry_attempts",
        "policy.providers.git.timeout_seconds",
        "policy.providers.archive.retry_attempts",
        "policy.providers.archive.timeout_seconds",
        "policy.providers.download.retry_attempts",
        "policy.providers.download.timeout_seconds",
        "policy.providers.go.retry_attempts",
        "policy.providers.go.timeout_seconds",
        "policy.providers.java.retry_attempts",
        "policy.providers.java.timeout_seconds",
        "policy.providers.java.gradle_home",
        "policy.providers.node.retry_attempts",
        "policy.providers.node.timeout_seconds",
        "policy.providers.python.retry_attempts",
        "policy.providers.python.timeout_seconds",
        "policy.providers.buildroot.retry_attempts",
        "policy.providers.buildroot.timeout_seconds",
        "policy.providers.buildroot.local_jobs",
        "policy.providers.buildroot.download_dir",
        "policy.providers.buildroot.ccache.enabled",
        "policy.providers.buildroot.ccache.dir",
        "policy.providers.buildroot.ccache.max_size",
        "policy.providers.buildroot.parallel_packages",
        "policy.providers.buildroot.package_cache.enabled",
        "policy.providers.buildroot.package_cache.dir",
        "policy.providers.buildroot.package_cache.system_dir",
        "policy.providers.buildroot.package_cache.project_dir",
        "policy.providers.buildroot.package_cache.level",
        "policy.providers.buildroot.package_cache.max_size",
        "policy.providers.buildroot.work_dir",
        "policy.providers.buildroot.ram_budget",
        "policy.providers.buildroot.keep_ram_tree",
        "policy.providers.buildroot.host_tools.default",
        "policy.providers.buildroot.host_tools.ccache",
        "policy.providers.buildroot.host_tools.pkgconf",
        "policy.providers.buildroot.shared_output",
        "policy.providers.buildroot.shared_output_dir",
        "policy.providers.buildroot.override_check",
        "policy.providers.starting_point.retry_attempts",
        "policy.providers.starting_point.timeout_seconds",
    ] {
        assert!(
            matches!(OverrideKey::parse(key), OverrideKey::Known(_)),
            "{key} did not parse as a known override"
        );
    }
}

#[test]
fn dynamic_override_prefixes_remain_explicit() {
    assert!(matches!(
        OverrideKey::parse("input.release"),
        OverrideKey::Input("release")
    ));
    assert!(matches!(
        OverrideKey::parse("env.GAIA_MODE"),
        OverrideKey::Env("GAIA_MODE")
    ));
    assert!(matches!(
        OverrideKey::parse("interpolation.values.version"),
        OverrideKey::InterpolationValue("version")
    ));
    assert!(matches!(
        OverrideKey::parse("build.labels.channel"),
        OverrideKey::BuildLabel("channel")
    ));
    assert!(matches!(
        OverrideKey::parse("provenance.identity.labels.owner"),
        OverrideKey::ProvenanceIdentityLabel("owner")
    ));
    assert!(matches!(
        OverrideKey::parse("workspace.paths.cache"),
        OverrideKey::WorkspacePath("cache")
    ));
}

#[test]
fn host_tools_overrides_store_valid_policies_and_reject_bad_steps() {
    let mut raw = raw::RawBuildConfig::default();
    apply_override(
        &mut raw,
        "policy.providers.buildroot.host_tools.ccache",
        "system,build",
    )
    .expect("ccache policy");
    apply_override(
        &mut raw,
        "policy.providers.buildroot.host_tools.default",
        "build",
    )
    .expect("default policy");
    let host_tools = &raw.providers.buildroot.host_tools;
    assert_eq!(
        host_tools.tools.get("ccache").map(String::as_str),
        Some("system,build")
    );
    assert_eq!(host_tools.default.as_deref(), Some("build"));

    assert!(
        apply_override(
            &mut raw,
            "policy.providers.buildroot.host_tools.pkgconf",
            "system,bogus",
        )
        .is_err()
    );
    assert!(apply_override(&mut raw, "policy.providers.buildroot.host_tools.ccache", "",).is_err());
    assert!(
        !raw.providers
            .buildroot
            .host_tools
            .tools
            .contains_key("pkgconf")
    );
}
