//! "Did you mean" hints for override keys that name no setting.

/// Every complete key spelling `OverrideKey::parse` accepts, aliases
/// included. Only used to suggest the closest key for an unknown one; the
/// test below keeps each entry parseable.
const LITERAL_KEYS: &[&str] = &[
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
    "image.legal_info",
    "image.buildroot.external_tree",
    "image.buildroot.source",
    "image.buildroot.external_tree_mode",
    "image.rootfs.exclude_packages",
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
    "policy.failure.keep_going",
    "policy.failure.rollback_completed",
    "policy.providers.rust.allow_nested_build",
    "policy.providers.rust.batch_builds",
    "policy.providers.rust.shared_target_dir",
    "policy.providers.rust.sccache",
    "policy.providers.rust.strip",
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
    "policy.providers.buildroot.make_timeout_seconds",
    "policy.providers.buildroot.make_retry_attempts",
    "policy.providers.buildroot.erofs_workers",
    "policy.providers.buildroot.download_dir",
    "policy.providers.buildroot.ccache.enabled",
    "policy.providers.buildroot.ccache.dir",
    "policy.providers.buildroot.ccache.max_size",
    "policy.providers.buildroot.parallel_packages",
    "policy.providers.buildroot.package_cache.enabled",
    "policy.providers.buildroot.package_cache.dir",
    "policy.providers.buildroot.package_cache.system_dir",
    "policy.providers.buildroot.package_cache.level",
    "policy.providers.buildroot.package_cache.project_dir",
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
    "policy.providers.buildroot.kernel_modules_check",
    "policy.providers.starting_point.retry_attempts",
    "policy.providers.starting_point.timeout_seconds",
];

/// `" (did you mean 'a', 'b'?)"` for the closest known keys, or nothing.
/// Keys are compared in full and by their last segment, so a misspelled
/// leaf such as `image.rootfs_pth` still finds `image.starting-point.rootfs_path`.
pub(super) fn did_you_mean(wanted: &str) -> String {
    let leaf = last_segment(wanted);
    let limit = (leaf.chars().count() / 3).max(2);
    let mut scored = LITERAL_KEYS
        .iter()
        .filter_map(|id| {
            let distance = edit_distance(wanted, id).min(edit_distance(leaf, last_segment(id)));
            (distance <= limit).then_some((distance, *id))
        })
        .collect::<Vec<_>>();
    scored.sort();
    scored.truncate(3);
    if scored.is_empty() {
        return String::new();
    }
    let names = scored
        .iter()
        .map(|(_, id)| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" (did you mean {names}?)")
}

fn last_segment(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::super::keys::OverrideKey;
    use super::*;

    #[test]
    fn every_literal_key_is_a_known_key() {
        for key in LITERAL_KEYS {
            assert_ne!(
                OverrideKey::parse(key),
                OverrideKey::Unknown,
                "suggestion list names '{key}', which the parser does not accept"
            );
        }
    }

    #[test]
    fn misspelled_leaf_suggests_the_closest_key() {
        assert!(did_you_mean("image.rootfs_pth").contains("'image.starting-point.rootfs_path'"));
        assert!(did_you_mean("build.nme").contains("'build.name'"));
    }

    #[test]
    fn unrelated_key_gets_no_suggestion() {
        assert_eq!(did_you_mean("zzzz.qqqq.wwww"), "");
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("name", "name"), 0);
        assert_eq!(edit_distance("nme", "name"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
