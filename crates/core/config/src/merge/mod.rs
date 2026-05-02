use std::collections::BTreeMap;

use crate::raw::{
    RawBuildConfig, RawBuildrootExpectedImageConfig, RawExecutionPolicyConfig,
    RawFailurePolicyConfig, RawGitProviderPolicyConfig, RawImageConfig, RawImageDefinition,
    RawImageFeedConfig, RawImageOutputConfig, RawInputOptionConfig, RawInterpolationConfig,
    RawOutputRetentionPolicyConfig, RawPostBuildHookConfig, RawPresetConfig, RawProductConfig,
    RawProvenanceConfig, RawProvenanceIdentityConfig, RawProviderPoliciesConfig,
    RawReportingConfig, RawReportingMaskingConfig, RawRustProviderPolicyConfig, RawStageConfig,
    RawWhenConfig, RawWhenImageKind, RawWorkspaceNamedPathConfig,
};

pub fn merge_config(raw: RawBuildConfig) -> RawBuildConfig {
    let mut merged = raw
        .extends_config
        .as_deref()
        .cloned()
        .map(merge_config)
        .unwrap_or_default();

    let context = ImportWhenContext::from_raw(&raw);
    for imported in &raw.imported_configs {
        if import_when_matches(imported.import.when.as_ref(), &context) {
            merged = merge_two(merged, merge_config(imported.config.clone()));
        }
    }

    merge_two(merged, strip_loaded_children(raw))
}

struct ImportWhenContext {
    target: Option<String>,
    profile: Option<String>,
    branch: Option<String>,
    image_kind: RawWhenImageKind,
}

impl ImportWhenContext {
    fn from_raw(raw: &RawBuildConfig) -> Self {
        let selected_inputs = selected_inputs_for_imports(raw);
        let mut target = raw
            .target
            .clone()
            .map(|value| interpolate_import_context_value(&value, &selected_inputs));
        let mut profile = raw
            .profile
            .clone()
            .map(|value| interpolate_import_context_value(&value, &selected_inputs));
        let mut branch = raw
            .branch
            .clone()
            .map(|value| interpolate_import_context_value(&value, &selected_inputs));
        for (key, value) in &raw.explicit_overrides {
            match key.as_str() {
                "build.target" => {
                    target = Some(interpolate_import_context_value(value, &selected_inputs))
                }
                "build.profile" => {
                    profile = Some(interpolate_import_context_value(value, &selected_inputs))
                }
                "build.branch" => {
                    branch = Some(interpolate_import_context_value(value, &selected_inputs))
                }
                _ => {}
            }
        }
        Self {
            target,
            profile,
            branch,
            image_kind: match raw.image.definition {
                RawImageDefinition::Buildroot { .. } => RawWhenImageKind::Buildroot,
                RawImageDefinition::StartingPoint { .. } => RawWhenImageKind::StartingPoint,
            },
        }
    }

    fn interpolate_expected(&self, value: &str) -> String {
        value
            .replace(
                "${build.target}",
                self.target.as_deref().unwrap_or_default(),
            )
            .replace(
                "${build.profile}",
                self.profile.as_deref().unwrap_or_default(),
            )
            .replace(
                "${build.branch}",
                self.branch.as_deref().unwrap_or_default(),
            )
    }
}

fn import_when_matches(when: Option<&RawWhenConfig>, context: &ImportWhenContext) -> bool {
    let Some(when) = when else {
        return true;
    };
    let target_matches = when.target.as_ref().is_none_or(|expected| {
        context.target.as_deref() == Some(context.interpolate_expected(expected).as_str())
    });
    let profile_matches = when.profile.as_ref().is_none_or(|expected| {
        context.profile.as_deref() == Some(context.interpolate_expected(expected).as_str())
    });
    let branch_matches = when.branch.as_ref().is_none_or(|expected| {
        context.branch.as_deref() == Some(context.interpolate_expected(expected).as_str())
    });
    let image_kind_matches = when
        .image_kind
        .is_none_or(|expected| expected == context.image_kind);
    let all_matches = when
        .all
        .iter()
        .all(|item| import_when_matches(Some(item), context));
    let any_matches = if when.any.is_empty() {
        true
    } else {
        when.any
            .iter()
            .any(|item| import_when_matches(Some(item), context))
    };
    let not_matches = when
        .not
        .as_ref()
        .is_none_or(|item| !import_when_matches(Some(item.as_ref()), context));

    target_matches
        && profile_matches
        && branch_matches
        && image_kind_matches
        && all_matches
        && any_matches
        && not_matches
}

fn selected_inputs_for_imports(raw: &RawBuildConfig) -> BTreeMap<String, String> {
    let mut selected = raw
        .selected_inputs
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    for (key, value) in &raw.explicit_overrides {
        if let Some(name) = key.strip_prefix("input.") {
            selected.insert(name.to_string(), value.clone());
        } else if let Some(name) = key.strip_prefix("inputs.") {
            selected.insert(name.to_string(), value.clone());
        }
    }
    selected
}

fn interpolate_import_context_value(
    value: &str,
    selected_inputs: &BTreeMap<String, String>,
) -> String {
    let mut output = String::new();
    let mut rest = value;

    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let token = &remainder[..end];
        let replacement = token
            .strip_prefix("input.")
            .or_else(|| token.strip_prefix("inputs."))
            .and_then(|name| selected_inputs.get(name))
            .cloned()
            .unwrap_or_default();
        output.push_str(&replacement);
        rest = &remainder[end + 1..];
    }

    output.push_str(rest);
    output
}

fn merge_two(mut base: RawBuildConfig, overlay: RawBuildConfig) -> RawBuildConfig {
    if !overlay.build_name.trim().is_empty() {
        base.build_name = overlay.build_name;
    }
    if overlay.display_name.is_some() {
        base.display_name = overlay.display_name;
    }
    if overlay.version.is_some() {
        base.version = overlay.version;
    }
    if overlay.description.is_some() {
        base.description = overlay.description;
    }
    if overlay.branch.is_some() {
        base.branch = overlay.branch;
    }
    if overlay.target.is_some() {
        base.target = overlay.target;
    }
    if overlay.profile.is_some() {
        base.profile = overlay.profile;
    }
    base.labels = merge_named_paths(base.labels, overlay.labels);
    base.product = merge_product(base.product, overlay.product);
    base.inputs = merge_inputs(base.inputs, overlay.inputs);
    if overlay.preset.is_some() {
        base.preset = overlay.preset;
    }
    base.presets = merge_presets(base.presets, overlay.presets);
    if overlay.source_path.is_some() {
        base.source_path = overlay.source_path;
    }

    base.env_files = merge_string_lists(base.env_files, overlay.env_files);
    base.env.extend(overlay.env);

    if !overlay.workspace.root_dir.trim().is_empty() {
        base.workspace.root_dir = overlay.workspace.root_dir;
    }
    if !overlay.workspace.build_dir.trim().is_empty() {
        base.workspace.build_dir = overlay.workspace.build_dir;
    }
    if !overlay.workspace.out_dir.trim().is_empty() {
        base.workspace.out_dir = overlay.workspace.out_dir;
    }
    base.workspace.named_paths =
        merge_workspace_named_paths(base.workspace.named_paths, overlay.workspace.named_paths);

    base.sources = merge_by_key(base.sources, overlay.sources, |item| item.id.clone());
    base.artifacts = merge_by_key(base.artifacts, overlay.artifacts, |item| item.id.clone());
    base.install = merge_by_key(base.install, overlay.install, |item| item.id.clone());
    base.stage = merge_stage(base.stage, overlay.stage);

    base.image = merge_image(base.image, overlay.image);
    base.checkpoints = merge_by_key(base.checkpoints, overlay.checkpoints, |item| {
        item.id.clone()
    });
    base.interpolation = merge_interpolation(base.interpolation, overlay.interpolation);
    base.clean = merge_clean(base.clean, overlay.clean);
    base.execution = merge_execution_policy(base.execution, overlay.execution);
    base.failure = merge_failure_policy(base.failure, overlay.failure);
    base.providers = merge_provider_policies(base.providers, overlay.providers);
    base.provenance = merge_provenance(base.provenance, overlay.provenance);
    base.reporting = merge_reporting(base.reporting, overlay.reporting);

    base
}

fn merge_clean(
    mut base: crate::raw::RawCleanConfig,
    overlay: crate::raw::RawCleanConfig,
) -> crate::raw::RawCleanConfig {
    if overlay.default.is_some() {
        base.default = overlay.default;
    }
    for (name, profile) in overlay.profiles {
        base.profiles.insert(name, profile);
    }
    base
}

fn merge_execution_policy(
    base: RawExecutionPolicyConfig,
    overlay: RawExecutionPolicyConfig,
) -> RawExecutionPolicyConfig {
    RawExecutionPolicyConfig {
        jobs: base.jobs.max(overlay.jobs),
        docker: crate::raw::RawDockerExecutionConfig {
            enabled: base.docker.enabled || overlay.docker.enabled,
            image: overlay.docker.image.or(base.docker.image),
        },
        output_retention: merge_output_retention(base.output_retention, overlay.output_retention),
    }
}

fn merge_output_retention(
    base: RawOutputRetentionPolicyConfig,
    overlay: RawOutputRetentionPolicyConfig,
) -> RawOutputRetentionPolicyConfig {
    RawOutputRetentionPolicyConfig {
        stdout_bytes: overlay.stdout_bytes.max(base.stdout_bytes),
        stderr_bytes: overlay.stderr_bytes.max(base.stderr_bytes),
        stdout_lines: overlay.stdout_lines.max(base.stdout_lines),
        stderr_lines: overlay.stderr_lines.max(base.stderr_lines),
        failure_tail_lines: overlay.failure_tail_lines.max(base.failure_tail_lines),
    }
}

fn strip_loaded_children(mut raw: RawBuildConfig) -> RawBuildConfig {
    raw.extends = None;
    raw.imports.clear();
    raw.extends_config = None;
    raw.imported_configs.clear();
    raw
}

fn merge_stage(base: RawStageConfig, overlay: RawStageConfig) -> RawStageConfig {
    RawStageConfig {
        files: merge_by_key(base.files, overlay.files, |item| item.id.clone()),
        env_sets: merge_by_key(base.env_sets, overlay.env_sets, |item| item.id.clone()),
        services: merge_by_key(base.services, overlay.services, |item| item.id.clone()),
    }
}

fn merge_image(base: RawImageConfig, overlay: RawImageConfig) -> RawImageConfig {
    RawImageConfig {
        definition: merge_image_definition(base.definition, overlay.definition),
        feed: RawImageFeedConfig {
            install_entries: merge_string_lists(
                base.feed.install_entries,
                overlay.feed.install_entries,
            ),
            stage_files: merge_string_lists(base.feed.stage_files, overlay.feed.stage_files),
            stage_env_sets: merge_string_lists(
                base.feed.stage_env_sets,
                overlay.feed.stage_env_sets,
            ),
            stage_services: merge_string_lists(
                base.feed.stage_services,
                overlay.feed.stage_services,
            ),
        },
        output: RawImageOutputConfig {
            collect_dir: overlay.output.collect_dir.or(base.output.collect_dir),
            archive_name: overlay.output.archive_name.or(base.output.archive_name),
            emit_report: base.output.emit_report || overlay.output.emit_report,
        },
    }
}

fn merge_image_definition(
    base: RawImageDefinition,
    overlay: RawImageDefinition,
) -> RawImageDefinition {
    match (base, overlay) {
        (
            RawImageDefinition::Buildroot {
                source: base_source,
                defconfig: base_defconfig,
                defconfig_path: base_defconfig_path,
                allow_fallback: base_allow_fallback,
                config_fragments: base_config_fragments,
                config_overrides: base_config_overrides,
                external_tree: base_external_tree,
                external_tree_mode: base_external_tree_mode,
                expected_images: base_expected_images,
            },
            RawImageDefinition::Buildroot {
                source: overlay_source,
                defconfig: overlay_defconfig,
                defconfig_path: overlay_defconfig_path,
                allow_fallback: overlay_allow_fallback,
                config_fragments: overlay_config_fragments,
                config_overrides: overlay_config_overrides,
                external_tree: overlay_external_tree,
                external_tree_mode: overlay_external_tree_mode,
                expected_images: overlay_expected_images,
            },
        ) => RawImageDefinition::Buildroot {
            source: overlay_source.or(base_source),
            defconfig: overlay_defconfig.or(base_defconfig),
            defconfig_path: overlay_defconfig_path.or(base_defconfig_path),
            allow_fallback: base_allow_fallback || overlay_allow_fallback,
            config_fragments: merge_string_lists(base_config_fragments, overlay_config_fragments),
            config_overrides: merge_override_pairs(base_config_overrides, overlay_config_overrides),
            external_tree: overlay_external_tree.or(base_external_tree),
            external_tree_mode: overlay_external_tree_mode.or(base_external_tree_mode),
            expected_images: merge_expected_images(base_expected_images, overlay_expected_images),
        },
        (
            RawImageDefinition::StartingPoint {
                source: base_source,
                source_path: base_source_path,
                rootfs_path: base_rootfs_path,
                image_partition: base_image_partition,
                image_read_only: base_image_read_only,
                packages: base_packages,
                rootfs_validation_mode: base_rootfs_validation_mode,
                output_mode: base_output_mode,
            },
            RawImageDefinition::StartingPoint {
                source: overlay_source,
                source_path: overlay_source_path,
                rootfs_path: overlay_rootfs_path,
                image_partition: overlay_image_partition,
                image_read_only: overlay_image_read_only,
                packages: overlay_packages,
                rootfs_validation_mode: overlay_rootfs_validation_mode,
                output_mode: overlay_output_mode,
            },
        ) => RawImageDefinition::StartingPoint {
            source: overlay_source.or(base_source),
            source_path: overlay_source_path.or(base_source_path),
            rootfs_path: if overlay_rootfs_path.trim().is_empty() {
                base_rootfs_path
            } else {
                overlay_rootfs_path
            },
            image_partition: overlay_image_partition.or(base_image_partition),
            image_read_only: overlay_image_read_only && base_image_read_only,
            packages: crate::raw::RawStartingPointPackagesConfig {
                enabled: base_packages.enabled || overlay_packages.enabled,
                execute: base_packages.execute || overlay_packages.execute,
                manager: overlay_packages.manager.or(base_packages.manager),
                release_version: overlay_packages
                    .release_version
                    .or(base_packages.release_version),
                allow_major_upgrade: base_packages.allow_major_upgrade
                    || overlay_packages.allow_major_upgrade,
                update: base_packages.update || overlay_packages.update,
                dist_upgrade: base_packages.dist_upgrade || overlay_packages.dist_upgrade,
                install: if overlay_packages.install.is_empty() {
                    base_packages.install
                } else {
                    overlay_packages.install
                },
                remove: if overlay_packages.remove.is_empty() {
                    base_packages.remove
                } else {
                    overlay_packages.remove
                },
                extra_args: if overlay_packages.extra_args.is_empty() {
                    base_packages.extra_args
                } else {
                    overlay_packages.extra_args
                },
                os_release_path: overlay_packages
                    .os_release_path
                    .or(base_packages.os_release_path),
            },
            rootfs_validation_mode: overlay_rootfs_validation_mode.or(base_rootfs_validation_mode),
            output_mode: overlay_output_mode.or(base_output_mode),
        },
        (
            base_definition,
            RawImageDefinition::Buildroot {
                source: _,
                defconfig: None,
                defconfig_path: None,
                allow_fallback: false,
                config_fragments,
                config_overrides,
                external_tree: None,
                external_tree_mode: None,
                expected_images,
            },
        ) if expected_images.is_empty()
            && config_fragments.is_empty()
            && config_overrides.is_empty() =>
        {
            base_definition
        }
        (base_definition, overlay_definition) => match overlay_definition {
            RawImageDefinition::StartingPoint {
                source,
                source_path,
                rootfs_path,
                image_partition,
                packages,
                rootfs_validation_mode,
                output_mode,
                image_read_only,
            } if source.is_none()
                && source_path.is_none()
                && rootfs_path.trim().is_empty()
                && image_partition.is_none()
                && rootfs_validation_mode.is_none()
                && output_mode.is_none()
                && image_read_only
                && !packages.enabled
                && !packages.execute
                && packages.manager.is_none()
                && packages.release_version.is_none()
                && !packages.allow_major_upgrade
                && !packages.update
                && !packages.dist_upgrade
                && packages.install.is_empty()
                && packages.remove.is_empty()
                && packages.extra_args.is_empty()
                && packages.os_release_path.is_none() =>
            {
                base_definition
            }
            overlay_definition => overlay_definition,
        },
    }
}

fn merge_reporting(base: RawReportingConfig, overlay: RawReportingConfig) -> RawReportingConfig {
    RawReportingConfig {
        summary: base.summary || overlay.summary,
        provenance: base.provenance || overlay.provenance,
        manifest: base.manifest || overlay.manifest,
        masking: merge_reporting_masking(base.masking, overlay.masking),
        post_build: merge_reporting_post_build(base.post_build, overlay.post_build),
    }
}

fn merge_reporting_masking(
    base: RawReportingMaskingConfig,
    overlay: RawReportingMaskingConfig,
) -> RawReportingMaskingConfig {
    RawReportingMaskingConfig {
        enabled: base.enabled || overlay.enabled,
        replacement: if overlay.replacement.is_empty() {
            base.replacement
        } else {
            overlay.replacement
        },
        patterns: merge_string_lists(base.patterns, overlay.patterns),
    }
}

fn merge_reporting_post_build(
    base: Option<RawPostBuildHookConfig>,
    overlay: Option<RawPostBuildHookConfig>,
) -> Option<RawPostBuildHookConfig> {
    match (base, overlay) {
        (Some(mut base), Some(overlay)) => {
            if !overlay.script.trim().is_empty() {
                base.script = overlay.script;
            }
            if overlay.timeout_seconds != 0 {
                base.timeout_seconds = overlay.timeout_seconds;
            }
            Some(base)
        }
        (None, Some(overlay)) if !overlay.script.trim().is_empty() => Some(overlay),
        (base, Some(_)) => base,
        (base, None) => base,
    }
}

fn merge_interpolation(
    base: RawInterpolationConfig,
    overlay: RawInterpolationConfig,
) -> RawInterpolationConfig {
    RawInterpolationConfig {
        allow_unresolved: base.allow_unresolved || overlay.allow_unresolved,
        values: merge_named_paths(base.values, overlay.values),
    }
}

fn merge_failure_policy(
    base: RawFailurePolicyConfig,
    overlay: RawFailurePolicyConfig,
) -> RawFailurePolicyConfig {
    RawFailurePolicyConfig {
        rollback_on_error: overlay.rollback_on_error.or(base.rollback_on_error),
        preserve_failed_outputs: overlay
            .preserve_failed_outputs
            .or(base.preserve_failed_outputs),
        rollback_domains: overlay.rollback_domains.or(base.rollback_domains),
    }
}

fn merge_provider_policies(
    base: RawProviderPoliciesConfig,
    overlay: RawProviderPoliciesConfig,
) -> RawProviderPoliciesConfig {
    RawProviderPoliciesConfig {
        rust: RawRustProviderPolicyConfig {
            allow_nested_build: base.rust.allow_nested_build || overlay.rust.allow_nested_build,
            retry_attempts: base.rust.retry_attempts.max(overlay.rust.retry_attempts),
            retry_backoff_ms: base
                .rust
                .retry_backoff_ms
                .max(overlay.rust.retry_backoff_ms),
            retry_backoff_strategy: overlay.rust.retry_backoff_strategy,
            timeout_seconds: base.rust.timeout_seconds.max(overlay.rust.timeout_seconds),
        },
        git: RawGitProviderPolicyConfig {
            allow_remote_resolution: base.git.allow_remote_resolution
                || overlay.git.allow_remote_resolution,
            retry_attempts: base.git.retry_attempts.max(overlay.git.retry_attempts),
            retry_backoff_ms: base.git.retry_backoff_ms.max(overlay.git.retry_backoff_ms),
            retry_backoff_strategy: overlay.git.retry_backoff_strategy,
            timeout_seconds: base.git.timeout_seconds.max(overlay.git.timeout_seconds),
        },
        archive: merge_command_policy(base.archive, overlay.archive),
        download: merge_command_policy(base.download, overlay.download),
        go: merge_command_policy(base.go, overlay.go),
        java: merge_command_policy(base.java, overlay.java),
        node: merge_command_policy(base.node, overlay.node),
        python: merge_command_policy(base.python, overlay.python),
        buildroot: merge_command_policy(base.buildroot, overlay.buildroot),
        starting_point: merge_command_policy(base.starting_point, overlay.starting_point),
    }
}

fn merge_command_policy(
    base: crate::raw::RawCommandProviderPolicyConfig,
    overlay: crate::raw::RawCommandProviderPolicyConfig,
) -> crate::raw::RawCommandProviderPolicyConfig {
    crate::raw::RawCommandProviderPolicyConfig {
        retry_attempts: base.retry_attempts.max(overlay.retry_attempts),
        retry_backoff_ms: base.retry_backoff_ms.max(overlay.retry_backoff_ms),
        retry_backoff_strategy: overlay.retry_backoff_strategy,
        timeout_seconds: base.timeout_seconds.max(overlay.timeout_seconds),
        local_jobs: base.local_jobs.max(overlay.local_jobs),
    }
}

fn merge_provenance(
    base: RawProvenanceConfig,
    overlay: RawProvenanceConfig,
) -> RawProvenanceConfig {
    RawProvenanceConfig {
        identity: RawProvenanceIdentityConfig {
            project: overlay.identity.project.or(base.identity.project),
            vendor: overlay.identity.vendor.or(base.identity.vendor),
            channel: overlay.identity.channel.or(base.identity.channel),
            labels: merge_named_paths(base.identity.labels, overlay.identity.labels),
        },
    }
}

fn merge_product(base: RawProductConfig, overlay: RawProductConfig) -> RawProductConfig {
    RawProductConfig {
        family: overlay.family.or(base.family),
        name: overlay.name.or(base.name),
        sku: overlay.sku.or(base.sku),
    }
}

fn merge_presets(
    mut base: BTreeMap<String, RawPresetConfig>,
    overlay: BTreeMap<String, RawPresetConfig>,
) -> BTreeMap<String, RawPresetConfig> {
    for (name, preset) in overlay {
        match base.remove(&name) {
            Some(existing) => {
                base.insert(name, merge_preset(existing, preset));
            }
            None => {
                base.insert(name, preset);
            }
        }
    }
    base
}

fn merge_inputs(
    mut base: BTreeMap<String, RawInputOptionConfig>,
    overlay: BTreeMap<String, RawInputOptionConfig>,
) -> BTreeMap<String, RawInputOptionConfig> {
    for (name, input) in overlay {
        base.insert(name, input);
    }
    base
}

fn merge_preset(base: RawPresetConfig, overlay: RawPresetConfig) -> RawPresetConfig {
    RawPresetConfig {
        env_files: merge_string_lists(base.env_files, overlay.env_files),
        env: {
            let mut env = base.env;
            env.extend(overlay.env);
            env
        },
        overrides: merge_named_paths(base.overrides, overlay.overrides),
    }
}

fn merge_workspace_named_paths(
    base: Vec<RawWorkspaceNamedPathConfig>,
    overlay: Vec<RawWorkspaceNamedPathConfig>,
) -> Vec<RawWorkspaceNamedPathConfig> {
    let mut merged = BTreeMap::new();
    for entry in base {
        merged.insert(entry.alias.clone(), entry);
    }
    for entry in overlay {
        merged.insert(entry.alias.clone(), entry);
    }
    merged.into_values().collect()
}

fn merge_named_paths(
    base: Vec<(String, String)>,
    overlay: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut merged = BTreeMap::new();
    for (key, value) in base {
        merged.insert(key, value);
    }
    for (key, value) in overlay {
        merged.insert(key, value);
    }
    merged.into_iter().collect()
}

fn merge_override_pairs(
    base: Vec<(String, String)>,
    overlay: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut merged = BTreeMap::new();
    for (key, value) in base {
        merged.insert(key, value);
    }
    for (key, value) in overlay {
        merged.insert(key, value);
    }
    merged.into_iter().collect()
}

fn merge_expected_images(
    base: Vec<RawBuildrootExpectedImageConfig>,
    overlay: Vec<RawBuildrootExpectedImageConfig>,
) -> Vec<RawBuildrootExpectedImageConfig> {
    let mut merged = BTreeMap::new();
    for image in base {
        merged.insert(image.name.clone(), image);
    }
    for image in overlay {
        merged.insert(image.name.clone(), image);
    }
    merged.into_values().collect()
}

fn merge_string_lists(base: Vec<String>, overlay: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    for value in base.into_iter().chain(overlay) {
        if !merged.contains(&value) {
            merged.push(value);
        }
    }
    merged
}

fn merge_by_key<T, K, F>(base: Vec<T>, overlay: Vec<T>, key: F) -> Vec<T>
where
    K: Ord,
    F: Fn(&T) -> K,
{
    let mut merged = BTreeMap::new();
    for item in base {
        merged.insert(key(&item), item);
    }
    for item in overlay {
        merged.insert(key(&item), item);
    }
    merged.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::merge_image_definition;
    use crate::raw::{
        RawBuildrootExpectedImageConfig, RawBuildrootExpectedImageFormat, RawImageDefinition,
    };

    #[test]
    fn layered_buildroot_image_definition_composes_overrides_and_expected_images() {
        let merged = merge_image_definition(
            RawImageDefinition::Buildroot {
                source: None,
                defconfig: None,
                defconfig_path: None,
                allow_fallback: false,
                config_fragments: vec!["storage.fragment".into()],
                config_overrides: vec![
                    ("BR2_TARGET_ROOTFS_EXT2".into(), "n".into()),
                    ("BR2_TARGET_ROOTFS_SQUASHFS".into(), "y".into()),
                ],
                external_tree: None,
                external_tree_mode: None,
                expected_images: vec![RawBuildrootExpectedImageConfig {
                    name: "rootfs.squashfs".into(),
                    format: RawBuildrootExpectedImageFormat::Squashfs,
                    required: true,
                }],
            },
            RawImageDefinition::Buildroot {
                source: None,
                defconfig: Some("raspberrypicm5io_defconfig".into()),
                defconfig_path: None,
                allow_fallback: false,
                config_fragments: vec!["target.fragment".into()],
                config_overrides: vec![("BR2_ROOTFS_POST_IMAGE_SCRIPT".into(), "\"\"".into())],
                external_tree: None,
                external_tree_mode: None,
                expected_images: vec![],
            },
        );

        match merged {
            RawImageDefinition::Buildroot {
                defconfig,
                config_fragments,
                config_overrides,
                expected_images,
                ..
            } => {
                assert_eq!(defconfig.as_deref(), Some("raspberrypicm5io_defconfig"));
                assert_eq!(
                    config_fragments,
                    vec![
                        "storage.fragment".to_string(),
                        "target.fragment".to_string()
                    ]
                );
                assert_eq!(
                    config_overrides,
                    vec![
                        (
                            "BR2_ROOTFS_POST_IMAGE_SCRIPT".to_string(),
                            "\"\"".to_string()
                        ),
                        ("BR2_TARGET_ROOTFS_EXT2".to_string(), "n".to_string()),
                        ("BR2_TARGET_ROOTFS_SQUASHFS".to_string(), "y".to_string()),
                    ]
                );
                assert_eq!(expected_images.len(), 1);
                assert_eq!(expected_images[0].name, "rootfs.squashfs");
                assert!(expected_images[0].required);
            }
            other => panic!("expected buildroot image, got {other:?}"),
        }
    }
}
