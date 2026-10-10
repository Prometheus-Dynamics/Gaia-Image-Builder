//! Turning config and package override changes into the least Buildroot
//! clean that keeps the output tree correct.
//!
//! Buildroot never rebuilds a built package by itself, and never removes
//! what a package installed. So:
//! - Newly enabled packages just build: no clean.
//! - A built package whose options, override contents or version changed is
//!   uninstalled and rebuilt (its files are removed from `target/`,
//!   `staging/` and `host/`, then its build directory: `<pkg>-dirclean`),
//!   together with everything that depends on it, recursively.
//! - A package that is no longer built is uninstalled, and what depended on
//!   it is rebuilt.
//! - A built package that gained or lost a dependency (an optional feature
//!   now found, such as kmod with xz) is rebuilt alone.
//! - Toolchain, architecture, libc and other system-wide settings, and
//!   anything that cannot be attributed to a package, still clean the whole
//!   tree.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CleanPlan {
    Nothing,
    Packages(PackageRebuild),
    /// A full `make clean`, with the reasons.
    Full(Vec<String>),
    /// No package rebuilds: only target finalization reads what changed.
    Finalize {
        reasons: Vec<String>,
        /// Remove `target/` so finalization reassembles it from the
        /// per-package directories, dropping files of removed overlays.
        refresh_target: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageRebuild {
    /// Uninstalled and rebuilt.
    pub rebuild: BTreeSet<String>,
    /// Uninstalled only: no longer part of the config.
    pub removed: BTreeSet<String>,
    /// Why, one line per directly affected package.
    pub reasons: Vec<String>,
    /// Also reassemble `target/` (see [`CleanPlan::Finalize`]).
    pub refresh_target: bool,
}

pub(crate) struct CleanInputs<'a> {
    pub config_changes: &'a [ConfigChange],
    /// Package override directories that were added, removed or changed.
    pub override_changes: &'a BTreeSet<String>,
    /// The graph recorded with the last built config, when there is one.
    pub previous: Option<&'a PackageGraph>,
    pub current: &'a PackageGraph,
    /// What reads a setting that belongs to no package.
    pub symbol_use: &'a dyn Fn(&str) -> SymbolUse,
    /// Per-package directories are on, so `target/` can be reassembled.
    pub per_package: bool,
}

/// The `.config` symbols that belong to a package: its own and its options.
fn package_symbols(name: &str) -> Vec<String> {
    let upper = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    match name {
        "linux" => vec!["BR2_LINUX_KERNEL".to_string()],
        _ if name.starts_with("host-") => vec![format!("BR2_PACKAGE_{upper}")],
        _ => vec![
            format!("BR2_PACKAGE_{upper}"),
            format!("BR2_TARGET_{upper}"),
        ],
    }
}

/// The package a setting belongs to: the longest package symbol it is or
/// starts (`BR2_PACKAGE_LIBCAMERA_PIPELINE_RPI` is libcamera's).
/// `BR2_PACKAGE_HAS_<X>` and `BR2_PACKAGE_PROVIDES_<X>` belong to virtual
/// package `<x>`.
fn owning_package<'a>(key: &str, names: &BTreeSet<&'a str>) -> Option<&'a str> {
    let key = ["BR2_PACKAGE_HAS_", "BR2_PACKAGE_PROVIDES_"]
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix))
        .map(|virtual_package| format!("BR2_PACKAGE_{virtual_package}"))
        .unwrap_or_else(|| key.to_string());
    names
        .iter()
        .flat_map(|name| {
            package_symbols(name)
                .into_iter()
                .map(move |symbol| (symbol, *name))
        })
        .filter(|(symbol, _)| {
            key == *symbol
                || key
                    .strip_prefix(symbol.as_str())
                    .is_some_and(|rest| rest.starts_with('_'))
        })
        .max_by_key(|(symbol, _)| symbol.len())
        .map(|(_, name)| name)
}

/// Packages every other package is built with: changing them means
/// rebuilding everything.
fn is_toolchain(name: &str) -> bool {
    matches!(
        name,
        "toolchain"
            | "glibc"
            | "musl"
            | "uclibc"
            | "linux-headers"
            | "gcc-initial"
            | "gcc-final"
            | "host-gcc-initial"
            | "host-gcc-final"
            | "host-binutils"
    ) || name.starts_with("toolchain-")
}

fn change_summary(change: &ConfigChange) -> String {
    let value = |value: &Option<String>| value.clone().unwrap_or_else(|| "unset".to_string());
    format!(
        "{} {} -> {}",
        change.key,
        value(&change.previous),
        value(&change.current)
    )
}

pub(crate) fn plan_clean(inputs: CleanInputs<'_>) -> CleanPlan {
    let CleanInputs {
        config_changes,
        override_changes,
        previous,
        current,
        symbol_use,
        per_package,
    } = inputs;
    let mut names = current.package_names().collect::<BTreeSet<_>>();
    if let Some(previous) = previous {
        names.extend(previous.package_names());
    }

    // Packages that were not built before. Without the previous graph,
    // those whose own symbol was just enabled.
    let added = match previous {
        Some(previous) => current
            .package_names()
            .filter(|name| !previous.contains(name))
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        None => config_changes
            .iter()
            .filter(|change| change.enables())
            .filter_map(|change| {
                current
                    .package_names()
                    .find(|name| package_symbols(name).contains(&change.key))
            })
            .map(str::to_string)
            .collect(),
    };

    let mut full = Vec::new();
    let mut finalize = Vec::new();
    let mut refresh_target = false;
    let mut changed = BTreeMap::<String, Vec<String>>::new();
    let mut removed = BTreeMap::<String, Vec<String>>::new();
    let note = |set: &mut BTreeMap<String, Vec<String>>, name: &str, why: String| {
        set.entry(name.to_string()).or_default().push(why);
    };

    for change in config_changes {
        // Without the previous graph, an unset package symbol may have been
        // a package of its own (`BR2_PACKAGE_LIBCAMERA_APPS` looks like a
        // libcamera option once libcamera-apps is gone), whose files would
        // stay installed.
        if previous.is_none()
            && change.key.starts_with("BR2_PACKAGE_")
            && change.previous.is_some()
            && change.current.is_none()
        {
            full.push(format!(
                "{} (no record of what it built)",
                change_summary(change)
            ));
            continue;
        }
        match owning_package(&change.key, &names) {
            Some(name) if is_toolchain(name) => full.push(format!(
                "{} (toolchain package {name})",
                change_summary(change)
            )),
            Some(name) if !current.contains(name) => {
                note(&mut removed, name, change_summary(change));
            }
            Some(name) if added.contains(name) => {}
            Some(name) => note(&mut changed, name, change_summary(change)),
            // Not an option of any package built before or now: a menu or
            // helper symbol. What it changes for built packages shows up as
            // changed dependencies.
            None if change.key.starts_with("BR2_PACKAGE_") => {}
            None => {
                let summary = change_summary(change);
                let found = symbol_use(&change.key);
                if let Some(why) = found.global {
                    full.push(format!("{summary} ({why})"));
                    continue;
                }
                for name in &found.packages {
                    if is_toolchain(name) {
                        full.push(format!("{summary} (read by toolchain package {name})"));
                    } else if current.contains(name) && !added.contains(name) {
                        note(&mut changed, name, format!("{summary} (its .mk reads it)"));
                    }
                }
                if found.finalize && !per_package {
                    full.push(format!(
                        "{summary} (read when finalizing the target, which cannot be \
                         reassembled without per-package directories)"
                    ));
                } else if found.finalize {
                    refresh_target = true;
                    finalize.push(format!(
                        "{summary}: only read when finalizing the target, no package rebuilt"
                    ));
                } else if found.packages.is_empty() {
                    finalize.push(format!(
                        "{summary}: read by no package or Buildroot makefile, no package rebuilt"
                    ));
                }
            }
        }
    }

    for name in override_changes {
        let was_built = previous.map_or(!added.contains(name), |previous| previous.contains(name));
        if !was_built {
            continue;
        }
        if is_toolchain(name) {
            full.push(format!("package override {name} (toolchain package)"));
        } else if current.contains(name) {
            note(&mut changed, name, "package override changed".to_string());
        } else {
            note(&mut removed, name, "package override removed".to_string());
        }
    }

    // Version and dependency changes of packages built before and now.
    let mut gained = BTreeMap::<String, Vec<String>>::new();
    for name in current.package_names() {
        if added.contains(name) {
            continue;
        }
        let package = &current.packages[name];
        match previous.and_then(|previous| previous.get(name)) {
            Some(before) => {
                if before.version != package.version {
                    note(
                        &mut changed,
                        name,
                        format!(
                            "version {} -> {}",
                            before.version.as_deref().unwrap_or("none"),
                            package.version.as_deref().unwrap_or("none")
                        ),
                    );
                }
                if before.dependencies != package.dependencies {
                    let mut differences = package
                        .dependencies
                        .difference(&before.dependencies)
                        .map(|dependency| format!("+{dependency}"))
                        .collect::<Vec<_>>();
                    differences.extend(
                        before
                            .dependencies
                            .difference(&package.dependencies)
                            .map(|dependency| format!("-{dependency}")),
                    );
                    note(
                        &mut gained,
                        name,
                        format!("dependencies {}", differences.join(" ")),
                    );
                }
            }
            None => {
                let new = package
                    .dependencies
                    .intersection(&added)
                    .map(|dependency| format!("+{dependency}"))
                    .collect::<Vec<_>>();
                if !new.is_empty() {
                    note(&mut gained, name, format!("dependencies {}", new.join(" ")));
                }
            }
        }
    }

    if !full.is_empty() {
        return CleanPlan::Full(full);
    }

    // Everything depending on a changed or removed package, recursively.
    let reverse_dependencies = |name: &str| {
        let mut all = BTreeSet::new();
        for graph in [Some(current), previous].into_iter().flatten() {
            if let Some(package) = graph.get(name) {
                all.extend(package.reverse_dependencies.iter().cloned());
            }
        }
        all
    };
    let mut affected = BTreeSet::new();
    let mut queue = changed
        .keys()
        .chain(removed.keys())
        .cloned()
        .collect::<Vec<_>>();
    while let Some(name) = queue.pop() {
        for dependent in reverse_dependencies(&name) {
            if affected.insert(dependent.clone()) {
                queue.push(dependent);
            }
        }
    }
    let builds = |name: &str| {
        current
            .get(name)
            .is_some_and(|package| !package.is_virtual && !added.contains(name))
    };
    let rebuild = changed
        .keys()
        .chain(gained.keys())
        .cloned()
        .chain(affected)
        .filter(|name| builds(name))
        .collect::<BTreeSet<_>>();
    if let Some(name) = rebuild.iter().find(|name| is_toolchain(name)) {
        return CleanPlan::Full(vec![format!("toolchain package {name} must be rebuilt")]);
    }
    let removed_packages = removed
        .keys()
        .filter(|name| {
            previous
                .and_then(|previous| previous.get(name))
                .is_some_and(|package| !package.is_virtual)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if rebuild.is_empty() && removed_packages.is_empty() {
        return if finalize.is_empty() {
            CleanPlan::Nothing
        } else {
            CleanPlan::Finalize {
                reasons: finalize,
                refresh_target,
            }
        };
    }

    let mut reasons = Vec::new();
    for (label, set) in [("changed", &changed), ("removed", &removed), ("", &gained)] {
        for (name, why) in set {
            let prefix = if label.is_empty() {
                name.clone()
            } else {
                format!("{name} {label}")
            };
            reasons.push(format!("{prefix}: {}", why.join(", ")));
        }
    }
    let direct = changed
        .keys()
        .chain(gained.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let dependents = rebuild.difference(&direct).cloned().collect::<Vec<_>>();
    if !dependents.is_empty() {
        reasons.push(format!("dependent packages: {}", dependents.join(", ")));
    }
    reasons.extend(finalize);
    CleanPlan::Packages(PackageRebuild {
        rebuild,
        removed: removed_packages,
        reasons,
        refresh_target,
    })
}

/// Installed-file lists Buildroot keeps per package, by the directory they
/// describe.
const FILE_LISTS: &[(&str, &str)] = &[
    (".files-list.txt", "target"),
    (".files-list-staging.txt", "staging"),
    (".files-list-host.txt", "host"),
];

/// Everything [`apply_package_rebuild`] removes, read without removing
/// anything: installed files that only the rebuilt packages installed, and
/// their build and per-package directories (which may not exist).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageRebuildRemovals {
    /// Installed files in `target/`, `staging/` or `host/` that exist now.
    pub(crate) files: Vec<PathBuf>,
    pub(crate) dirs: Vec<PathBuf>,
}

/// The removals of [`apply_package_rebuild`] for `plan`.
pub(crate) fn package_rebuild_removals(
    output_dir: &Path,
    plan: &PackageRebuild,
    previous: Option<&PackageGraph>,
    current: &PackageGraph,
) -> PackageRebuildRemovals {
    let targets = plan
        .rebuild
        .iter()
        .chain(&plan.removed)
        .collect::<BTreeSet<_>>();

    // Who installed what, from every package's lists.
    let mut owners = BTreeMap::<(usize, String), BTreeSet<String>>::new();
    if let Ok(entries) = fs::read_dir(output_dir.join("build")) {
        for entry in entries.filter_map(Result::ok) {
            for (index, (list, _)) in FILE_LISTS.iter().enumerate() {
                let Ok(contents) = fs::read_to_string(entry.path().join(list)) else {
                    continue;
                };
                for line in contents.lines() {
                    if let Some((package, path)) = line.split_once(',') {
                        owners
                            .entry((index, path.to_string()))
                            .or_default()
                            .insert(package.to_string());
                    }
                }
            }
        }
    }
    let mut removals = PackageRebuildRemovals::default();
    for ((index, path), packages) in &owners {
        let ours = packages.iter().all(|package| targets.contains(package));
        let relative = Path::new(path);
        let safe = relative
            .components()
            .all(|component| matches!(component, Component::CurDir | Component::Normal(_)));
        if !ours || !safe {
            continue;
        }
        let file = output_dir.join(FILE_LISTS[*index].1).join(relative);
        if fs::symlink_metadata(&file).is_ok_and(|metadata| !metadata.is_dir()) {
            removals.files.push(file);
        }
    }

    let per_package = output_dir.join("per-package");
    for name in &targets {
        let stamp_dirs = [previous, Some(current)]
            .into_iter()
            .flatten()
            .filter_map(|graph| graph.get(name)?.stamp_dir.clone())
            .collect::<BTreeSet<_>>();
        for stamp_dir in stamp_dirs {
            let relative = Path::new(&stamp_dir);
            if !relative.starts_with("build")
                || !relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                continue;
            }
            removals.dirs.push(output_dir.join(relative));
        }
        removals.dirs.push(per_package.join(name));
    }
    removals
}

/// Uninstalls the packages (removing the files they installed that no other
/// package also installed) and removes their build directories, so the
/// next `make` builds them again from scratch.
pub(crate) fn apply_package_rebuild(
    output_dir: &Path,
    plan: &PackageRebuild,
    previous: Option<&PackageGraph>,
    current: &PackageGraph,
) -> Result<Vec<String>, ImageProviderError> {
    let removals = package_rebuild_removals(output_dir, plan, previous, current);
    let removed_files = removals
        .files
        .iter()
        .filter(|file| fs::remove_file(file).is_ok())
        .count();
    for dir in &removals.dirs {
        remove_dir_if_present(dir)?;
    }
    let packages = plan.rebuild.union(&plan.removed).count();
    Ok(vec![format!(
        "uninstalled {packages} Buildroot package(s) ({removed_files} installed file(s)) for rebuild"
    )])
}

pub(crate) fn remove_dir_if_present(dir: &Path) -> Result<(), ImageProviderError> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ImageProviderError::backend_command(format!(
            "failed to remove Buildroot build directory '{}': {error}",
            dir.display()
        ))),
    }
}

#[cfg(test)]
#[path = "clean_plan_tests.rs"]
mod tests;

/// One line naming what a package rebuild uninstalls and rebuilds.
pub(crate) fn rebuild_summary(rebuild: &PackageRebuild) -> String {
    let join = |names: &BTreeSet<String>| names.iter().cloned().collect::<Vec<_>>().join(", ");
    format!(
        "buildroot rebuild of {} package(s){}: {}",
        rebuild.rebuild.len(),
        if rebuild.removed.is_empty() {
            String::new()
        } else {
            format!(", uninstall of {}", join(&rebuild.removed))
        },
        join(&rebuild.rebuild)
    )
}

/// What a full clean moves aside first: the big trees of the output dir.
pub(crate) const FULL_CLEAN_DIRS: &[&str] = &[
    "build",
    "per-package",
    "host",
    "target",
    "images",
    "legal-info",
    "graphs",
];

/// Removes output tree directories at once (see [`gaia_process::discard`]).
pub(crate) fn discard_output_dirs(
    output_dir: &Path,
    dirs: &[&str],
) -> Result<(), ImageProviderError> {
    for dir in dirs {
        let path = output_dir.join(dir);
        gaia_process::discard(&path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to remove '{}': {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}
