use super::*;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(crate) fn resolve_buildroot_dir(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Option<PathBuf> {
    if let Some(source_id) = buildroot_source_id(image) {
        let candidate = buildroot_source_dir(spec, source_id);
        if candidate.join("Makefile").is_file() {
            return Some(candidate);
        }
    }
    for key in ["GAIA_BUILDROOT_DIR", "BUILDROOT_DIR"] {
        if let Some(candidate) = env::var_os(key).map(PathBuf::from)
            && candidate.join("Makefile").is_file()
        {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn buildroot_source_id(image: &ImageSpec) -> Option<&SourceId> {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.source.as_ref(),
        _ => None,
    }
}

pub(crate) fn buildroot_allow_fallback(image: &ImageSpec) -> bool {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.allow_fallback,
        _ => false,
    }
}

pub(crate) fn buildroot_source_dir(spec: &ResolvedBuildSpec, source_id: &SourceId) -> PathBuf {
    Path::new(&spec.workspace.root_dir)
        .join(&spec.workspace.build_dir)
        .join("sources")
        .join(source_id.as_str())
}

#[derive(Clone)]
pub(crate) struct ImageCommandContext<'a> {
    pub(crate) execution: &'a ImageExecutionContext,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

pub(crate) struct BuildrootRunRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) command: ImageCommandContext<'a>,
}

/// Extra behavior for the final `make` of [`run_buildroot_with`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BuildrootMakeOptions<'a> {
    /// Appended to `BR2_ROOTFS_POST_BUILD_SCRIPT` on the make command line
    /// (the `.config` file is left untouched).
    pub(crate) post_build_script: Option<&'a Path>,
    /// The output dir is a shared tree: once its fakeroot scripts match the
    /// current config, only `make target-finalize` runs, because every build
    /// packs its own images.
    pub(crate) shared_tree: bool,
}

pub(crate) fn run_buildroot(
    request: BuildrootRunRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    run_buildroot_with(request, BuildrootMakeOptions::default())
}

pub(crate) fn run_buildroot_with(
    request: BuildrootRunRequest<'_>,
    options: BuildrootMakeOptions<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootRunRequest {
        spec,
        image,
        buildroot_dir,
        output_dir,
        command: command_context,
    } = request;
    fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot output dir '{}': {error}",
            output_dir.display()
        ))
    })?;
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

    if command_context.policy.parallel_packages
        && let Some(message) = apply_reflink_finalize(buildroot_dir, output_dir)?
    {
        messages.push(message);
    }
    let package_overrides =
        materialize_buildroot_package_overrides(spec, buildroot_dir, output_dir)?;
    if package_overrides.generated_external_tree.is_some() {
        ensure_no_generated_external_name_conflict(external_tree)?;
    }
    let br2_external = buildroot_external_tree_value(
        spec,
        external_tree,
        package_overrides
            .generated_external_tree
            .as_ref()
            .map(|generated| generated.path.as_path()),
    );
    let br2_external = br2_external.as_deref();
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

    if let Some(defconfig_path) = defconfig_path {
        let resolved_defconfig_path = resolve_workspace_path(
            &ResolvedBuildSpec {
                workspace: spec.workspace.clone(),
                ..spec.clone()
            },
            defconfig_path,
        )?;
        materialize_defconfig_support_files(&resolved_defconfig_path, output_dir)?;
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg("defconfig")
            .arg(format!(
                "BR2_DEFCONFIG={}",
                resolved_defconfig_path.display()
            ))
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2_external {
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
                br2_external,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree: br2_external,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
        messages.extend(apply_buildroot_cache_config(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            command_context.clone(),
        )?);
    } else if let Some(defconfig) = defconfig {
        let mut command = Command::new("make");
        command
            .arg(format!("O={}", output_dir.display()))
            .arg(defconfig)
            .current_dir(buildroot_dir);
        apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
        if let Some(br2_external) = br2_external {
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
                br2_external,
                command_context.clone(),
            )?);
        }
        if !config_overrides.is_empty() {
            messages.extend(apply_buildroot_config_overrides(
                BuildrootConfigOverrideRequest {
                    spec,
                    output_dir,
                    overrides: config_overrides,
                    external_tree: br2_external,
                    buildroot_dir,
                    command: command_context.clone(),
                },
            )?);
        }
        messages.extend(apply_buildroot_cache_config(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            command_context.clone(),
        )?);
    } else if !config_fragments.is_empty() || !config_overrides.is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "buildroot config_fragments/config_overrides require defconfig or defconfig_path",
        ));
    }

    // The final `.config` edit: everything below compares, records and builds
    // exactly this config.
    if buildroot_legacy_disabled(config_overrides) {
        disable_buildroot_legacy_flag(output_dir)?;
    }
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
    let config_changes = config_changes_since_snapshot(output_dir);
    let unattributed_config_change = config_changes.is_none()
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
    let config_changes = config_changes.unwrap_or_default();
    let override_digests = package_override_digests(&buildroot_package_override_dirs(spec));
    let override_changes = match read_package_override_digests(output_dir) {
        Some(previous) => changed_override_packages(&previous, &override_digests),
        // Older state has one digest for all override trees: when it
        // changed, any override package may have.
        None if replacement_clean_needed => override_digests.keys().cloned().collect(),
        None => BTreeSet::new(),
    };
    // Every config step is done: fail (or warn) about requested overrides
    // that olddefconfig dropped, before the clean and the long make.
    messages.extend(check_buildroot_config_overrides(
        spec,
        output_dir,
        config_overrides,
        command_context.policy.override_check,
    )?);

    // Finish deleting what an earlier clean moved aside.
    gaia_process::purge_trash(&output_dir.join(gaia_process::TRASH_DIR));
    // `make defconfig` already creates `build/`; only a build creates these.
    let built_before = ["target", "host", "per-package"]
        .iter()
        .any(|dir| output_dir.join(dir).is_dir());
    let something_changed = !config_changes.is_empty() || !override_changes.is_empty();
    let previous_graph = PackageGraph::load(output_dir);
    messages.extend(redo_interrupted_packages(
        output_dir,
        &[previous_graph.as_ref()],
    )?);
    let current_graph = if (built_before && something_changed) || previous_graph.is_none() {
        query_package_graph(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            &command_context,
        )?
    } else {
        None
    };
    let plan = if !built_before {
        CleanPlan::Nothing
    } else if unattributed_config_change {
        CleanPlan::Full(vec![
            "effective config changed (no snapshot of the previously built config)".to_string(),
        ])
    } else if !built_before || !something_changed {
        CleanPlan::Nothing
    } else if let Some(current) = &current_graph {
        let graphs = [Some(&current.graph), previous_graph.as_ref()];
        let symbols = SymbolIndex::load(buildroot_dir, br2_external, &graphs);
        plan_clean(CleanInputs {
            config_changes: &config_changes,
            override_changes: &override_changes,
            previous: previous_graph.as_ref(),
            current: &current.graph,
            symbol_use: &|key| symbols.symbol_use(key),
            per_package: command_context.policy.parallel_packages,
        })
    } else {
        let mut reasons = config_changes
            .iter()
            .map(|change| change.key.clone())
            .chain(
                override_changes
                    .iter()
                    .map(|name| format!("package override {name}")),
            )
            .collect::<Vec<_>>();
        reasons.push("Buildroot did not report its package graph".to_string());
        CleanPlan::Full(reasons)
    };
    match &plan {
        CleanPlan::Nothing => {}
        CleanPlan::Finalize {
            reasons,
            refresh_target,
        } => {
            for line in reasons {
                log_line(&command_context, line.clone());
            }
            if *refresh_target {
                discard_output_dirs(output_dir, &["target"])?;
            }
            messages.extend(reasons.iter().cloned());
        }
        CleanPlan::Full(reasons) => {
            // Move the big trees aside first: `make clean` deleting them
            // could hold the build up for hours on a busy disk.
            discard_output_dirs(
                output_dir,
                &[
                    "build",
                    "per-package",
                    "host",
                    "target",
                    "images",
                    "legal-info",
                    "graphs",
                ],
            )?;
            let mut command = Command::new("make");
            command
                .arg(format!("O={}", output_dir.display()))
                .arg("clean")
                .current_dir(buildroot_dir);
            apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
            if let Some(br2_external) = br2_external {
                command.env("BR2_EXTERNAL", br2_external);
            }
            let reasons = describe_clean_reasons(reasons);
            messages.extend(run_command(
                command,
                &format!("buildroot clean: {reasons}"),
                command_context.execution,
                command_context.policy,
                command_context.log_sink.clone(),
                command_context.cancel_check.clone(),
            )?);
            messages.push(format!("cleaned Buildroot output: {reasons}"));
        }
        CleanPlan::Packages(rebuild) => {
            let summary = rebuild_summary(rebuild);
            for line in std::iter::once(summary.clone()).chain(rebuild.reasons.iter().cloned()) {
                log_line(&command_context, line);
            }
            messages.extend(apply_package_rebuild(
                output_dir,
                rebuild,
                previous_graph.as_ref(),
                current_graph
                    .as_ref()
                    .map(|current| &current.graph)
                    .expect("a package plan comes from the current graph"),
            )?);
            if rebuild.refresh_target {
                discard_output_dirs(output_dir, &["target"])?;
            }
            messages.push(summary);
            messages.extend(rebuild.reasons.iter().cloned());
        }
    }

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .current_dir(buildroot_dir);
    append_make_jobs(&mut command, command_context.policy.local_jobs);
    if command_context.policy.parallel_packages {
        // Packages build concurrently, each with BR2_JLEVEL jobs: the load
        // limit (inherited by every package's make) keeps that from
        // oversubscribing the machine.
        command.arg(format!(
            "-l{}",
            make_jobs(command_context.policy.local_jobs)
        ));
    }
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(br2_external) = br2_external {
        command.env("BR2_EXTERNAL", br2_external);
    }
    let ccache_stats_log = output_dir.join(CCACHE_STATS_LOG);
    if command_context.policy.ccache_enabled {
        let _ = fs::remove_file(&ccache_stats_log);
        command.env("CCACHE_STATSLOG", &ccache_stats_log);
    }
    // Record the state the output tree now corresponds to before the long
    // make: if the build (or a post-image script, or a later assembly step)
    // fails, retrying with the same inputs must resume it rather than clean
    // everything again.
    if let Some(replacement_digest) = package_overrides.replacement_digest.as_deref() {
        write_buildroot_state(
            output_dir,
            ".gaia-buildroot-package-replacements-state",
            replacement_digest,
        )?;
    }
    if let Some(config_digest) = config_digest.as_deref() {
        write_buildroot_state(output_dir, ".gaia-buildroot-config-state", config_digest)?;
    }
    write_config_snapshot(output_dir)?;
    write_package_override_digests(output_dir, &override_digests)?;
    if let Some(current) = &current_graph {
        current.record(output_dir)?;
    }
    let cached_packages = restore_cached_packages(RestoreCachedPackages {
        spec,
        buildroot_dir,
        output_dir,
        br2_external,
        command_context: &command_context,
        graph: current_graph
            .as_ref()
            .map(|current| current.graph.clone())
            .or(previous_graph),
        messages: &mut messages,
    })?;
    if let Some(script) = options.post_build_script {
        command.arg(post_build_script_override(output_dir, script));
    }
    // With per-package directories, build only up to target-finalize first:
    // the filesystem images and post-image step run afterwards, and only
    // when what they read changed (see rootfs_inputs).
    let images_command =
        (command_context.policy.parallel_packages && !options.shared_tree).then(|| {
            let mut images = gaia_process::clone_command(&command);
            // Without finalizing (and building packages) again.
            images.args([
                "-o",
                "target-finalize",
                "-o",
                "host-finalize",
                "-o",
                "staging-finalize",
            ]);
            command.arg("target-finalize");
            images
        });
    let mut finalize_only = false;
    if options.shared_tree {
        let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
        let fs_types = shared_rootfs_types(&config)?;
        finalize_only = shared_pack_scripts_current(output_dir, &fs_types);
        if finalize_only {
            command.arg("target-finalize");
        } else {
            clear_shared_pack_state(output_dir)?;
        }
    }
    let make_started = std::time::SystemTime::now();
    mark_make_running(output_dir)?;
    let make = run_command(
        command,
        "buildroot make",
        command_context.execution,
        command_context.policy,
        command_context.log_sink.clone(),
        command_context.cancel_check.clone(),
    );
    messages.extend(
        finish_make(output_dir, make, cached_packages.as_ref())
            // Which packages a failed or interrupted make spent its time on.
            .map_err(|error| {
                error.with_step_times(buildroot_build_time_steps(
                    output_dir,
                    make_started,
                    std::time::SystemTime::now(),
                ))
            })?,
    );
    messages.extend(buildroot_build_time_steps(
        output_dir,
        make_started,
        std::time::SystemTime::now(),
    ));
    if let Some(images_command) = images_command {
        messages.extend(run_images_if_inputs_changed(
            images_command,
            image,
            buildroot_dir,
            output_dir,
            &command_context,
        )?);
    }
    if let Some(cached) = cached_packages {
        let started = std::time::Instant::now();
        messages.extend(cached.store(output_dir));
        messages.push(gaia_process::step_time_message(
            "package cache store",
            started.elapsed(),
        ));
    }
    if command_context.policy.ccache_enabled
        && let Some(stats) = fs::read_to_string(&ccache_stats_log)
            .ok()
            .and_then(|log| ccache_hit_rate(&log))
    {
        messages.push(format!("{SUMMARY_NOTE_PREFIX}{stats}"));
    }
    if options.shared_tree && !finalize_only {
        write_shared_pack_state(output_dir)?;
    }
    Ok(messages)
}

/// The reasons for a full clean, as named in its label and message, so an
/// unexpected clean can be traced to what caused it.
fn describe_clean_reasons(reasons: &[String]) -> String {
    const SHOWN: usize = 8;
    if reasons.len() > SHOWN {
        format!(
            "{} and {} more",
            reasons[..SHOWN].join(", "),
            reasons.len() - SHOWN
        )
    } else {
        reasons.join(", ")
    }
}

/// Settings that only say where to download or cache things, or how many
/// jobs to run. Changing them cannot change what is built, so they must not
/// trigger a full clean.
const BUILDROOT_NON_OUTPUT_SETTINGS: &[&str] = &[
    "BR2_DL_DIR",
    "BR2_CCACHE_DIR",
    "BR2_CCACHE_INITIAL_SETUP",
    "BR2_JLEVEL",
    "BR2_PRIMARY_SITE",
    "BR2_BACKUP_SITE",
    "BR2_KERNEL_MIRROR",
    "BR2_GNU_MIRROR",
    "BR2_LUAROCKS_MIRROR",
    "BR2_CPAN_MIRROR",
];

/// Squashfs tuning (compression, block size, padding). Root filesystem
/// images are regenerated from `target/` by every `make`, and the host
/// squashfs tools support every compressor, so switching XZ for zstd in a
/// development preset needs no clean.
fn is_squashfs_tuning_setting(key: &str) -> bool {
    key.starts_with("BR2_TARGET_ROOTFS_SQUASHFS") && key != "BR2_TARGET_ROOTFS_SQUASHFS"
}

/// Digest of the effective `.config` settings. Excluded: the generated header,
/// which names the Buildroot version (with a `-g<sha>` suffix for git trees),
/// the settings in [`BUILDROOT_NON_OUTPUT_SETTINGS`] and squashfs tuning.
/// "is not set" lines are kept.
pub(crate) fn buildroot_config_digest(output_dir: &Path) -> Option<String> {
    buildroot_settings_digest(output_dir, true).map(|hex| format!("settings-v2-sha256:{hex}"))
}

/// The digest written before squashfs tuning was excluded; still accepted so
/// an upgrade does not force a full Buildroot clean.
fn buildroot_config_digest_v1(output_dir: &Path) -> Option<String> {
    buildroot_settings_digest(output_dir, false).map(|hex| format!("settings-sha256:{hex}"))
}

fn buildroot_settings_digest(output_dir: &Path, skip_squashfs_tuning: bool) -> Option<String> {
    let contents = fs::read_to_string(output_dir.join(".config")).ok()?;
    let settings = contents
        .lines()
        .filter(|line| !is_buildroot_config_header(line))
        .filter(|line| {
            let key = line
                .trim_start_matches("# ")
                .split(['=', ' '])
                .next()
                .unwrap_or_default();
            let squashfs_tuning = skip_squashfs_tuning && is_squashfs_tuning_setting(key);
            !(BUILDROOT_NON_OUTPUT_SETTINGS.contains(&key) || squashfs_tuning)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut hasher = Sha256::new();
    hasher.update(settings.as_bytes());
    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

/// Whole-file digest written by earlier Gaia versions; still accepted so an
/// upgrade does not force a full Buildroot clean.
fn buildroot_legacy_config_digest(output_dir: &Path) -> Option<String> {
    let config_path = output_dir.join(".config");
    config_path
        .is_file()
        .then(|| file_sha256_or_placeholder(&config_path))
}

fn is_buildroot_config_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed == "#"
        || trimmed.starts_with("# Automatically generated file")
        || (trimmed.starts_with("# Buildroot ") && trimmed.ends_with(" Configuration"))
}

fn buildroot_state_needs_clean(output_dir: &Path, state_file: &str, digest: &str) -> bool {
    let state_path = output_dir.join(state_file);
    match fs::read_to_string(state_path) {
        Ok(state) => state.trim() != digest,
        Err(_) => buildroot_output_has_prior_build(output_dir),
    }
}

pub(crate) fn buildroot_legacy_disabled(overrides: &[(String, String)]) -> bool {
    overrides
        .iter()
        .any(|(key, value)| key == "BR2_LEGACY" && value.trim() == "n")
}

pub(crate) fn disable_buildroot_legacy_flag(output_dir: &Path) -> Result<(), ImageProviderError> {
    let config_path = output_dir.join(".config");
    if !config_path.is_file() {
        return Ok(());
    }
    let config = fs::read_to_string(&config_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to read Buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })?;
    if !config.lines().any(|line| line.trim() == "BR2_LEGACY=y") {
        return Ok(());
    }
    let rewritten = config
        .lines()
        .map(|line| {
            if line.trim() == "BR2_LEGACY=y" {
                "# BR2_LEGACY is not set"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&config_path, rewritten).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write Buildroot config '{}': {error}",
                config_path.display()
            ),
        )
    })
}

fn buildroot_output_has_prior_build(output_dir: &Path) -> bool {
    ["build", "target", "images"]
        .iter()
        .any(|entry| output_dir.join(entry).exists())
}

fn write_buildroot_state(
    output_dir: &Path,
    state_file: &str,
    digest: &str,
) -> Result<(), ImageProviderError> {
    fs::write(output_dir.join(state_file), format!("{digest}\n")).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write Buildroot state '{}' in '{}': {error}",
            state_file,
            output_dir.display()
        ))
    })
}

pub(crate) fn buildroot_state_digest(image: &ImageSpec, output_dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    output_dir
        .join(".config")
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&output_dir.join(".config")).hash(&mut hasher);
    image_feed_signature_path(output_dir)
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&image_feed_signature_path(output_dir)).hash(&mut hasher);
    if let ImageDefinition::Buildroot(buildroot) = &image.definition {
        for expected in &buildroot.expected_images {
            expected.name.hash(&mut hasher);
            expected.format.as_str().hash(&mut hasher);
            expected.required.hash(&mut hasher);
            file_state_for_digest(&output_dir.join("images").join(&expected.name))
                .hash(&mut hasher);
            file_state_for_digest(&output_dir.join(&expected.name)).hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

fn file_state_for_digest(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => format!(
            "file:{}:{}",
            metadata.len(),
            file_sha256_or_placeholder(path)
        ),
        Ok(metadata) if metadata.is_dir() => format!("dir:{}", metadata.len()),
        Ok(_) => "other".to_string(),
        Err(_) => "missing".to_string(),
    }
}
