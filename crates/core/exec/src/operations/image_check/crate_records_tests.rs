//! One version of each own crate per image.

use super::crate_records::check_splits;
use super::kernel_modules::{Outcome, Severity};
use super::surface_state_tests::{INTERPRETER, findings, rust};
use super::surfaces_tests::{object, put};
use gaia_spec::{CrateSplitSpec, InstallEntrySpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::path::Path;

/// Records `crates` (name, version, source) as the artifact's crate graph.
fn record(output: &Path, crates: &[Crate]) {
    let path = gaia_spec::rust_crates_record_path(output);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let crates = crates
        .iter()
        .map(|(name, version, source)| {
            serde_json::json!({"name": name, "version": version, "source": source, "features": []})
        })
        .collect::<Vec<_>>();
    std::fs::write(
        path,
        serde_json::json!({"schema": 2, "package": "x", "crates": crates}).to_string(),
    )
    .expect("record");
}

/// A crate's name, version and source.
type Crate<'a> = (&'a str, &'a str, &'a str);

const LEMNOS_A: &str = "git+https://example.org/Lemnos.git#aaaa";
const LEMNOS_B: &str = "git+https://example.org/Lemnos.git#bbbb";

fn image(scratch: &TempDir) -> (ResolvedBuildSpec, std::path::PathBuf) {
    let root = scratch.join("rootfs");
    let out = scratch.join("out");
    let mut spec = ResolvedBuildSpec::new("crate-splits");
    let programs: [(&str, Vec<Crate>); 3] = [
        (
            "styx",
            vec![
                ("lemnos-ipc", "0.4.0", LEMNOS_A),
                ("serde", "1.0.200", ""),
                ("eidos-core", "0.2.0", "/src/eidos"),
            ],
        ),
        (
            "eidos",
            vec![
                ("lemnos_ipc", "0.4.0", LEMNOS_B),
                ("serde", "1.0.210", ""),
                ("eidos-core", "0.2.0", "/src/eidos"),
            ],
        ),
        (
            // A split inside one artifact counts too.
            "daedalus",
            vec![
                ("daedalus-accel", "0.1.0", "/src/eidos/vendor/accel"),
                ("daedalus-accel", "0.1.0", "/src/daedalus/accel"),
            ],
        ),
    ];
    for (id, crates) in programs {
        let output = out.join(id);
        spec.artifacts.push(rust(id, id, &output, None));
        spec.install.entries.push(InstallEntrySpec::new(
            format!("i-{id}"),
            id,
            format!("/usr/bin/{id}"),
        ));
        put(
            &root,
            &format!("/usr/bin/{id}"),
            &object(INTERPRETER, &[], &[], &[]),
        );
        record(&output, &crates);
    }
    (spec, root)
}

#[test]
fn an_own_crate_at_two_versions_or_sources_is_an_error_unless_allowed() {
    let scratch = TempDir::new("gaia-image-check-crate-splits");
    let (mut spec, root) = image(&scratch);

    let mut outcome = Outcome::default();
    check_splits(&spec, &root, &mut outcome);
    // serde's two registry versions are cargo's business; eidos-core is one.
    assert_eq!(
        findings(&outcome, Severity::Error),
        vec![
            ("crate_version_split", Some("daedalus-accel".into())),
            ("crate_version_split", Some("lemnos-ipc".into())),
        ]
    );
    let message = &outcome.findings[1].message;
    assert!(
        message.contains(&format!("0.4.0 from {LEMNOS_A} ('styx')")),
        "{message}"
    );
    assert!(
        message.contains(&format!("0.4.0 from {LEMNOS_B} ('eidos')")),
        "{message}"
    );

    spec.image.checks.crate_splits.push(CrateSplitSpec {
        crate_name: "daedalus_accel".into(),
        reason: "Eidos pins its own accel until the lock moves".into(),
    });
    let mut outcome = Outcome::default();
    check_splits(&spec, &root, &mut outcome);
    assert_eq!(
        findings(&outcome, Severity::Error),
        vec![("crate_version_split", Some("lemnos-ipc".into()))]
    );
}
