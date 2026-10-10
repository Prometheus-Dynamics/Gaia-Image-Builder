//! `--rebuild-package`: Buildroot packages a run dircleans and builds again
//! instead of restoring them from the package cache. They join the clean
//! decision (uninstalled, their build directories removed, as for a changed
//! package) and are left out of the cache restore, so the make that follows
//! builds them and the package cache stores the new build.
use super::*;

/// The packages named by `--rebuild-package`, checked against `graph`. A name
/// the build does not have is an error listing close names.
pub(crate) fn requested_package_rebuilds(
    policy: &ImageExecutionPolicy,
    graph: Option<&PackageGraph>,
) -> Result<BTreeSet<String>, ImageProviderError> {
    let requested = policy
        .rebuild_packages
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if requested.is_empty() {
        return Ok(requested);
    }
    let Some(graph) = graph else {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            "--rebuild-package needs the Buildroot package graph, which this run could not read",
        ));
    };
    let unknown = requested
        .iter()
        .filter(|name| !graph.contains(name.as_str()))
        .collect::<Vec<_>>();
    if unknown.is_empty() {
        return Ok(requested);
    }
    let known = graph.package_names().collect::<Vec<_>>();
    let hints = unknown
        .iter()
        .map(|name| {
            let close = known
                .iter()
                .filter(|known| known.contains(name.as_str()))
                .take(3)
                .map(|known| format!("'{known}'"))
                .collect::<Vec<_>>();
            if close.is_empty() {
                format!("'{name}'")
            } else {
                format!("'{name}' (did you mean {}?)", close.join(", "))
            }
        })
        .collect::<Vec<_>>();
    Err(ImageProviderError::new(
        ImageProviderErrorKind::RuntimeState,
        format!(
            "--rebuild-package names no package of this Buildroot build: {}",
            hints.join(", ")
        ),
    ))
}

/// `clean` with `requested` added to the packages it rebuilds. A full clean
/// already rebuilds every package, so it stays as it is.
pub(crate) fn with_requested_rebuilds(clean: CleanPlan, requested: &BTreeSet<String>) -> CleanPlan {
    if requested.is_empty() {
        return clean;
    }
    let reason = format!(
        "rebuild requested with --rebuild-package: {}",
        requested.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    match clean {
        CleanPlan::Full(reasons) => CleanPlan::Full(reasons),
        CleanPlan::Nothing => CleanPlan::Packages(PackageRebuild {
            rebuild: requested.clone(),
            reasons: vec![reason],
            ..PackageRebuild::default()
        }),
        CleanPlan::Finalize {
            mut reasons,
            refresh_target,
        } => {
            reasons.push(reason);
            CleanPlan::Packages(PackageRebuild {
                rebuild: requested.clone(),
                reasons,
                refresh_target,
                ..PackageRebuild::default()
            })
        }
        CleanPlan::Packages(mut rebuild) => {
            rebuild.rebuild.extend(requested.iter().cloned());
            rebuild.reasons.push(reason);
            CleanPlan::Packages(rebuild)
        }
    }
}

/// The packages the cache restore leaves to make: those whose sources the
/// image reads (`keep`) and the requested ones.
pub(crate) fn excluded_from_restore(
    keep: &BTreeSet<String>,
    requested: &BTreeSet<String>,
) -> BTreeSet<String> {
    keep.union(requested).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Two packages: `app` depends on `base`.
    fn cache_graph() -> PackageGraph {
        let mut graph = PackageGraph::default();
        for (name, dependencies) in [("base", vec![]), ("app", vec!["base"])] {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    kind: "target".to_string(),
                    stamp_dir: Some(format!("build/{name}-1")),
                    dependencies: dependencies.into_iter().map(str::to_string).collect(),
                    ..PackageInfo::default()
                },
            );
        }
        graph
    }

    fn policy(packages: &[&str]) -> ImageExecutionPolicy {
        ImageExecutionPolicy {
            rebuild_packages: packages.iter().map(|name| name.to_string()).collect(),
            ..ImageExecutionPolicy::default()
        }
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn requested_packages_join_every_kind_of_clean_plan() {
        let app = names(&["app"]);
        assert_eq!(
            with_requested_rebuilds(CleanPlan::Nothing, &app),
            CleanPlan::Packages(PackageRebuild {
                rebuild: app.clone(),
                reasons: vec!["rebuild requested with --rebuild-package: app".into()],
                ..PackageRebuild::default()
            })
        );
        let full = CleanPlan::Full(vec!["toolchain changed".into()]);
        assert_eq!(with_requested_rebuilds(full.clone(), &app), full);
        let CleanPlan::Packages(rebuild) = with_requested_rebuilds(
            CleanPlan::Packages(PackageRebuild {
                rebuild: names(&["base"]),
                ..PackageRebuild::default()
            }),
            &app,
        ) else {
            panic!("a package plan stays a package plan");
        };
        assert_eq!(rebuild.rebuild, names(&["app", "base"]));
        // Nothing requested: the decision is unchanged.
        assert_eq!(
            with_requested_rebuilds(CleanPlan::Nothing, &BTreeSet::new()),
            CleanPlan::Nothing
        );
    }

    #[test]
    fn unknown_packages_are_refused_with_close_names() {
        let graph = cache_graph();
        let error = requested_package_rebuilds(&policy(&["ap"]), Some(&graph))
            .expect_err("'ap' is a substring of 'app' only");
        assert!(
            error.message.contains("'ap' (did you mean 'app'?)"),
            "{}",
            error.message
        );
        let error =
            requested_package_rebuilds(&policy(&["nope"]), Some(&graph)).expect_err("unknown");
        assert!(error.message.contains("'nope'"), "{}", error.message);
        assert_eq!(
            requested_package_rebuilds(&policy(&["app"]), Some(&graph)).expect("known"),
            names(&["app"])
        );
        assert!(
            requested_package_rebuilds(&policy(&[]), None)
                .expect("none")
                .is_empty()
        );
        assert!(requested_package_rebuilds(&policy(&["app"]), None).is_err());
    }
}
