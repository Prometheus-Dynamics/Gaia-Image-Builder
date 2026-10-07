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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageRebuild {
    /// Uninstalled and rebuilt.
    pub rebuild: BTreeSet<String>,
    /// Uninstalled only: no longer part of the config.
    pub removed: BTreeSet<String>,
    /// Why, one line per directly affected package.
    pub reasons: Vec<String>,
}

pub(crate) struct CleanInputs<'a> {
    pub config_changes: &'a [ConfigChange],
    /// Package override directories that were added, removed or changed.
    pub override_changes: &'a BTreeSet<String>,
    /// The graph recorded with the last built config, when there is one.
    pub previous: Option<&'a PackageGraph>,
    pub current: &'a PackageGraph,
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
            None => full.push(format!(
                "{} (toolchain, architecture or system setting)",
                change_summary(change)
            )),
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
        return CleanPlan::Nothing;
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
    CleanPlan::Packages(PackageRebuild {
        rebuild,
        removed: removed_packages,
        reasons,
    })
}

/// Installed-file lists Buildroot keeps per package, by the directory they
/// describe.
const FILE_LISTS: &[(&str, &str)] = &[
    (".files-list.txt", "target"),
    (".files-list-staging.txt", "staging"),
    (".files-list-host.txt", "host"),
];

/// Uninstalls the packages (removing the files they installed that no other
/// package also installed) and removes their build directories, so the
/// next `make` builds them again from scratch.
pub(crate) fn apply_package_rebuild(
    output_dir: &Path,
    plan: &PackageRebuild,
    previous: Option<&PackageGraph>,
    current: &PackageGraph,
) -> Result<Vec<String>, ImageProviderError> {
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
    let mut removed_files = 0usize;
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
        if fs::symlink_metadata(&file).is_ok_and(|metadata| !metadata.is_dir())
            && fs::remove_file(&file).is_ok()
        {
            removed_files += 1;
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
            remove_dir_if_present(&output_dir.join(relative))?;
        }
        remove_dir_if_present(&per_package.join(name))?;
    }
    Ok(vec![format!(
        "uninstalled {} Buildroot package(s) ({removed_files} installed file(s)) for rebuild",
        targets.len()
    )])
}

fn remove_dir_if_present(dir: &Path) -> Result<(), ImageProviderError> {
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
mod tests {
    use super::*;

    /// A graph from `(name, dependencies)`, with reverse dependencies
    /// derived as Buildroot reports them.
    fn graph(packages: &[(&str, &[&str])]) -> PackageGraph {
        let mut graph = PackageGraph::default();
        for (name, dependencies) in packages {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    kind: if name.starts_with("host-") {
                        "host"
                    } else {
                        "target"
                    }
                    .to_string(),
                    version: Some("1".to_string()),
                    stamp_dir: Some(format!("build/{name}-1")),
                    dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
                    ..PackageInfo::default()
                },
            );
        }
        for (name, dependencies) in packages {
            for dependency in *dependencies {
                if let Some(package) = graph.packages.get_mut(*dependency) {
                    package.reverse_dependencies.insert(name.to_string());
                }
            }
        }
        graph
    }

    fn change(key: &str, previous: Option<&str>, current: Option<&str>) -> ConfigChange {
        ConfigChange {
            key: key.to_string(),
            previous: previous.map(str::to_string),
            current: current.map(str::to_string),
        }
    }

    fn plan(
        changes: &[ConfigChange],
        overrides: &[&str],
        previous: Option<&PackageGraph>,
        current: &PackageGraph,
    ) -> CleanPlan {
        plan_clean(CleanInputs {
            config_changes: changes,
            override_changes: &overrides.iter().map(|name| name.to_string()).collect(),
            previous,
            current,
        })
    }

    fn rebuilds(plan: CleanPlan) -> Vec<String> {
        match plan {
            CleanPlan::Packages(rebuild) => rebuild.rebuild.into_iter().collect(),
            other => panic!("expected a package rebuild, got {other:?}"),
        }
    }

    const BASE: &[(&str, &[&str])] = &[
        ("toolchain", &[]),
        ("host-xz", &[]),
        ("libcamera", &["toolchain"]),
        ("gstreamer1", &["toolchain"]),
        ("libcamera-apps", &["libcamera"]),
        ("photonvision", &["libcamera-apps", "gstreamer1"]),
        ("kmod", &["toolchain"]),
        ("mesa3d", &["toolchain"]),
    ];

    #[test]
    fn new_packages_build_without_a_clean() {
        let before = graph(BASE);
        let mut after = BASE.to_vec();
        after.push(("xz", &["toolchain"]));
        after.push(("pd-image-slots", &["toolchain", "xz"]));
        let after = graph(&after);
        let changes = [
            change("BR2_PACKAGE_XZ", None, Some("y")),
            change("BR2_PACKAGE_PD_IMAGE_SLOTS", None, Some("y")),
            change("BR2_PACKAGE_PD_IMAGE_SLOTS_SLOTS", None, Some("2")),
        ];
        // pd-image-slots is a new package in an override tree.
        for previous in [Some(&before), None] {
            assert_eq!(
                plan(&changes, &["pd-image-slots"], previous, &after),
                CleanPlan::Nothing
            );
        }
    }

    #[test]
    fn packages_that_gain_a_new_dependency_rebuild_alone() {
        let before = graph(BASE);
        let mut after = BASE.to_vec();
        after.push(("xz", &["toolchain"]));
        after.retain(|(name, _)| *name != "kmod");
        after.push(("kmod", &["toolchain", "xz"]));
        let after = graph(&after);
        let changes = [change("BR2_PACKAGE_XZ", None, Some("y"))];
        for previous in [Some(&before), None] {
            assert_eq!(rebuilds(plan(&changes, &[], previous, &after)), ["kmod"]);
        }
    }

    #[test]
    fn changed_options_rebuild_the_package_and_its_dependents() {
        let current = graph(BASE);
        let changes = [change(
            "BR2_PACKAGE_LIBCAMERA_PIPELINE_RPI_PISP",
            None,
            Some("y"),
        )];
        let plan = plan(&changes, &[], Some(&current), &current);
        let CleanPlan::Packages(rebuild) = plan else {
            panic!("expected a package rebuild, got {plan:?}");
        };
        assert_eq!(
            rebuild.rebuild.into_iter().collect::<Vec<_>>(),
            ["libcamera", "libcamera-apps", "photonvision"]
        );
        assert_eq!(
            rebuild.reasons,
            [
                "libcamera changed: BR2_PACKAGE_LIBCAMERA_PIPELINE_RPI_PISP unset -> y",
                "dependent packages: libcamera-apps, photonvision",
            ]
        );
    }

    #[test]
    fn override_content_changes_rebuild_only_the_overridden_package() {
        let current = graph(BASE);
        assert_eq!(
            rebuilds(plan(&[], &["mesa3d"], Some(&current), &current)),
            ["mesa3d"]
        );
        assert_eq!(
            rebuilds(plan(&[], &["libcamera"], None, &current)),
            ["libcamera", "libcamera-apps", "photonvision"]
        );
    }

    #[test]
    fn version_changes_rebuild_the_package_and_its_dependents() {
        let before = graph(BASE);
        let mut after = before.clone();
        if let Some(gstreamer) = after.packages.get_mut("gstreamer1") {
            gstreamer.version = Some("2".to_string());
        }
        assert_eq!(
            rebuilds(plan(&[], &[], Some(&before), &after)),
            ["gstreamer1", "photonvision"]
        );
    }

    #[test]
    fn removed_packages_are_uninstalled_and_their_dependents_rebuilt() {
        let before = graph(BASE);
        let after = graph(
            &[
                ("toolchain", &[][..]),
                ("host-xz", &[]),
                ("libcamera", &["toolchain"]),
                ("gstreamer1", &["toolchain"]),
                ("photonvision", &["gstreamer1"]),
                ("kmod", &["toolchain"]),
                ("mesa3d", &["toolchain"]),
            ][..],
        );
        let changes = [change("BR2_PACKAGE_LIBCAMERA_APPS", Some("y"), None)];
        let CleanPlan::Packages(rebuild) = plan(&changes, &[], Some(&before), &after) else {
            panic!("expected a package rebuild");
        };
        assert_eq!(
            rebuild.removed.into_iter().collect::<Vec<_>>(),
            ["libcamera-apps"]
        );
        assert_eq!(
            rebuild.rebuild.into_iter().collect::<Vec<_>>(),
            ["photonvision"]
        );
        // Without the previous graph, what the removed package built is
        // unknown.
        assert!(matches!(
            plan(&changes, &[], None, &after),
            CleanPlan::Full(_)
        ));
    }

    #[test]
    fn toolchain_and_system_settings_clean_everything() {
        let current = graph(BASE);
        for key in [
            "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_BLEEDING_EDGE",
            "BR2_cortex_a76",
            "BR2_INIT_SYSTEMD",
            "BR2_PACKAGE_TOOLCHAIN_EXTERNAL_GDBSERVER_COPY",
        ] {
            let plan = plan(
                &[change(key, None, Some("y"))],
                &[],
                Some(&current),
                &current,
            );
            let CleanPlan::Full(reasons) = plan else {
                panic!("{key}: expected a full clean, got {plan:?}");
            };
            assert!(reasons[0].starts_with(key), "{reasons:?}");
        }
    }

    #[test]
    fn helper_symbols_of_no_package_are_ignored() {
        let current = graph(BASE);
        let changes = [change("BR2_PACKAGE_XORG7", None, Some("y"))];
        assert_eq!(
            plan(&changes, &[], Some(&current), &current),
            CleanPlan::Nothing
        );
    }

    #[test]
    fn settings_belong_to_the_longest_matching_package() {
        let names = BTreeSet::from(["libcamera", "libcamera-apps", "linux", "host-xz", "jpeg"]);
        let owner = |key: &str| owning_package(key, &names);
        assert_eq!(
            owner("BR2_PACKAGE_LIBCAMERA_APPS_PREVIEW"),
            Some("libcamera-apps")
        );
        assert_eq!(owner("BR2_PACKAGE_LIBCAMERA_V4L2"), Some("libcamera"));
        assert_eq!(
            owner("BR2_LINUX_KERNEL_CUSTOM_TARBALL_LOCATION"),
            Some("linux")
        );
        assert_eq!(owner("BR2_PACKAGE_HOST_XZ"), Some("host-xz"));
        assert_eq!(owner("BR2_PACKAGE_PROVIDES_JPEG"), Some("jpeg"));
        assert_eq!(owner("BR2_PACKAGE_LIBCAMERAX"), None);
    }
}
