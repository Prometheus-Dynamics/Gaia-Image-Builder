//! `gaia preview` for a Buildroot image: what running it would do, without
//! changing the output tree.
//!
//! The run's own steps are repeated on a scratch copy: the state files of the
//! tree (`.config`, the `.gaia-*` records, `local.mk`) are copied into a
//! temporary directory, and the config steps, host tool decisions, clean
//! decision and package graph run there ([`configure_tree`],
//! [`decide_clean`], ...). What the run would delete or move is then read
//! from the real tree, and the package cache is planned read-only.
use super::*;
use crate::requested_rebuilds::{
    excluded_from_restore, requested_package_rebuilds, with_requested_rebuilds,
};
use gaia_image_providers::{
    ImagePreview, PreviewCleanKind, PreviewDeletion, PreviewDeletionKind, PreviewSection,
};

const PROVIDER_ID: &str = "image.buildroot";
/// Largest state file copied into the scratch tree.
const SCRATCH_STATE_LIMIT: u64 = 64 * 1024 * 1024;
/// Names listed in full in a verdict before the rest are counted.
const VERDICT_NAMES: usize = 3;

/// A scratch copy of an output tree's state, removed when dropped.
struct Scratch {
    root: PathBuf,
    /// The tree the steps run in, `<root>/buildroot-output`.
    tree: PathBuf,
}

impl Scratch {
    /// Creates a scratch tree holding copies of the state files of
    /// `state_dir` (none for a tree that does not exist yet).
    fn create(state_dir: Option<&Path>) -> Result<Self, ImageProviderError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("gaia-preview-{}-{nanos}", std::process::id()));
        let tree = root.join("buildroot-output");
        let scratch = Self { root, tree };
        fs::create_dir_all(&scratch.tree).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create the preview scratch dir '{}': {error}",
                scratch.tree.display()
            ))
        })?;
        if let Some(state_dir) = state_dir {
            copy_state_files(state_dir, &scratch.tree)?;
        }
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Copies the small state files of a tree: its config, `local.mk`, the
/// external tree record, and the `.gaia-*` records (not the trees under them).
fn copy_state_files(from: &Path, to: &Path) -> Result<(), ImageProviderError> {
    let entries = fs::read_dir(from).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read '{}' for the preview: {error}",
            from.display()
        ))
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let state = name == ".config"
            || name == "local.mk"
            || name.starts_with(".br2-external")
            || (name.starts_with(".gaia-") && !name.ends_with(".tmp"));
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !state || !metadata.is_file() || metadata.len() > SCRATCH_STATE_LIMIT {
            continue;
        }
        fs::copy(entry.path(), to.join(&name)).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to copy '{}' for the preview: {error}",
                entry.path().display()
            ))
        })?;
    }
    Ok(())
}

/// Where the state the run would compare against lives: the existing tree,
/// or nowhere when the run starts from an empty tree (a fresh RAM tree, a
/// disk tree moved to RAM, a RAM tree dropped for disk).
fn recorded_state_dir(work: &WorkDirDecision, output_dir: &Path) -> Option<PathBuf> {
    match work {
        WorkDirDecision::Disk { dropped: None }
        | WorkDirDecision::RamFallback { dropped: None, .. } => {
            output_dir.is_dir().then(|| output_dir.to_path_buf())
        }
        WorkDirDecision::Tree(placement) if !placement.create && !placement.moved_from_disk => {
            Some(placement.dir.clone())
        }
        _ => None,
    }
}

fn has_built_dirs(tree: &Path) -> bool {
    ["target", "host", "per-package"]
        .iter()
        .any(|dir| tree.join(dir).is_dir())
}

fn blocked(mut preview: ImagePreview, message: impl Into<String>) -> ImagePreview {
    let message = message.into();
    preview.verdict = format!("BLOCKED: {message}");
    preview.blocked = Some(message);
    preview
}

fn deletion(kind: PreviewDeletionKind, path: &Path, reason: impl Into<String>) -> PreviewDeletion {
    // File lists name installed files as `./usr/bin/x`: show them plainly.
    let path = path
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect::<PathBuf>();
    PreviewDeletion {
        kind,
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

/// Up to [`VERDICT_NAMES`] names, then the count of the rest.
fn name_list(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(VERDICT_NAMES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > VERDICT_NAMES {
        format!("{shown}, … ({} in all)", names.len())
    } else {
        shown
    }
}

/// `name old -> new` for a config change, as the clean reasons name them.
fn config_change_line(change: &ConfigChange) -> String {
    let value = |value: &Option<String>| value.clone().unwrap_or_else(|| "unset".to_string());
    format!(
        "{} {} -> {}",
        change.key,
        value(&change.previous),
        value(&change.current)
    )
}

/// The preview of a Buildroot image operation.
pub(crate) fn preview_buildroot(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    policy: &ImageExecutionPolicy,
) -> Result<ImagePreview, ImageProviderError> {
    let mut preview = ImagePreview {
        provider_id: PROVIDER_ID.into(),
        sections: Vec::new(),
        clean: PreviewCleanKind::Nothing,
        clean_reasons: Vec::new(),
        rebuilt_packages: Vec::new(),
        uninstalled_packages: Vec::new(),
        deletions: Vec::new(),
        blocked: None,
        verdict: String::new(),
    };
    let Some(buildroot_dir) = resolve_buildroot_dir(spec, image) else {
        return Ok(blocked(
            preview,
            "no Buildroot source (its Makefile is not found): the run would fail or use the \
             fallback rootfs",
        ));
    };
    if policy.shared_output {
        return Ok(blocked(
            preview,
            "shared Buildroot output trees are not previewed",
        ));
    }
    let output_dir = buildroot_output_dir(spec);
    let config_overrides = match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.config_overrides.as_slice(),
        _ => &[][..],
    };

    // Where the tree goes, and what that discards.
    let facts = work_dir_facts(&output_dir, policy)?;
    let work = decide_work_dir(&facts);
    let tree = work.tree(&output_dir);
    let state_dir = recorded_state_dir(&work, &output_dir);
    let mut work_lines = work_dir_messages(&work, &output_dir);
    match &work {
        WorkDirDecision::Disk { dropped } | WorkDirDecision::RamFallback { dropped, .. } => {
            if let Some(target) = dropped {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Tree,
                    target,
                    "RAM tree of an earlier build, discarded to build on disk",
                ));
            }
        }
        WorkDirDecision::Tree(placement) => {
            if placement.moved_from_disk {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Tree,
                    &output_dir,
                    "disk tree moved to RAM: discarded, packages restored from the package cache",
                ));
            } else if !placement.create {
                let size = tree_size(&placement.dir);
                work_lines.insert(
                    0,
                    format!(
                        "keeps the existing tree '{}' ({:.1} GiB)",
                        placement.dir.display(),
                        size as f64 / (1024.0 * 1024.0 * 1024.0)
                    ),
                );
            }
        }
    }
    if work_lines.is_empty() {
        work_lines.push(format!("building in the build dir '{}'", tree.display()));
    }
    if work.is_ram() {
        work_lines.push(if policy.work_dir.keep_ram_tree {
            "kept after the build (keep_ram_tree = true)".to_string()
        } else {
            preview.deletions.push(deletion(
                PreviewDeletionKind::Tree,
                &tree,
                "RAM tree dropped after the build (keep_ram_tree = false)",
            ));
            "dropped after the build (keep_ram_tree = false)".to_string()
        });
    }
    preview.sections.push(PreviewSection {
        title: "work dir".into(),
        lines: work_lines,
    });

    // The config steps and host tools, on a scratch copy of the state.
    let execution = execution_context(spec);
    let command = ImageCommandContext {
        execution: &execution,
        policy,
        log_sink: None,
        cancel_check: None,
    };
    let scratch = Scratch::create(state_dir.as_deref())?;
    gaia_process::register_docker_mount(&scratch.root);
    let configured =
        match configure_tree(spec, image, &buildroot_dir, &scratch.tree, &command, true) {
            Ok(configured) => configured,
            Err(error) => return Ok(blocked(preview, error.message)),
        };
    // Step times are for the run's progress, not for the report.
    let mut config_lines = configured
        .messages
        .iter()
        .filter(|message| !message.starts_with(gaia_process::STEP_TIME_PREFIX))
        .cloned()
        .collect::<Vec<_>>();
    let mut changes = tree_changes(&scratch.tree, &buildroot_dir, spec, &configured);
    let config = fs::read_to_string(scratch.tree.join(".config")).unwrap_or_default();
    let previous_decisions =
        fs::read_to_string(scratch.tree.join(DECISIONS_FILE)).unwrap_or_default();
    let host = match decide_host_tools_probed(&config, &previous_decisions, &command) {
        Ok(host) => host,
        Err(error) => return Ok(blocked(preview, error.message)),
    };
    write_host_tools(&scratch.tree, &host)?;
    changes
        .override_changes
        .extend(host.changed_packages.iter().cloned());
    match check_buildroot_config_overrides(
        spec,
        &scratch.tree,
        config_overrides,
        policy.override_check,
    ) {
        Ok(messages) => config_lines.extend(messages),
        Err(error) => return Ok(blocked(preview, error.message)),
    }
    for change in &changes.config_changes {
        config_lines.push(format!("changed: {}", config_change_line(change)));
    }
    if changes.unattributed_config_change {
        config_lines
            .push("the config changed and no snapshot names what: the tree is cleaned".to_string());
    }
    preview.sections.push(PreviewSection {
        title: "config".into(),
        lines: config_lines,
    });
    let host_lines = host
        .messages
        .iter()
        .cloned()
        .chain(std::iter::once(format!(
            "decisions: {}",
            if host.decisions.is_empty() {
                "none (no host tool is configured)"
            } else {
                &host.decisions
            }
        )))
        .collect();
    preview.sections.push(PreviewSection {
        title: "host tools".into(),
        lines: host_lines,
    });

    // The clean. Its state is the recorded one, read from the real tree.
    let built_before = state_dir.as_deref().is_some_and(has_built_dirs);
    let previous_graph = PackageGraph::load(&scratch.tree);
    let mut clean_lines = Vec::new();
    if let Some(dir) = &state_dir {
        let interrupted = interrupted_builds(dir, &[previous_graph.as_ref()]);
        if !interrupted.is_empty() {
            let names = interrupted
                .iter()
                .map(|build| build.name.clone())
                .collect::<Vec<_>>();
            clean_lines.push(format!(
                "an interrupted make left {} package(s) in progress; a run builds them again \
                 first: {}",
                names.len(),
                name_list(&names)
            ));
        }
    }
    let current = if needs_current_graph(built_before, &changes, previous_graph.is_none())
        || !policy.rebuild_packages.is_empty()
    {
        match query_package_graph(
            spec,
            &buildroot_dir,
            &scratch.tree,
            configured.br2_external.as_deref(),
            &command,
        ) {
            Ok(graph) => graph,
            Err(error) => return Ok(blocked(preview, error.message)),
        }
    } else {
        None
    };
    let symbols = current.as_ref().map(|current| {
        SymbolIndex::load(
            &buildroot_dir,
            configured.br2_external.as_deref(),
            &[Some(&current.graph), previous_graph.as_ref()],
        )
    });
    let symbol_use = |key: &str| {
        symbols
            .as_ref()
            .map(|symbols| symbols.symbol_use(key))
            .unwrap_or_default()
    };
    let clean = decide_clean(CleanDecisionInput {
        built_before,
        changes: &changes,
        previous: previous_graph.as_ref(),
        current: current.as_ref().map(|current| &current.graph),
        symbol_use: &symbol_use,
        per_package: policy.parallel_packages,
    });
    let requested =
        requested_package_rebuilds(policy, current.as_ref().map(|current| &current.graph))?;
    let clean = with_requested_rebuilds(clean, &requested);
    let graph = current
        .as_ref()
        .map(|current| &current.graph)
        .or(previous_graph.as_ref());
    // Changed external tree files, named whatever the clean is.
    clean_lines.extend(changes.external_reasons.iter().cloned());
    let (kind, reasons) = match &clean {
        CleanPlan::Nothing => (PreviewCleanKind::Nothing, Vec::new()),
        CleanPlan::Finalize { reasons, .. } => (PreviewCleanKind::Finalize, reasons.clone()),
        CleanPlan::Packages(rebuild) => (PreviewCleanKind::Packages, rebuild.reasons.clone()),
        CleanPlan::Full(reasons) => (PreviewCleanKind::Full, reasons.clone()),
    };
    preview.clean = kind;
    preview.clean_reasons = reasons.clone();
    match &clean {
        CleanPlan::Nothing => clean_lines.push(if built_before {
            "nothing to clean: no setting or package override changed".to_string()
        } else {
            "the tree has not been built: nothing to clean".to_string()
        }),
        CleanPlan::Finalize {
            reasons,
            refresh_target,
        } => {
            clean_lines.extend(reasons.iter().cloned());
            if *refresh_target {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Tree,
                    &tree.join("target"),
                    "target/ reassembled from the per-package directories",
                ));
            }
        }
        CleanPlan::Full(reasons) => {
            clean_lines.push(format!("full clean: {}", reasons.join(", ")));
            for dir in FULL_CLEAN_DIRS {
                let path = tree.join(dir);
                if path.exists() {
                    preview.deletions.push(deletion(
                        PreviewDeletionKind::Tree,
                        &path,
                        "full clean (moved aside, then removed)",
                    ));
                }
            }
        }
        CleanPlan::Packages(rebuild) => {
            clean_lines.push(format!(
                "rebuild {} package(s): {}",
                rebuild.rebuild.len(),
                rebuild
                    .rebuild
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if !rebuild.removed.is_empty() {
                clean_lines.push(format!(
                    "uninstall {} package(s): {}",
                    rebuild.removed.len(),
                    rebuild
                        .removed
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            clean_lines.extend(rebuild.reasons.iter().cloned());
            let current_graph = current
                .as_ref()
                .map(|current| &current.graph)
                .expect("a package plan comes from the current graph");
            let removals =
                package_rebuild_removals(&tree, rebuild, previous_graph.as_ref(), current_graph);
            for file in removals.files {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Package,
                    &file,
                    "installed file of a package rebuilt",
                ));
            }
            for dir in removals.dirs.iter().filter(|dir| dir.exists()) {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Package,
                    dir,
                    "build or per-package directory of a package rebuilt",
                ));
            }
            if rebuild.refresh_target {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Tree,
                    &tree.join("target"),
                    "target/ reassembled from the per-package directories",
                ));
            }
            preview.rebuilt_packages = rebuild.rebuild.iter().cloned().collect();
            preview.uninstalled_packages = rebuild.removed.iter().cloned().collect();
        }
    }
    let trash = output_dir.join(gaia_process::TRASH_DIR);
    for entry in fs::read_dir(&trash).into_iter().flatten().flatten() {
        preview.deletions.push(deletion(
            PreviewDeletionKind::Trash,
            &entry.path(),
            "left by an earlier clean, purged in the background",
        ));
    }

    // The package cache: what the build would restore, build, and evict.
    let mut cache_lines = Vec::new();
    let cache_probe = if tree.is_dir() {
        tree.clone()
    } else {
        output_dir.parent().unwrap_or(&output_dir).to_path_buf()
    };
    let cache = package_cache(spec, policy, &cache_probe)?;
    match (&cache, graph) {
        (None, _) => cache_lines.push(if policy.package_cache.enabled {
            "off: it needs [providers.buildroot] parallel_packages = true".to_string()
        } else {
            "off: [providers.buildroot.package_cache] is disabled".to_string()
        }),
        (Some(_), None) => cache_lines
            .push("off for this run: Buildroot did not report its package graph".to_string()),
        (Some(cache), Some(graph)) => {
            let mut identity = execution_identity(&execution);
            let system_tools = system_host_tools(&host.decisions);
            if !system_tools.is_empty() {
                identity = format!("{identity} {system_tools}");
            }
            let external = external_package_key_digests(spec);
            let keys = package_keys(&KeyInputs {
                buildroot_dir: &buildroot_dir,
                output_dir: &scratch.tree,
                graph,
                execution_identity: &identity,
                external: &external,
            });
            let read_by_image = packages_reading_sources(spec, graph);
            let rebuilt = match &clean {
                CleanPlan::Packages(rebuild) => rebuild
                    .rebuild
                    .union(&rebuild.removed)
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                _ => BTreeSet::new(),
            };
            let full = matches!(clean, CleanPlan::Full(_));
            let built_after = |name: &str| {
                !full
                    && !rebuilt.contains(name)
                    && state_dir
                        .as_deref()
                        .is_some_and(|dir| stamp_built(dir, graph, name))
            };
            let mut restored = cache.restore_plan(
                &tree,
                graph,
                &keys,
                &excluded_from_restore(&read_by_image, &requested),
                &built_after,
            );
            restored.sort();
            let mut reused = Vec::new();
            let mut built = Vec::new();
            for (name, package) in &graph.packages {
                if package.kind == "rootfs" || package.is_virtual {
                    continue;
                }
                if restored.contains(name) {
                    continue;
                }
                if built_after(name) {
                    reused.push(name.clone());
                } else {
                    built.push(name.clone());
                }
            }
            if !restored.is_empty() {
                cache_lines.push(format!(
                    "restored from the cache ({}): {}",
                    restored.len(),
                    name_list(&restored)
                ));
            }
            if !built.is_empty() {
                cache_lines.push(format!("built ({}): {}", built.len(), name_list(&built)));
            }
            if !read_by_image.is_empty() {
                let mut names = read_by_image.iter().cloned().collect::<Vec<_>>();
                names.sort();
                cache_lines.push(format!(
                    "built, not restored: the image reads their sources: {}",
                    name_list(&names)
                ));
            }
            cache_lines.push(format!("reused in place ({})", reused.len()));
            for (name, build_dir) in source_less_build_dirs(&tree, graph, &read_by_image) {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Package,
                    &build_dir,
                    format!(
                        "{name} built again: the image reads its sources, which the cache does not hold"
                    ),
                ));
            }
            for eviction in cache.preview_evictions() {
                preview.deletions.push(deletion(
                    PreviewDeletionKind::Cache,
                    &eviction.path,
                    format!(
                        "package cache entry evicted beyond its size ({} bytes)",
                        eviction.size
                    ),
                ));
            }
        }
    }
    preview.sections.push(PreviewSection {
        title: "clean".into(),
        lines: clean_lines,
    });
    preview.sections.push(PreviewSection {
        title: "package cache".into(),
        lines: cache_lines,
    });

    let outside_trash = preview.deletions_outside_trash();
    preview.verdict = match kind {
        PreviewCleanKind::Full => {
            let mut reason = reasons
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            if reasons.len() > 2 {
                reason.push_str(&format!("; and {} more", reasons.len() - 2));
            }
            format!("FULL CLEAN of {} (reason: {reason})", tree.display())
        }
        _ => {
            let rebuilt = &preview.rebuilt_packages;
            let uninstalled = if preview.uninstalled_packages.is_empty() {
                String::new()
            } else {
                format!(", {} uninstalled", preview.uninstalled_packages.len())
            };
            let scope = if kind == PreviewCleanKind::Finalize {
                " (finalize only)"
            } else {
                ""
            };
            format!(
                "no clean{scope}, {} packages rebuilt{} ({}), {outside_trash} deleted paths",
                rebuilt.len(),
                uninstalled,
                if rebuilt.is_empty() {
                    "none".to_string()
                } else {
                    name_list(rebuilt)
                },
            )
        }
    };
    Ok(preview)
}
