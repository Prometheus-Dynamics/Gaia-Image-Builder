//! Buildroot's package graph, from `make show-info`: which packages the
//! config builds, their versions, build directories and (reverse)
//! dependencies. The graph of each built config is kept next to its config
//! snapshot so the next config change can be turned into package rebuilds.
use super::*;

const PACKAGE_GRAPH_STATE: &str = ".gaia-buildroot-packages.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageInfo {
    /// `target`, `host`, `rootfs`, ...
    pub kind: String,
    pub is_virtual: bool,
    pub version: Option<String>,
    /// Build and stamp directory, relative to the output directory.
    pub stamp_dir: Option<String>,
    /// Where its `.mk` lives: relative to the Buildroot source, or absolute.
    pub package_dir: Option<String>,
    /// Names of the files it downloads (tarballs, extra downloads).
    pub sources: Vec<String>,
    /// Its `.hash` files and the patches applied to it.
    pub hash_files: Vec<String>,
    pub patches: Vec<String>,
    pub dependencies: BTreeSet<String>,
    pub reverse_dependencies: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageGraph {
    pub packages: BTreeMap<String, PackageInfo>,
}

impl PackageGraph {
    /// The graph in `make show-info` output, or `None` when there is none or
    /// the Buildroot version does not report reverse dependencies.
    pub(crate) fn parse(output: &str) -> Option<Self> {
        let json = output
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with('{'))?;
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let strings = |entry: &serde_json::Value, field: &str| -> Option<BTreeSet<String>> {
            entry.get(field)?.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
        };
        let text = |entry: &serde_json::Value, field: &str| {
            entry
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let mut packages = BTreeMap::new();
        for (name, entry) in value.as_object()? {
            let kind = text(entry, "type").unwrap_or_default();
            let info = if kind == "rootfs" {
                PackageInfo {
                    kind,
                    dependencies: strings(entry, "dependencies").unwrap_or_default(),
                    ..PackageInfo::default()
                }
            } else {
                PackageInfo {
                    kind,
                    is_virtual: entry
                        .get("virtual")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    version: text(entry, "version"),
                    stamp_dir: text(entry, "stamp_dir"),
                    package_dir: text(entry, "package_dir"),
                    sources: entry
                        .get("downloads")
                        .and_then(serde_json::Value::as_array)
                        .map(|downloads| {
                            downloads
                                .iter()
                                .filter_map(|download| text(download, "source"))
                                .collect()
                        })
                        .unwrap_or_default(),
                    hash_files: strings(entry, "hashes")
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    patches: strings(entry, "patches")
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    dependencies: strings(entry, "dependencies")?,
                    reverse_dependencies: strings(entry, "reverse_dependencies")?,
                }
            };
            packages.insert(name.clone(), info);
        }
        Some(Self { packages })
    }

    /// The graph recorded with the last built config.
    pub(crate) fn load(output_dir: &Path) -> Option<Self> {
        Self::parse(&fs::read_to_string(output_dir.join(PACKAGE_GRAPH_STATE)).ok()?)
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.packages
            .get(name)
            .is_some_and(|package| package.kind != "rootfs")
    }

    pub(crate) fn get(&self, name: &str) -> Option<&PackageInfo> {
        self.packages
            .get(name)
            .filter(|package| package.kind != "rootfs")
    }

    /// Packages (not filesystem images) in the graph.
    pub(crate) fn package_names(&self) -> impl Iterator<Item = &str> {
        self.packages
            .iter()
            .filter(|(_, package)| package.kind != "rootfs")
            .map(|(name, _)| name.as_str())
    }
}

/// The current config's package graph and the raw `show-info` output to
/// record with it, or `None` when Buildroot could not report it.
pub(crate) struct QueriedPackageGraph {
    pub graph: PackageGraph,
    raw: String,
}

impl QueriedPackageGraph {
    pub(crate) fn record(&self, output_dir: &Path) -> Result<(), ImageProviderError> {
        fs::write(output_dir.join(PACKAGE_GRAPH_STATE), &self.raw).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to record Buildroot package graph in '{}': {error}",
                output_dir.display()
            ))
        })
    }
}

pub(crate) fn query_package_graph(
    spec: &ResolvedBuildSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    br2_external: Option<&str>,
    command_context: &ImageCommandContext<'_>,
) -> Result<Option<QueriedPackageGraph>, ImageProviderError> {
    let mut command = Command::new("make");
    command
        .arg("-s")
        .arg("--no-print-directory")
        .arg(format!("O={}", output_dir.display()))
        .arg("show-info")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(br2_external) = br2_external {
        command.env("BR2_EXTERNAL", br2_external);
    }
    let output_path = output_dir.join(".gaia-buildroot-show-info.tmp");
    let started = std::time::Instant::now();
    let output = command_stdout_to_file_with_timeout(CommandStdoutToFileRequest {
        command: &mut command,
        output_path: &output_path,
        execution: command_context.execution,
        timeout: Duration::from_secs(command_context.policy.timeout_seconds.max(1)),
        label: "buildroot show-info",
        retention: command_context.policy.output_retention,
        // The graph is one very long JSON line; it is recorded, not logged.
        log_sink: None,
        cancel_check: command_context.cancel_check.clone(),
    });
    tracing::info!(
        provider_domain = "image.buildroot",
        elapsed_ms = started.elapsed().as_millis(),
        "buildroot show-info"
    );
    let raw = fs::read_to_string(&output_path).unwrap_or_default();
    let _ = fs::remove_file(&output_path);
    match output {
        Ok(output) if output.status.success() => {
            Ok(PackageGraph::parse(&raw).map(|graph| QueriedPackageGraph { graph, raw }))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind == ImageProviderErrorKind::Cancelled => Err(error),
        Err(_) => Ok(None),
    }
}
