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
        if command_context.policy.package_cache.enabled {
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
    // Host tools taken from the system are part of every key.
    let mut identity = execution_identity(command_context.execution);
    let system_tools = recorded_host_tools(output_dir);
    if !system_tools.is_empty() {
        identity = format!("{identity} {system_tools}");
    }
    let keys = package_keys(&KeyInputs {
        buildroot_dir,
        output_dir,
        graph: &graph,
        execution_identity: &identity,
    });
    let restored = cache.restore(output_dir, &graph, &keys);
    refresh_current_stamps(output_dir, &graph, &keys);
    pin_restored_linux_version(output_dir, &graph)?;
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

const LINUX_PIN_BEGIN: &str = "# BEGIN gaia restored linux (generated; do not edit)";
const LINUX_PIN_END: &str = "# END gaia restored linux";

/// A `linux` restored from the package cache has no kernel source tree, so
/// Buildroot cannot ask it for the kernel release (`LINUX_VERSION_PROBED`,
/// `make kernelrelease`), and target finalization would run `depmod` for the
/// build machine's kernel. While the tree is missing, the release is pinned
/// in `local.mk` from the modules directory the package installed.
fn pin_restored_linux_version(
    output_dir: &Path,
    graph: &PackageGraph,
) -> Result<(), ImageProviderError> {
    let pin = restored_linux_release(output_dir, graph)
        .map(|release| format!("override LINUX_VERSION_PROBED = {release}\n"))
        .unwrap_or_default();
    write_local_mk_section(output_dir, LINUX_PIN_BEGIN, LINUX_PIN_END, &pin)
}

/// The kernel release of an installed `linux` whose build dir has no kernel
/// source.
fn restored_linux_release(output_dir: &Path, graph: &PackageGraph) -> Option<String> {
    let build_dir = output_dir.join(graph.get("linux")?.stamp_dir.as_ref()?);
    if !build_dir.join(".stamp_installed").exists() || build_dir.join("Makefile").exists() {
        return None;
    }
    let mut releases = fs::read_dir(output_dir.join("per-package/linux/target/lib/modules"))
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect::<Vec<_>>();
    (releases.len() == 1).then(|| releases.remove(0))
}

#[cfg(test)]
mod linux_pin_tests {
    use super::*;

    #[test]
    fn a_restored_kernel_without_its_source_pins_its_release() {
        let output = std::env::temp_dir().join(format!("gaia-linux-pin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);
        let build = output.join("build/linux-custom");
        fs::create_dir_all(&build).expect("build");
        fs::write(build.join(".stamp_installed"), "").expect("stamp");
        fs::create_dir_all(output.join("per-package/linux/target/lib/modules/6.12.25-v8-16k"))
            .expect("modules");
        let mut graph = PackageGraph::default();
        graph.packages.insert(
            "linux".to_string(),
            PackageInfo {
                stamp_dir: Some("build/linux-custom".to_string()),
                ..PackageInfo::default()
            },
        );
        pin_restored_linux_version(&output, &graph).expect("pin");
        let local = fs::read_to_string(output.join("local.mk")).expect("local.mk");
        assert!(
            local.contains("override LINUX_VERSION_PROBED = 6.12.25-v8-16k\n"),
            "{local}"
        );
        // Built again from source: the pin goes.
        fs::write(build.join("Makefile"), "").expect("kernel source");
        pin_restored_linux_version(&output, &graph).expect("unpin");
        assert!(!output.join("local.mk").exists());
        let _ = fs::remove_dir_all(output);
    }
}
