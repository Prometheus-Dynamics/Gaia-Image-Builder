mod image;

use std::collections::BTreeMap;

use crate::raw::{
    RawBuildConfig, RawBuildrootExpectedImageConfig, RawExecutionPolicyConfig,
    RawFailurePolicyConfig, RawGitProviderPolicyConfig, RawImageDefinition, RawInputOptionConfig,
    RawInterpolationConfig, RawOutputHygieneConfig, RawOutputRetentionPolicyConfig,
    RawPostBuildHookConfig, RawPresetConfig, RawProductConfig, RawProvenanceConfig,
    RawProvenanceIdentityConfig, RawProviderPoliciesConfig, RawReportingConfig,
    RawReportingMaskingConfig, RawRustProviderPolicyConfig, RawStageConfig, RawWhenConfig,
    RawWhenImageKind, RawWorkspaceNamedPathConfig,
};
use image::merge_image;

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

fn merge_reporting(base: RawReportingConfig, overlay: RawReportingConfig) -> RawReportingConfig {
    RawReportingConfig {
        summary: base.summary || overlay.summary,
        provenance: base.provenance || overlay.provenance,
        manifest: base.manifest || overlay.manifest,
        masking: merge_reporting_masking(base.masking, overlay.masking),
        output_hygiene: merge_output_hygiene(base.output_hygiene, overlay.output_hygiene),
        post_build: merge_reporting_post_build(base.post_build, overlay.post_build),
    }
}

fn merge_output_hygiene(
    base: RawOutputHygieneConfig,
    overlay: RawOutputHygieneConfig,
) -> RawOutputHygieneConfig {
    RawOutputHygieneConfig {
        large_file_threshold_bytes: overlay
            .large_file_threshold_bytes
            .or(base.large_file_threshold_bytes),
        transient_dir_names: overlay.transient_dir_names.or(base.transient_dir_names),
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
        download_dir: overlay.download_dir.or(base.download_dir),
        ccache: crate::raw::RawBuildrootCcachePolicyConfig {
            enabled: base.ccache.enabled || overlay.ccache.enabled,
            dir: overlay.ccache.dir.or(base.ccache.dir),
        },
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

pub(super) fn merge_override_pairs(
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

pub(super) fn merge_expected_images(
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

pub(super) fn merge_string_lists(base: Vec<String>, overlay: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    for value in base.into_iter().chain(overlay) {
        if !merged.contains(&value) {
            merged.push(value);
        }
    }
    merged
}

pub(super) fn merge_by_key<T, K, F>(base: Vec<T>, overlay: Vec<T>, key: F) -> Vec<T>
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
