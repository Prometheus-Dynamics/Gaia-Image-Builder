//! An image assembly whose files read `${source.<id>.path}` depends on that
//! source: a changed file inside the source reruns the assembly, and an
//! unchanged source keeps it reused.

pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::ResolvedBuildSpec;
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::provider_catalogs;

/// A build whose assembly copies `payload.bin` out of the path source `app`.
fn spec_reading_source(scratch: &TempDir) -> (ResolvedBuildSpec, PathBuf) {
    spec_with_entry(
        scratch,
        r#"src = "${source.app.path}/payload.bin"
dest = "payload.bin""#,
    )
}

/// A build whose assembly names the source's directory in generated text and
/// never reads its files: only the source's content can change the assembly.
fn spec_naming_source(scratch: &TempDir) -> (ResolvedBuildSpec, PathBuf) {
    spec_with_entry(
        scratch,
        r#"content = "root=${source.app.path}"
dest = "where.txt""#,
    )
}

fn spec_with_entry(scratch: &TempDir, entry: &str) -> (ResolvedBuildSpec, PathBuf) {
    let root = scratch.join("workspace");
    let source = scratch.join("app");
    fs::create_dir_all(&root).expect("workspace");
    fs::create_dir_all(&source).expect("source");
    let payload = source.join("payload.bin");
    fs::write(&payload, "one").expect("payload");
    let config = scratch.join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
build_name = "myboard-slot"
version = "1.0.0"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app"
kind = "path"
path = "{source}"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.files]]
tree = "boot"
{entry}
"#,
            root = root.display(),
            source = source.display(),
            entry = entry,
        ),
    )
    .expect("config");
    let spec = gaia_config::resolve_config(config.to_str().expect("utf-8 path"));
    (spec, payload)
}

/// A reuse state recording every operation of the current plan as done.
fn reuse_state_for(spec: &ResolvedBuildSpec) -> ReuseState {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str().to_string())
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: plan
            .operations
            .iter()
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: plan
            .operations
            .iter()
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: Default::default(),
    }
}

/// The reuse decision of the assembly under `state`.
fn assembly_reuse(spec: &ResolvedBuildSpec, state: &ReuseState) -> OperationReuse {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(state),
    );
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .map(|operation| operation.reuse.clone())
        .expect("assembly operation")
}

fn reuse_code(reuse: &OperationReuse) -> String {
    match reuse {
        OperationReuse::Execute(reason) => reason.code.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn the_assembly_depends_on_the_source_its_tokens_name() {
    let scratch = TempDir::new("gaia-plan-assembly-source");
    let (spec, _payload) = spec_reading_source(&scratch);
    assert_eq!(
        spec.image
            .assembly
            .as_ref()
            .expect("assembly")
            .assembly_sources
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["app"]
    );
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let assembly = plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("assembly operation");
    assert!(
        assembly
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "source:app"),
        "{:?}",
        assembly.depends_on
    );
}

#[test]
fn a_changed_file_in_the_source_reruns_the_assembly_and_an_unchanged_one_reuses_it() {
    let scratch = TempDir::new("gaia-plan-assembly-source-reuse");
    let (spec, payload) = spec_reading_source(&scratch);
    let state = reuse_state_for(&spec);

    // Nothing changed: the source and the assembly keep their fingerprints.
    let unchanged = reuse_code(&assembly_reuse(&spec, &state));
    assert_ne!(unchanged, "operation_fingerprint_mismatch", "{unchanged}");
    assert_ne!(unchanged, "dependency_rebuilt", "{unchanged}");

    fs::write(&payload, "two, longer").expect("changed file");
    assert_eq!(
        reuse_code(&assembly_reuse(&spec, &state)),
        "operation_fingerprint_mismatch",
        "a changed file inside the source ({})",
        payload.display()
    );
}

#[test]
fn naming_the_source_in_generated_text_makes_its_content_an_input() {
    let scratch = TempDir::new("gaia-plan-assembly-source-named");
    let (spec, payload) = spec_naming_source(&scratch);
    let state = reuse_state_for(&spec);
    assert_ne!(
        reuse_code(&assembly_reuse(&spec, &state)),
        "operation_fingerprint_mismatch",
        "nothing changed"
    );
    fs::write(&payload, "two, longer").expect("changed file");
    assert_eq!(
        reuse_code(&assembly_reuse(&spec, &state)),
        "operation_fingerprint_mismatch",
        "a changed file in a source the assembly names ({})",
        payload.display()
    );
}
