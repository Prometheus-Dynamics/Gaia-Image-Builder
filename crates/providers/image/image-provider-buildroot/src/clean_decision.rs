//! The clean a run makes of an output tree, decided from what changed since
//! the last build. A run acts on this decision and `gaia preview` reports
//! it, so both always agree.
use super::*;

/// What changed in an output tree's config and package overrides, against
/// the state the last build recorded.
#[derive(Debug, Default)]
pub(crate) struct TreeChanges {
    /// Digest of the final config; `None` when there is no `.config`.
    pub(crate) config_digest: Option<String>,
    pub(crate) config_changes: Vec<ConfigChange>,
    /// The config changed, and no snapshot names what.
    pub(crate) unattributed_config_change: bool,
    pub(crate) override_digests: BTreeMap<String, String>,
    /// Package overrides, and host tools, that changed.
    pub(crate) override_changes: BTreeSet<String>,
}

impl TreeChanges {
    pub(crate) fn something_changed(&self) -> bool {
        !self.config_changes.is_empty() || !self.override_changes.is_empty()
    }
}

/// Whether the decision needs the current package graph (a `make show-info`):
/// only for a built tree whose config or overrides changed, or a tree with no
/// recorded graph yet.
pub(crate) fn needs_current_graph(
    built_before: bool,
    changes: &TreeChanges,
    no_previous_graph: bool,
) -> bool {
    (built_before && changes.something_changed()) || no_previous_graph
}

/// What [`decide_clean`] decides from.
pub(crate) struct CleanDecisionInput<'a> {
    /// The tree has been built (its big directories exist).
    pub(crate) built_before: bool,
    pub(crate) changes: &'a TreeChanges,
    /// The graph recorded with the last built config.
    pub(crate) previous: Option<&'a PackageGraph>,
    /// The current config's graph, when it was read (see
    /// [`needs_current_graph`]).
    pub(crate) current: Option<&'a PackageGraph>,
    pub(crate) symbol_use: &'a dyn Fn(&str) -> SymbolUse,
    /// Per-package directories are on.
    pub(crate) per_package: bool,
}

/// The clean for a tree, from its changes. Pure.
pub(crate) fn decide_clean(input: CleanDecisionInput<'_>) -> CleanPlan {
    let changes = input.changes;
    if !input.built_before {
        return CleanPlan::Nothing;
    }
    if changes.unattributed_config_change {
        return CleanPlan::Full(vec![
            "effective config changed (no snapshot of the previously built config)".to_string(),
        ]);
    }
    if !changes.something_changed() {
        return CleanPlan::Nothing;
    }
    if let Some(current) = input.current {
        return plan_clean(CleanInputs {
            config_changes: &changes.config_changes,
            override_changes: &changes.override_changes,
            previous: input.previous,
            current,
            symbol_use: input.symbol_use,
            per_package: input.per_package,
        });
    }
    let mut reasons = changes
        .config_changes
        .iter()
        .map(|change| change.key.clone())
        .chain(
            changes
                .override_changes
                .iter()
                .map(|name| format!("package override {name}")),
        )
        .collect::<Vec<_>>();
    reasons.push("Buildroot did not report its package graph".to_string());
    CleanPlan::Full(reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph from `(name, dependencies)`, with reverse dependencies.
    fn graph(packages: &[(&str, &[&str])]) -> PackageGraph {
        let mut graph = PackageGraph::default();
        for (name, dependencies) in packages {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    kind: "target".to_string(),
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

    fn changes(config: Vec<ConfigChange>, overrides: &[&str]) -> TreeChanges {
        TreeChanges {
            config_changes: config,
            override_changes: overrides.iter().map(|name| name.to_string()).collect(),
            ..TreeChanges::default()
        }
    }

    fn decide(
        built_before: bool,
        changes: &TreeChanges,
        previous: Option<&PackageGraph>,
        current: Option<&PackageGraph>,
    ) -> CleanPlan {
        let symbol_use = |_: &str| SymbolUse::default();
        decide_clean(CleanDecisionInput {
            built_before,
            changes,
            previous,
            current,
            symbol_use: &symbol_use,
            per_package: true,
        })
    }

    #[test]
    fn an_unbuilt_tree_or_an_unchanged_one_cleans_nothing() {
        let unchanged = changes(Vec::new(), &[]);
        assert_eq!(decide(false, &unchanged, None, None), CleanPlan::Nothing);
        assert_eq!(decide(true, &unchanged, None, None), CleanPlan::Nothing);
        assert!(!needs_current_graph(false, &unchanged, false));
        assert!(needs_current_graph(false, &unchanged, true));
    }

    #[test]
    fn an_unattributed_config_change_cleans_everything() {
        let mut unattributed = changes(Vec::new(), &[]);
        unattributed.unattributed_config_change = true;
        assert_eq!(
            decide(true, &unattributed, None, None),
            CleanPlan::Full(vec![
                "effective config changed (no snapshot of the previously built config)".to_string()
            ])
        );
    }

    #[test]
    fn a_change_without_a_package_graph_cleans_everything_and_says_so() {
        let config = changes(
            vec![ConfigChange {
                key: "BR2_KERNEL".to_string(),
                previous: None,
                current: Some("y".to_string()),
            }],
            &["foo"],
        );
        let CleanPlan::Full(reasons) = decide(true, &config, None, None) else {
            panic!("expected a full clean");
        };
        assert_eq!(
            reasons,
            vec![
                "BR2_KERNEL".to_string(),
                "package override foo".to_string(),
                "Buildroot did not report its package graph".to_string(),
            ]
        );
    }

    #[test]
    fn an_override_change_rebuilds_that_package_and_its_dependents() {
        let before = graph(&[("foo", &[]), ("bar", &["foo"]), ("baz", &[])]);
        let after = before.clone();
        let changed = changes(Vec::new(), &["foo"]);
        assert!(needs_current_graph(true, &changed, false));
        let CleanPlan::Packages(rebuild) = decide(true, &changed, Some(&before), Some(&after))
        else {
            panic!("expected package rebuilds");
        };
        assert_eq!(
            rebuild.rebuild,
            BTreeSet::from(["bar".to_string(), "foo".to_string()])
        );
        assert!(rebuild.removed.is_empty());
    }
}
