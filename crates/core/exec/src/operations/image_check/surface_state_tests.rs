//! "Not both": programs that link a surface's state and load its library.

use super::kernel_modules::{Outcome, Severity};
use super::surface_state::check;
use super::surfaces_tests::{object, put};
use gaia_spec::{
    ArtifactDefinition, ArtifactOutputSpec, ArtifactSpec, ArtifactVariantSpec, InstallEntrySpec,
    ResolvedBuildSpec, RustArtifactSpec, RustSurfaceSpec,
};
use gaia_testkit::TempDir;
use std::path::Path;

pub(super) const INTERPRETER: Option<&str> = Some("/lib64/ld-linux-x86-64.so.2");

pub(super) fn rust(
    id: &str,
    package: &str,
    output: &Path,
    surface: Option<Box<RustSurfaceSpec>>,
) -> ArtifactSpec {
    ArtifactSpec::new(
        id,
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: package.into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
            cargo_git_sources: Vec::new(),
            build_env: Vec::new(),
            cpu: None,
            link: gaia_spec::RustLinkSpec::Static,
            lto: gaia_spec::RustLtoSpec::Profile,
            surface,
            trim: None,
        }),
        None,
        ArtifactOutputSpec {
            path: output.display().to_string(),
        },
    )
}

/// Records `crates` (name, features) as the artifact's crate graph.
fn record(output: &Path, crates: &[(&str, &[&str])]) {
    let path = gaia_spec::rust_crates_record_path(output);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let crates = crates
        .iter()
        .map(|(name, features)| serde_json::json!({"name": name, "features": features}))
        .collect::<Vec<_>>();
    std::fs::write(
        path,
        serde_json::json!({"schema": 1, "package": "x", "crates": crates}).to_string(),
    )
    .expect("record");
}

pub(super) fn findings(
    outcome: &Outcome,
    severity: Severity,
) -> Vec<(&'static str, Option<String>)> {
    outcome
        .findings
        .iter()
        .filter(|finding| finding.severity == severity)
        .map(|finding| (finding.code, finding.module.clone()))
        .collect()
}

#[test]
fn a_program_linking_the_surface_state_and_loading_it_is_an_error() {
    let scratch = TempDir::new("gaia-image-check-surface-state");
    let root = scratch.join("rootfs");
    let out = scratch.join("out");
    let mut spec = ResolvedBuildSpec::new("surface-state");
    spec.workspace.root_dir = scratch.path().display().to_string();
    let mut surface = RustSurfaceSpec::new("lemnos.exports");
    surface.shared_feature = Some("shared".into());
    spec.artifacts.push(rust(
        "capi",
        "lemnos-capi",
        &out.join("liblemnos_capi.so"),
        Some(Box::new(surface)),
    ));
    spec.install.entries.push(InstallEntrySpec::new(
        "i-capi",
        "capi",
        "/usr/lib/liblemnos_capi.so.1",
    ));
    put(
        &root,
        "/usr/lib/liblemnos_capi.so.1",
        &object(None, &[], &["lemnos_version"], &[]),
    );
    for (id, links_static, shared, recorded, loads) in [
        ("both", true, false, true, true),
        ("wrapper-shared", true, true, true, true),
        ("static-only", true, false, true, false),
        ("unrecorded", true, false, false, true),
        ("core-only", false, false, true, true),
    ] {
        let output = out.join(id);
        spec.artifacts.push(rust(id, id, &output, None));
        spec.install.entries.push(InstallEntrySpec::new(
            format!("i-{id}"),
            id,
            format!("/usr/bin/{id}"),
        ));
        let needed: &[&str] = if loads { &["liblemnos_capi.so.1"] } else { &[] };
        put(
            &root,
            &format!("/usr/bin/{id}"),
            &object(INTERPRETER, needed, &[], &[]),
        );
        if recorded {
            let features: &[&str] = if shared {
                &["default", "shared"]
            } else {
                &["default"]
            };
            let mut crates: Vec<(&str, &[&str])> = vec![(id, &[]), ("lemnos_ipc", &["std"])];
            if links_static {
                crates.push(("lemnos_capi", features));
            }
            record(&output, &crates);
        }
    }

    let mut outcome = Outcome::default();
    check(&spec, &root, &mut outcome);

    assert_eq!(
        findings(&outcome, Severity::Error),
        vec![("surface_state_twice", Some("/usr/bin/both".into()))]
    );
    assert_eq!(
        findings(&outcome, Severity::Warning),
        vec![(
            "surface_crates_unrecorded",
            Some("/usr/bin/unrecorded".into())
        )]
    );
    let message = &outcome.findings[0].message;
    assert!(
        message.contains("links lemnos_capi statically"),
        "{message}"
    );
    assert!(
        message.contains("turn on lemnos-capi's 'shared' feature"),
        "{message}"
    );
}

#[test]
fn a_declared_state_crate_other_than_the_package_counts_whatever_its_features() {
    let scratch = TempDir::new("gaia-image-check-surface-state-crates");
    let root = scratch.join("rootfs");
    let out = scratch.join("out");
    let mut spec = ResolvedBuildSpec::new("surface-state-crates");
    let mut surface = RustSurfaceSpec::new("x.exports");
    surface.state_crates = vec!["acme-registry".into()];
    surface.shared_feature = Some("shared".into());
    spec.artifacts.push(rust(
        "surface",
        "acme-surface",
        &out.join("libacme.so"),
        Some(Box::new(surface)),
    ));
    spec.install.entries.push(InstallEntrySpec::new(
        "i-surface",
        "surface",
        "/usr/lib/libacme.so",
    ));
    put(
        &root,
        "/usr/lib/libacme.so",
        &object(None, &[], &["acme_open"], &[]),
    );
    spec.artifacts
        .push(rust("app", "app", &out.join("app"), None));
    spec.install
        .entries
        .push(InstallEntrySpec::new("i-app", "app", "/usr/bin/app"));
    put(
        &root,
        "/usr/bin/app",
        &object(INTERPRETER, &["libacme.so"], &[], &[]),
    );
    // The surface package with its shared feature, but the registry linked in.
    record(
        &out.join("app"),
        &[
            ("acme-surface", &["shared"]),
            ("acme-registry", &["shared"]),
        ],
    );

    let mut outcome = Outcome::default();
    check(&spec, &root, &mut outcome);
    assert_eq!(
        findings(&outcome, Severity::Error),
        vec![("surface_state_twice", Some("/usr/bin/app".into()))]
    );
}

/// A Rust plugin under the surface's `plugin_dirs` runs in the surface's
/// process, so linking the state in is "both" even without `DT_NEEDED`.
#[test]
fn a_plugin_linking_the_surface_state_is_an_error() {
    let scratch = TempDir::new("gaia-image-check-surface-state-plugins");
    let root = scratch.join("rootfs");
    let out = scratch.join("out");
    let mut spec = ResolvedBuildSpec::new("surface-state-plugins");
    let mut surface = RustSurfaceSpec::new("x.exports");
    surface.plugin_dirs = vec!["/usr/lib/helios/plugins".into()];
    spec.artifacts.push(rust(
        "surface",
        "daedalus-capi",
        &out.join("libdaedalus.so"),
        Some(Box::new(surface)),
    ));
    spec.install.entries.push(InstallEntrySpec::new(
        "i-surface",
        "surface",
        "/usr/lib/libdaedalus.so",
    ));
    put(
        &root,
        "/usr/lib/libdaedalus.so",
        &object(None, &[], &["daedalus_open"], &[]),
    );
    for id in ["eidos-plugin", "styx-plugin"] {
        let output = out.join(format!("lib{id}.so"));
        spec.artifacts.push(rust(id, id, &output, None));
        spec.install.entries.push(InstallEntrySpec::new(
            format!("i-{id}"),
            id,
            format!("/usr/lib/helios/plugins/lib{id}.so"),
        ));
        put(
            &root,
            &format!("/usr/lib/helios/plugins/lib{id}.so"),
            &object(None, &[], &[], &[]),
        );
        let crates: Vec<(&str, &[&str])> = if id == "eidos-plugin" {
            vec![(id, &[]), ("daedalus-capi", &["default"])]
        } else {
            vec![(id, &[])]
        };
        record(&output, &crates);
    }

    let mut outcome = Outcome::default();
    check(&spec, &root, &mut outcome);
    assert_eq!(
        findings(&outcome, Severity::Error),
        vec![(
            "surface_state_twice",
            Some("/usr/lib/helios/plugins/libeidos-plugin.so".into())
        )]
    );
    assert!(
        outcome.findings[0]
            .message
            .contains("is a plugin opened by surface /usr/lib/libdaedalus.so"),
        "{}",
        outcome.findings[0].message
    );
}
