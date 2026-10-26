//! Reuse of the image assembly for generated content: a generated file, a
//! JSON document and a listed binary are inputs of the assembly, so a change
//! to what they read or copy makes the next run execute again.

pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{
    AssemblyBinarySpec, AssemblyFileSpec, AssemblyJsonValue, AssemblyRoots, AssemblyTreeSpec,
    ImageAssemblySpec, ResolvedBuildSpec,
};
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, test_spec_with_root};

/// The assembly with one generated file, one JSON file and one listed
/// binary; the token reads a file the assembly does not write, so it is an
/// input that exists before the run.
fn spec_with_generated(scratch: &TempDir, name: &str) -> (ResolvedBuildSpec, PathBuf, PathBuf) {
    let root_dir = scratch.join(name).display().to_string();
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = test_spec_with_root(scratch, root_dir);
    let assembly = ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: None,
                src_glob: None,
                content: Some("size=${assembly.size:$assembly.out/part.bin}".into()),
                json: None,
                dest: "cmdline.txt".into(),
                mode: None,
                optional: false,
                src_dir: None,
                preserve_symlink: false,
                allow_missing_libs: false,
            },
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: None,
                src_glob: None,
                content: None,
                json: Some(AssemblyJsonValue::Object(vec![(
                    "digest".into(),
                    AssemblyJsonValue::String("${assembly.sha256:$assembly.out/part.bin}".into()),
                )])),
                dest: "manifest.json".into(),
                mode: None,
                optional: false,
                src_dir: None,
                preserve_symlink: false,
                allow_missing_libs: false,
            },
        ],
        binaries: vec![AssemblyBinarySpec {
            strip: false,
            tree: "boot".into(),
            src: "$assembly.out/tool".into(),
            dest: "usr/bin/tool".into(),
            sysroot: None,
            mode: None,
        }],
        ..ImageAssemblySpec::default()
    };
    let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");
    let part = roots
        .resolve_path(&spec, "$assembly.out/part.bin")
        .expect("part path");
    let tool = roots
        .resolve_path(&spec, "$assembly.out/tool")
        .expect("tool path");
    for path in [&part, &tool] {
        fs::create_dir_all(path.parent().expect("parent")).expect("out dir");
        fs::write(path, "one").expect("input");
    }
    spec.image.assembly = Some(assembly);
    (spec, part, tool)
}

fn reuse_state_for_assembly(spec: &ResolvedBuildSpec) -> ReuseState {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["image:assembly"];
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        fingerprint_scheme: gaia_spec::FINGERPRINT_SCHEME,
        completed_operation_ids: reused_ids
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: Default::default(),
    }
}

fn assembly_reuse_code(spec: &ResolvedBuildSpec, state: &ReuseState) -> Option<String> {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan: ExecutionPlan = plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(state),
    );
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .map(|operation| match &operation.reuse {
            OperationReuse::Execute(reason) => reason.code.to_string(),
            other => format!("{other:?}"),
        })
}

fn assert_rebuilds(spec: &ResolvedBuildSpec, state: &ReuseState, what: &str, path: &Path) {
    assert_eq!(
        assembly_reuse_code(spec, state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "{what} ({})",
        path.display()
    );
}

#[test]
fn plan_rebuilds_assembly_when_a_file_a_generated_text_reads_changes() {
    let scratch = TempDir::new("gaia-plan-test");
    let (spec, part, _tool) = spec_with_generated(&scratch, "gaia-plan-generated-part");
    let state = reuse_state_for_assembly(&spec);
    assert_ne!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "an unchanged assembly keeps its fingerprint"
    );
    fs::write(&part, "two, longer").expect("changed input");
    assert_rebuilds(&spec, &state, "generated text reads a changed file", &part);
}

#[test]
fn plan_rebuilds_assembly_when_a_listed_binary_changes() {
    let scratch = TempDir::new("gaia-plan-test");
    let (spec, _part, tool) = spec_with_generated(&scratch, "gaia-plan-generated-tool");
    let state = reuse_state_for_assembly(&spec);
    fs::write(&tool, "two, longer").expect("changed binary");
    assert_rebuilds(&spec, &state, "a listed binary changed", &tool);
}

#[test]
fn plan_rebuilds_assembly_when_a_json_token_reads_a_changed_file() {
    let scratch = TempDir::new("gaia-plan-test");
    let (spec, part, _tool) = spec_with_generated(&scratch, "gaia-plan-generated-json");
    let state = reuse_state_for_assembly(&spec);
    // The JSON file reads the same bytes through its digest token.
    fs::write(&part, "another part").expect("changed input");
    assert_rebuilds(
        &spec,
        &state,
        "a JSON digest token reads a changed file",
        &part,
    );
}
