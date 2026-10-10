//! The package cache and compiler cache around the main Buildroot `make`:
//! restoring cached packages before it, storing what it built, and the
//! ccache hit rate of the run.
use super::*;
use crate::requested_rebuilds::{excluded_from_restore, requested_package_rebuilds};

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
    /// Whether this make restored any package from the cache.
    pub(crate) fn restored_any(&self) -> bool {
        !self.restored.is_empty()
    }

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
        // Installed packages without a key (local sources, for example) can
        // never be stored: say so rather than leave them out silently.
        let uncached = self
            .graph
            .package_names()
            .filter(|name| {
                matches!(self.keys.get(*name), Some(None))
                    && stamp_built(output_dir, &self.graph, name)
            })
            .collect::<Vec<_>>();
        if !uncached.is_empty() {
            messages.push(format!(
                "package cache: {} installed package(s) have no cache key (for example local \
                 sources), so they are not cached: {}",
                uncached.len(),
                uncached.join(", ")
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
    let external = external_package_key_digests(spec);
    let keys = package_keys(&KeyInputs {
        buildroot_dir,
        output_dir,
        graph: &graph,
        execution_identity: &identity,
        external: &external,
    });
    let keep_source = packages_reading_sources(spec, &graph);
    for (name, build_dir) in source_less_build_dirs(output_dir, &graph, &keep_source) {
        let _ = gaia_process::discard(&build_dir);
        let _ = gaia_process::discard(&output_dir.join("per-package").join(&name));
        tracing::info!(
            provider_domain = "image.buildroot",
            "building {name} again: its sources are read by the image and the cache does not hold them"
        );
    }
    // `--rebuild-package` packages are built again, never restored.
    let requested = requested_package_rebuilds(command_context.policy, Some(&graph))?;
    let restored = cache.restore_except(
        output_dir,
        &graph,
        &keys,
        &excluded_from_restore(&keep_source, &requested),
    );
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

/// Packages whose build directory the build reads later (an assembly
/// source under `buildroot-output/build/<dir>/`): a cache entry holds a
/// package's installed files, not its sources, so these are built instead
/// of restored. Pure: see [`source_less_build_dirs`] for the build
/// directories a run removes before it restores.
pub(crate) fn packages_reading_sources(
    spec: &ResolvedBuildSpec,
    graph: &PackageGraph,
) -> BTreeSet<String> {
    let dirs = referenced_build_dirs(&format!("{:?}", spec.image));
    graph
        .packages
        .iter()
        .filter(|(_, package)| {
            package
                .stamp_dir
                .as_deref()
                .and_then(|stamp_dir| stamp_dir.strip_prefix("build/"))
                .is_some_and(|dir| {
                    dirs.iter()
                        .any(|pattern| gaia_spec::wildcard_match(pattern, dir))
                })
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Of `packages` (see [`packages_reading_sources`]), those installed in
/// `output_dir` whose build directory has no sources left, with that build
/// directory. A source-less build directory left by an earlier restore is
/// removed before the restore, so Buildroot builds the package again.
pub(crate) fn source_less_build_dirs(
    output_dir: &Path,
    graph: &PackageGraph,
    packages: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    packages
        .iter()
        .filter_map(|name| {
            let stamp_dir = graph.get(name)?.stamp_dir.as_deref()?;
            let build_dir = output_dir.join(stamp_dir);
            let has_sources = fs::read_dir(&build_dir)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
            (build_dir.join(".stamp_installed").exists() && !has_sources)
                .then(|| (name.clone(), build_dir))
        })
        .collect()
}

/// `<dir>` (possibly a glob) of every `buildroot-output/build/<dir>/` or
/// `buildroot_output/build/<dir>/` (the `$provider.buildroot_output`
/// variable of assembly sources) in `text`.
pub(crate) fn referenced_build_dirs(text: &str) -> BTreeSet<String> {
    ["buildroot-output/build/", "buildroot_output/build/"]
        .iter()
        .flat_map(|marker| {
            text.match_indices(marker)
                .map(move |(start, _)| start + marker.len())
        })
        .filter_map(|start| {
            let rest = &text[start..];
            let end = rest.find('/')?;
            let dir = &rest[..end];
            (!dir.is_empty() && !dir.contains(['"', ' ', '\\'])).then(|| dir.to_string())
        })
        .collect()
}

#[cfg(test)]
mod read_sources_tests {
    use super::*;

    #[test]
    fn build_dirs_read_by_the_image_are_found() {
        let text = r#"source: "/w/build/image/buildroot-output/build/rpi-firmware-1.2/boot/start4.elf", other: "/w/build/image/buildroot-output/images/Image", x: "buildroot-output/build/linux-custom/arch/arm64/boot/dts/x.dtb", y: "$provider.buildroot_output/build/rpi-eeprom-*/firmware/x.bin""#;
        assert_eq!(
            referenced_build_dirs(text),
            BTreeSet::from([
                "rpi-firmware-1.2".to_string(),
                "linux-custom".to_string(),
                "rpi-eeprom-*".to_string()
            ])
        );
    }
}
