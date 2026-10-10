use super::*;
use crate::requested_rebuilds::{requested_package_rebuilds, with_requested_rebuilds};
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
    /// Stop after `target-finalize`: the prepare operation only readies the
    /// target tree; the build operation, which adds the image feed and so
    /// changes the tree, makes the filesystem images.
    pub(crate) finalize_only: bool,
}

/// The prepare operation: packages and target finalization, no images.
pub(crate) fn run_buildroot(
    request: BuildrootRunRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    run_buildroot_with(
        request,
        BuildrootMakeOptions {
            finalize_only: true,
            ..BuildrootMakeOptions::default()
        },
    )
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
    let config_overrides = match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.config_overrides.as_slice(),
        _ => &[][..],
    };

    if command_context.policy.parallel_packages {
        let clock = gaia_process::ActiveClock::start();
        messages.extend(apply_reflink_finalize(buildroot_dir, output_dir)?);
        messages.push(phase_step_message("reflink finalize", &clock, &[]));
        let clock = gaia_process::ActiveClock::start();
        messages.extend(apply_host_finalize_skip(buildroot_dir)?);
        messages.push(phase_step_message("host finalize patch", &clock, &[]));
    }
    let configured = configure_tree(
        spec,
        image,
        buildroot_dir,
        output_dir,
        &command_context,
        false,
    )?;
    messages.extend(configured.messages.iter().cloned());
    let br2_external = configured.br2_external.as_deref();
    let package_overrides = &configured.package_overrides;
    let clock = gaia_process::ActiveClock::start();
    let mut changes = tree_changes(output_dir, spec, &configured);
    messages.push(phase_step_message("tree changes", &clock, &[]));
    for line in &changes.external_reasons {
        log_line(&command_context, line.clone());
        messages.push(line.clone());
    }
    let config_digest = changes.config_digest.clone();
    let override_digests = changes.override_digests.clone();
    let clock = gaia_process::ActiveClock::start();
    let host_tools = apply_host_tools(output_dir, &command_context)?;
    messages.push(phase_step_message("host tools probe", &clock, &[]));
    messages.extend(host_tools.messages);
    changes.override_changes.extend(host_tools.changed_packages);
    // Every config step is done: fail (or warn) about requested overrides
    // that olddefconfig dropped, before the clean and the long make.
    messages.extend(timed_phase("config override check", || {
        check_buildroot_config_overrides(
            spec,
            output_dir,
            config_overrides,
            command_context.policy.override_check,
        )
    })?);

    // Finish deleting what an earlier clean moved aside.
    let clock = gaia_process::ActiveClock::start();
    gaia_process::purge_trash(&output_dir.join(gaia_process::TRASH_DIR));
    messages.push(phase_step_message("trash purge", &clock, &[]));
    // `make defconfig` already creates `build/`; only a build creates these.
    let built_before = ["target", "host", "per-package"]
        .iter()
        .any(|dir| output_dir.join(dir).is_dir());
    // Whether this run changes what `host-finalize` copies: the make rule
    // skips the copy after a finalize that left the marker, so the marker
    // goes before the make when this is set. A killed make (its marker is
    // still there) may have left the copy half done.
    let mut host_tree_changed = output_dir.join(MAKE_RUNNING).is_file();
    let clock = gaia_process::ActiveClock::start();
    let previous_graph = PackageGraph::load(output_dir);
    messages.push(phase_step_message("package graph load", &clock, &[]));
    messages.extend(timed_phase("interrupted packages", || {
        redo_interrupted_packages(output_dir, &[previous_graph.as_ref()])
    })?);
    // `--rebuild-package` needs the graph to name packages and to clean them.
    let current_graph = if needs_current_graph(built_before, &changes, previous_graph.is_none())
        || !command_context.policy.rebuild_packages.is_empty()
    {
        let clock = gaia_process::ActiveClock::start();
        let queried = query_package_graph(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            &command_context,
        )?;
        messages.push(phase_step_message("package graph show-info", &clock, &[]));
        queried
    } else {
        None
    };
    let clock = gaia_process::ActiveClock::start();
    let symbols = current_graph.as_ref().map(|current| {
        SymbolIndex::load(
            buildroot_dir,
            br2_external,
            &[Some(&current.graph), previous_graph.as_ref()],
        )
    });
    messages.push(phase_step_message("symbol index", &clock, &[]));
    let symbol_use = |key: &str| {
        symbols
            .as_ref()
            .map(|symbols| symbols.symbol_use(key))
            .unwrap_or_default()
    };
    let clock = gaia_process::ActiveClock::start();
    let plan = decide_clean(CleanDecisionInput {
        built_before,
        changes: &changes,
        previous: previous_graph.as_ref(),
        current: current_graph.as_ref().map(|current| &current.graph),
        symbol_use: &symbol_use,
        per_package: command_context.policy.parallel_packages,
    });
    messages.push(phase_step_message("clean planning", &clock, &[]));
    let requested = requested_package_rebuilds(
        command_context.policy,
        current_graph.as_ref().map(|current| &current.graph),
    )?;
    let plan = with_requested_rebuilds(plan, &requested);
    host_tree_changed |= matches!(plan, CleanPlan::Full(_) | CleanPlan::Packages(_));
    let clean_clock = gaia_process::ActiveClock::start();
    let clean_messages_from = messages.len();
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
            discard_output_dirs(output_dir, FULL_CLEAN_DIRS)?;
            invalidate_host_finalized(output_dir);
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
    if !matches!(plan, CleanPlan::Nothing) {
        let nested = messages[clean_messages_from..].to_vec();
        messages.push(phase_step_message(
            "clean application",
            &clean_clock,
            &nested,
        ));
    }

    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .current_dir(buildroot_dir);
    // No load limit (`-l`): it counts every process on the machine, so on a
    // machine busy with other work it held every package's make to one job
    // at a time. Concurrent packages each running BR2_JLEVEL jobs are left to
    // the scheduler.
    append_make_jobs(&mut command, command_context.policy.local_jobs);
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
    let clock = gaia_process::ActiveClock::start();
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
    write_external_files_state(output_dir, &changes.external_files)?;
    if let Some(current) = &current_graph {
        current.record(output_dir)?;
    }
    messages.push(phase_step_message("build state records", &clock, &[]));
    // Before the restore: a tree whose every package is installed, with
    // nothing to clean, makes nothing but its finalize (see finalize_state).
    let make_was_running = output_dir.join(MAKE_RUNNING).is_file();
    let nothing_to_build_before_restore = matches!(plan, CleanPlan::Nothing)
        && !make_was_running
        && current_graph
            .as_ref()
            .map(|current| &current.graph)
            .or(previous_graph.as_ref())
            .is_some_and(|graph| all_packages_installed(output_dir, graph));
    let clock = gaia_process::ActiveClock::start();
    let restore_messages_from = messages.len();
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
    let nested = messages[restore_messages_from..].to_vec();
    messages.push(phase_step_message("package cache setup", &clock, &nested));
    // Restored files and stamps carry the cache's modification times.
    host_tree_changed |= cached_packages
        .as_ref()
        .is_some_and(CachedPackages::restored_any);
    if let Some(script) = options.post_build_script {
        command.arg(post_build_script_override(output_dir, script));
    }
    // With per-package directories, build only up to target-finalize first:
    // the filesystem images and post-image step run afterwards, and only
    // when what they read changed (see rootfs_inputs).
    // The split build: a first make that finalizes, then the images make.
    let split_images =
        command_context.policy.parallel_packages && !options.shared_tree && !options.finalize_only;
    let restored_any = cached_packages
        .as_ref()
        .is_some_and(CachedPackages::restored_any);
    let nothing_to_build = nothing_to_build_before_restore && !restored_any;
    // The marker survives only a make that finalizes a tree building nothing.
    let keeps_marker =
        nothing_to_build && !options.shared_tree && (options.finalize_only || split_images);
    if !keeps_marker {
        invalidate_finalized(output_dir);
    }
    // Post-build scripts (the image feed) run inside target-finalize: a
    // build with one must finalize, or the feed would not be applied.
    let skip_finalize = split_images
        && keeps_marker
        && options.post_build_script.is_none()
        && config_digest
            .as_deref()
            .is_some_and(|digest| finalized_for(output_dir, digest));
    if skip_finalize {
        command.args(["-o", "target-finalize"]);
        messages.push(
            "skipped target-finalize: the tree was finalized after its last build and nothing \
             has been built since"
                .to_string(),
        );
    }
    if options.finalize_only {
        command.arg("target-finalize");
    }
    let images_command = split_images.then(|| {
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
    if host_tree_changed {
        invalidate_host_finalized(output_dir);
    }
    let make_started = std::time::SystemTime::now();
    mark_make_running(output_dir)?;
    let progress = MakeProgress::start(output_dir, command_context.log_sink.clone());
    let make = run_command(
        command,
        "buildroot make",
        command_context.execution,
        command_context.policy,
        command_context.log_sink.clone(),
        command_context.cancel_check.clone(),
    );
    drop(progress);
    let clock = gaia_process::ActiveClock::start();
    let finished = finish_make(output_dir, make, cached_packages.as_ref())
        // Which packages a failed or interrupted make spent its time on.
        .map_err(|error| {
            error.with_step_times(buildroot_build_time_steps(
                output_dir,
                make_started,
                std::time::SystemTime::now(),
            ))
        })?;
    messages.push(phase_step_message("make finish", &clock, &finished));
    messages.extend(finished);
    // A make that stopped after the finalize (or ran it first) left the tree
    // finalized for this config.
    if (options.finalize_only || split_images)
        && !options.shared_tree
        && let Some(digest) = config_digest.as_deref()
    {
        record_finalized(output_dir, digest);
    }
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
pub(crate) fn buildroot_config_digest_v1(output_dir: &Path) -> Option<String> {
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
pub(crate) fn buildroot_legacy_config_digest(output_dir: &Path) -> Option<String> {
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

pub(crate) fn buildroot_state_needs_clean(
    output_dir: &Path,
    state_file: &str,
    digest: &str,
) -> bool {
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
    buildroot_state_digest_with(image, output_dir, &BTreeMap::new())
}

/// [`buildroot_state_digest`], given the digests `collected` already holds
/// for expected images by name (see `collect_expected_images_hashed`): the
/// image the collect copied is not read again.
pub(crate) fn buildroot_state_digest_with(
    image: &ImageSpec,
    output_dir: &Path,
    collected: &BTreeMap<String, String>,
) -> String {
    let mut hasher = DefaultHasher::new();
    output_dir
        .join(".config")
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&output_dir.join(".config"), None).hash(&mut hasher);
    image_feed_signature_path(output_dir)
        .display()
        .to_string()
        .hash(&mut hasher);
    file_state_for_digest(&image_feed_signature_path(output_dir), None).hash(&mut hasher);
    if let ImageDefinition::Buildroot(buildroot) = &image.definition {
        for expected in &buildroot.expected_images {
            expected.name.hash(&mut hasher);
            expected.format.as_str().hash(&mut hasher);
            expected.required.hash(&mut hasher);
            let images_path = output_dir.join("images").join(&expected.name);
            let root_path = output_dir.join(&expected.name);
            // Collection copies the first of these that exists.
            let copied = if images_path.exists() {
                Some(&images_path)
            } else if root_path.exists() {
                Some(&root_path)
            } else {
                None
            };
            for path in [&images_path, &root_path] {
                let known = collected
                    .get(&expected.name)
                    .filter(|_| copied == Some(path))
                    .map(String::as_str);
                file_state_for_digest(path, known).hash(&mut hasher);
            }
        }
    }
    format!("{:016x}", hasher.finish())
}

fn file_state_for_digest(path: &Path, known_digest: Option<&str>) -> String {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => format!(
            "file:{}:{}",
            metadata.len(),
            known_digest
                .map(str::to_string)
                .unwrap_or_else(|| file_sha256_or_placeholder(path))
        ),
        Ok(metadata) if metadata.is_dir() => format!("dir:{}", metadata.len()),
        Ok(_) => "other".to_string(),
        Err(_) => "missing".to_string(),
    }
}
