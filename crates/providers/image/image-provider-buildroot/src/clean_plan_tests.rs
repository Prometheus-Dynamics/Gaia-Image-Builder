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
        symbol_use: &|key| test_symbol_use(key),
        per_package: true,
    })
}

/// Settings a test makefile set reads: `BR2_RAZE_USERS_TABLE` only when
/// finalizing, `BR2_RAZE_TUNE` by `kmod`'s `.mk`, anything else
/// unknown to the infrastructure.
fn test_symbol_use(key: &str) -> SymbolUse {
    match key {
        "BR2_RAZE_USERS_TABLE" => SymbolUse {
            finalize: true,
            ..SymbolUse::default()
        },
        "BR2_RAZE_TUNE" => SymbolUse {
            packages: BTreeSet::from(["kmod".to_string()]),
            ..SymbolUse::default()
        },
        "BR2_RAZE_UNUSED" => SymbolUse::default(),
        _ => SymbolUse {
            global: Some("read by Buildroot's package/Makefile.in".to_string()),
            ..SymbolUse::default()
        },
    }
}

#[test]
fn settings_no_package_reads_rebuild_nothing() {
    let current = graph(BASE);
    let result = plan(
        &[change(
            "BR2_RAZE_USERS_TABLE",
            None,
            Some("\"/x/users.table\""),
        )],
        &[],
        Some(&current),
        &current,
    );
    match result {
        CleanPlan::Finalize {
            reasons,
            refresh_target,
        } => {
            assert!(refresh_target);
            assert!(
                reasons[0].contains("only read when finalizing"),
                "{reasons:?}"
            );
        }
        other => panic!("expected no rebuild, got {other:?}"),
    }
    let result = plan(
        &[change("BR2_RAZE_UNUSED", None, Some("y"))],
        &[],
        Some(&current),
        &current,
    );
    assert!(
        matches!(
            result,
            CleanPlan::Finalize {
                refresh_target: false,
                ..
            }
        ),
        "{result:?}"
    );
    let result = plan(
        &[change("BR2_RAZE_TUNE", None, Some("y"))],
        &[],
        Some(&current),
        &current,
    );
    assert_eq!(rebuilds(result), ["kmod"]);
    let result = plan(
        &[change("BR2_RAZE_GLOBAL", None, Some("y"))],
        &[],
        Some(&current),
        &current,
    );
    assert!(matches!(result, CleanPlan::Full(_)), "{result:?}");
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
