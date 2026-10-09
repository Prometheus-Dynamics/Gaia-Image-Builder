//! The package cache and compiler cache around the main Buildroot `make`:
//! restoring cached packages before it, storing what it built, and the
//! ccache hit rate of the run.
use super::*;

pub(crate) struct RestoreCachedPackages<'a, 'b> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) br2_external: Option<&'a str>,
    pub(crate) command_context: &'a ImageCommandContext<'b>,
    pub(crate) graph: Option<PackageGraph>,
    pub(crate) messages: &'a mut Vec<String>,
}

/// The package cache's work for one `make`: what was restored before it,
/// and the keys to store what it built under.
pub(crate) struct CachedPackages {
    cache: PackageCache,
    graph: PackageGraph,
    keys: BTreeMap<String, Option<String>>,
    restored: Vec<String>,
}

impl CachedPackages {
    pub(crate) fn store(&self, output_dir: &Path) -> Vec<String> {
        let (stored, skipped) =
            self.cache
                .store(output_dir, &self.graph, &self.keys, &self.restored);
        let mut messages = Vec::new();
        if !stored.is_empty() {
            messages.push(format!(
                "stored {} Buildroot package(s) in the package cache: {}",
                stored.len(),
                stored.join(", ")
            ));
        }
        for reason in &skipped {
            tracing::info!(
                provider_domain = "image.buildroot",
                "package cache: not stored: {reason}"
            );
        }
        if !skipped.is_empty() {
            messages.push(format!(
                "package cache: {} package(s) not stored: {}",
                skipped.len(),
                skipped.join("; ")
            ));
        }
        messages.push(format!(
            "{SUMMARY_NOTE_PREFIX}buildroot package cache: {} restored, {} stored",
            self.restored.len(),
            stored.len()
        ));
        messages
    }
}

/// Restores the packages this tree still has to build from the package
/// cache, when it is on.
pub(crate) fn restore_cached_packages(
    request: RestoreCachedPackages<'_, '_>,
) -> Result<Option<CachedPackages>, ImageProviderError> {
    let RestoreCachedPackages {
        spec,
        buildroot_dir,
        output_dir,
        br2_external,
        command_context,
        graph,
        messages,
    } = request;
    let Some(cache) = package_cache(spec, command_context.policy, output_dir)? else {
        if command_context.policy.package_cache_enabled {
            messages.push(
                "package cache: off, it needs [providers.buildroot] parallel_packages = true"
                    .to_string(),
            );
        }
        return Ok(None);
    };
    let graph = match graph {
        Some(graph) => graph,
        None => match query_package_graph(
            spec,
            buildroot_dir,
            output_dir,
            br2_external,
            command_context,
        )? {
            Some(queried) => {
                queried.record(output_dir)?;
                queried.graph
            }
            None => {
                messages.push(
                    "package cache: off for this run, Buildroot did not report its package graph"
                        .to_string(),
                );
                return Ok(None);
            }
        },
    };
    messages.extend(cache.space_warning());
    messages.extend(cache.note.clone());
    let started = std::time::Instant::now();
    let identity = execution_identity(command_context.execution);
    let keys = package_keys(&KeyInputs {
        buildroot_dir,
        output_dir,
        graph: &graph,
        execution_identity: &identity,
    });
    let restored = cache.restore(output_dir, &graph, &keys);
    refresh_current_stamps(output_dir, &graph, &keys);
    messages.push(gaia_process::step_time_message(
        "package cache keys and restore",
        started.elapsed(),
    ));
    if !restored.is_empty() {
        let line = format!(
            "restored {} Buildroot package(s) from the package cache: {}",
            restored.len(),
            restored.join(", ")
        );
        tracing::info!(provider_domain = "image.buildroot", "{line}");
        if let Some(log_sink) = &command_context.log_sink {
            log_sink(gaia_process::ProcessLogLine {
                stream: gaia_process::ProcessLogStream::Stderr,
                line: line.clone(),
            });
        }
        messages.push(line);
    }
    Ok(Some(CachedPackages {
        cache,
        graph,
        keys,
        restored,
    }))
}

/// ccache's per-compilation statistics for one `make`, which only that
/// make's compilations write, however many builds share the cache.
pub(crate) const CCACHE_STATS_LOG: &str = ".gaia-ccache-stats.log";

/// Prefix of the run messages the provider moves into
/// [`ImageExecutionResult::notes`] for the run summary.
pub(crate) const SUMMARY_NOTE_PREFIX: &str = "summary: ";

/// `buildroot ccache: <hits>/<cacheable> compilations from cache (<n>%)`
/// from a ccache stats log, or `None` when nothing was compiled.
pub(crate) fn ccache_hit_rate(log: &str) -> Option<String> {
    let count = |counter: &str| log.lines().filter(|line| line.trim() == counter).count();
    let hits = count("direct_cache_hit") + count("preprocessed_cache_hit");
    let cacheable = hits + count("cache_miss");
    (cacheable > 0).then(|| {
        format!(
            "buildroot ccache: {hits}/{cacheable} compilations from cache ({:.1}%)",
            hits as f64 * 100.0 / cacheable as f64
        )
    })
}
