//! The Buildroot config steps: materializing package overrides, then defconfig,
//! fragments, config overrides and the cache settings, and what those changed
//! against the last build. A run and `gaia preview` (on a scratch copy) both
//! use them, so the preview sees the config a run would build.
use super::*;

/// The result of the config steps.
pub(crate) struct ConfiguredTree {
    pub(crate) package_overrides: MaterializedBuildrootPackageOverrides,
    /// `BR2_EXTERNAL`, when the build has an external tree.
    pub(crate) br2_external: Option<String>,
    pub(crate) messages: Vec<String>,
}

/// Runs the config steps in `output_dir`. `dry_run` writes no file outside
/// the tree (the compiler cache's config), for previews.
pub(crate) fn configure_tree(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    command_context: &ImageCommandContext<'_>,
    dry_run: bool,
) -> Result<ConfiguredTree, ImageProviderError> {
    let mut messages = Vec::new();
    let (defconfig, defconfig_path, config_fragments, config_overrides, external_tree) =
        match &image.definition {
            ImageDefinition::Buildroot(buildroot) => (
                buildroot.defconfig.as_deref(),
                buildroot.defconfig_path.as_deref(),
                buildroot.config_fragments.as_slice(),
                buildroot.config_overrides.as_slice(),
                buildroot.external_tree.as_deref(),
            ),
            _ => (None, None, &[][..], &[][..], None),
        };

    let clock = gaia_process::ActiveClock::start();
    let package_overrides =
        materialize_buildroot_package_overrides(spec, buildroot_dir, output_dir)?;
    if package_overrides.generated_external_tree.is_some() {
        ensure_no_generated_external_name_conflict(external_tree)?;
    }
    messages.push(phase_step_message("package overrides", &clock, &[]));
    // The config steps: everything after the overrides, less the commands
    // they ran (which report their own step times).
    let config_clock = gaia_process::ActiveClock::start();
    let config_messages_from = messages.len();
    let br2_external = buildroot_external_tree_value(
        spec,
        external_tree,
        package_overrides
            .generated_external_tree
            .as_ref()
            .map(|generated| generated.path.as_path()),
    );
    let br2 = br2_external.as_deref();
    if let Some(generated_external_tree) = &package_overrides.generated_external_tree {
        messages.push(format!(
            "staged {} generated Buildroot external package override(s) at '{}'",
            generated_external_tree.package_count,
            generated_external_tree.path.display()
        ));
    }
    if package_overrides.replacement_count > 0 {
        messages.push(format!(
            "replaced {} Buildroot source package definition(s)",
            package_overrides.replacement_count
        ));
    }

    // The `make` arguments selecting the defconfig, when there is one.
    let resolved_defconfig = defconfig_path
        .map(|defconfig_path| {
            resolve_workspace_path(
                &ResolvedBuildSpec {
                    workspace: spec.workspace.clone(),
                    ..spec.clone()
                },
                defconfig_path,
            )
        })
        .transpose()?;
    let defconfig_args = match (&resolved_defconfig, defconfig) {
        (Some(resolved_defconfig_path), _) => {
            materialize_defconfig_support_files(resolved_defconfig_path, output_dir)?;
            Some(vec![
                "defconfig".to_string(),
                format!("BR2_DEFCONFIG={}", resolved_defconfig_path.display()),
            ])
        }
        (None, Some(defconfig)) => Some(vec![defconfig.to_string()]),
        (None, None) => None,
    };
    // The config steps rebuild `.config` from their inputs: when those and
    // the `.config` they left are unchanged, the steps are skipped (see
    // config_inputs). Previews always run them.
    let has_defconfig = defconfig_args.is_some();
    let config_inputs = if has_defconfig && !dry_run {
        let fragment_paths = config_fragments
            .iter()
            .map(|fragment| resolve_workspace_path(spec, fragment))
            .collect::<Result<Vec<_>, _>>()?;
        let normalized_overrides = normalize_buildroot_config_overrides(spec, config_overrides);
        let (cache_overrides, _) = buildroot_cache_overrides(spec, command_context.policy, true)?;
        Some(config_inputs_digest(&ConfigInputs {
            buildroot_dir,
            external_tree: br2.map(Path::new),
            defconfig_file: resolved_defconfig.as_deref(),
            defconfig_name: defconfig.filter(|_| resolved_defconfig.is_none()),
            fragments: &fragment_paths,
            overrides: &normalized_overrides,
            cache_overrides: &cache_overrides,
            package_replacements: package_overrides.replacement_digest.as_deref(),
        }))
    } else {
        None
    };
    let config_current = config_inputs
        .as_deref()
        .is_some_and(|digest| config_steps_current(output_dir, digest));
    if config_current {
        messages.push(
            "buildroot config unchanged since the last configuration; config steps skipped"
                .to_string(),
        );
    }
    if let Some(args) = defconfig_args.filter(|_| !config_current) {
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .args(&args)
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2 {
            command.env("BR2_EXTERNAL", br2_external);
        }
        messages.extend(run_command(
            command,
            "buildroot defconfig",
            command_context.execution,
            command_context.policy,
            command_context.log_sink.clone(),
            command_context.cancel_check.clone(),
        )?);
        if !config_fragments.is_empty() {
            messages.extend(apply_buildroot_config_fragments(
                spec,
                buildroot_dir,
                output_dir,
                config_fragments,
                br2,
                command_context.clone(),
            )?);
        }
        messages.extend(apply_buildroot_config_settings(BuildrootSettingsRequest {
            spec,
            output_dir,
            overrides: config_overrides,
            external_tree: br2,
            buildroot_dir,
            command: command_context.clone(),
            dry_run,
        })?);
    } else if !has_defconfig && (!config_fragments.is_empty() || !config_overrides.is_empty()) {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "buildroot config_fragments/config_overrides require defconfig or defconfig_path",
        ));
    }

    // The final `.config` edit: everything after this compares, records and
    // builds exactly this config.
    if buildroot_legacy_disabled(config_overrides) {
        disable_buildroot_legacy_flag(output_dir)?;
    }
    // Only a run that wrote the config steps' `.config` records their inputs;
    // a skipped run keeps the record it has.
    if let Some(digest) = config_inputs.as_deref().filter(|_| !config_current) {
        record_config_steps(output_dir, digest)?;
    }
    let step = if config_current {
        "config steps skipped"
    } else {
        "config steps"
    };
    let nested = messages[config_messages_from..].to_vec();
    messages.push(phase_step_message(step, &config_clock, &nested));
    Ok(ConfiguredTree {
        package_overrides,
        br2_external,
        messages,
    })
}

/// Compares a configured tree with the state its last build recorded.
pub(crate) fn tree_changes(
    output_dir: &Path,
    buildroot_dir: &Path,
    spec: &ResolvedBuildSpec,
    configured: &ConfiguredTree,
) -> TreeChanges {
    let package_overrides = &configured.package_overrides;
    let config_digest = buildroot_config_digest(output_dir);
    let accepted_config_digests = [
        buildroot_config_digest_v1(output_dir),
        buildroot_legacy_config_digest(output_dir),
    ];
    let replacement_clean_needed =
        package_overrides
            .replacement_digest
            .as_deref()
            .is_some_and(|replacement_digest| {
                [
                    Some(replacement_digest),
                    package_overrides.legacy_replacement_digest.as_deref(),
                ]
                .into_iter()
                .flatten()
                .all(|digest| {
                    buildroot_state_needs_clean(
                        output_dir,
                        ".gaia-buildroot-package-replacements-state",
                        digest,
                    )
                })
            });
    // Compare against the snapshot of the config the tree was built from when
    // there is one, naming the changed settings; trees from older Gaia
    // versions only have digests.
    let snapshot_changes = config_changes_since_snapshot(output_dir);
    let unattributed_config_change = snapshot_changes.is_none()
        && config_digest.as_deref().is_some_and(|config_digest| {
            buildroot_state_needs_clean(output_dir, ".gaia-buildroot-config-state", config_digest)
                && accepted_config_digests
                    .iter()
                    .flatten()
                    .all(|older_digest| {
                        buildroot_state_needs_clean(
                            output_dir,
                            ".gaia-buildroot-config-state",
                            older_digest,
                        )
                    })
        });
    let override_digests = package_override_digests(&buildroot_package_override_dirs(spec));
    let mut override_changes = match read_package_override_digests(output_dir) {
        Some(previous) => changed_override_packages(&previous, &override_digests),
        // Older state has one digest for all override trees: when it
        // changed, any override package may have.
        None if replacement_clean_needed => override_digests.keys().cloned().collect(),
        None => BTreeSet::new(),
    };
    // BR2_EXTERNAL files: the packages they touch are rebuilt. A changed file
    // that touches none is classified by what the configured tree reads it
    // through (see external_classify); only a `.mk` file is a full clean.
    let trees = external_trees_of(spec);
    let external_files = current_external_files(spec);
    let external = external_changes_since_build(output_dir, &external_files);
    let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
    let previous_graph = PackageGraph::load(output_dir);
    let known = previous_graph
        .as_ref()
        .map(|graph| graph.package_names().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let classified =
        classify_external_changes(&external.unmapped, &trees, &config, &known, buildroot_dir);
    override_changes.extend(external.packages);
    override_changes.extend(classified.packages);
    let mut external_reasons = external.reasons;
    external_reasons.extend(classified.reasons);
    TreeChanges {
        config_digest,
        config_changes: snapshot_changes.unwrap_or_default(),
        unattributed_config_change,
        override_digests,
        override_changes,
        external_files,
        external_unmapped: classified.unmapped,
        external_finalize: classified.finalize,
        external_reasons,
    }
}
