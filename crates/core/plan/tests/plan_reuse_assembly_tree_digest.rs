//! Reuse of the image assembly for a `${assembly.sha256_tree:...}` JSON map:
//! the map is a function of the tree's bytes, so a changed file rebuilds the
//! assembly, while rewriting a file with the same bytes does not.

pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{
    AssemblyFileSpec, AssemblyJsonValue, AssemblyRoots, AssemblyTreeSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec_with_root};

/// An assembly whose `slot.json` holds the digest map of the `payload` tree,
/// which already holds `a.txt` (the tree is filled before the run).
fn spec_with_tree(scratch: &TempDir, name: &str) -> (ResolvedBuildSpec, PathBuf) {
    let root_dir = scratch.join(name).display().to_string();
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = test_spec_with_root(scratch, root_dir);
    let assembly = ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![
            AssemblyTreeSpec {
                id: "boot".into(),
                path: "$assembly.work/boot".into(),
            },
            AssemblyTreeSpec {
                id: "payload".into(),
                path: "$assembly.out/payload".into(),
            },
        ],
        files: vec![AssemblyFileSpec {
            exclude: Vec::new(),
            tree: "boot".into(),
            src: None,
            src_glob: None,
            content: None,
            json: Some(AssemblyJsonValue::Object(vec![(
                "boot".into(),
                AssemblyJsonValue::Object(vec![(
                    "files".into(),
                    AssemblyJsonValue::String("${assembly.sha256_tree:payload}".into()),
                )]),
            )])),
            dest: "slot.json".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    };
    let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");
    let payload = roots
        .resolve_path(&spec, "$assembly.out/payload")
        .expect("payload path");
    fs::create_dir_all(&payload).expect("payload dir");
    fs::write(payload.join("a.txt"), "one").expect("payload file");
    spec.image.assembly = Some(assembly);
    (spec, payload)
}

fn reuse_state_for_assembly(spec: &ResolvedBuildSpec) -> ReuseState {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["image:assembly"];
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
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

#[test]
fn plan_rebuilds_assembly_when_a_file_of_the_digested_tree_changes() {
    let scratch = TempDir::new("gaia-plan-test");
    let (spec, payload) = spec_with_tree(&scratch, "gaia-plan-tree-digest-changed");
    let state = reuse_state_for_assembly(&spec);
    assert_ne!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "an unchanged tree keeps the assembly's fingerprint"
    );
    fs::write(payload.join("a.txt"), "two, longer").expect("changed file");
    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "a changed file of the digested tree ({})",
        payload.display()
    );
}

#[test]
fn plan_keeps_assembly_when_the_digested_tree_is_rewritten_with_the_same_bytes() {
    let scratch = TempDir::new("gaia-plan-test");
    let (spec, payload) = spec_with_tree(&scratch, "gaia-plan-tree-digest-same");
    let state = reuse_state_for_assembly(&spec);
    fs::remove_file(payload.join("a.txt")).expect("remove");
    fs::write(payload.join("a.txt"), "one").expect("rewritten with the same bytes");
    assert_ne!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "the digest map depends on bytes, not on mtimes"
    );
    // A new file in the tree is a change to the map.
    fs::write(payload.join("b.txt"), "extra").expect("new file");
    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "a new file in the digested tree ({})",
        payload.display()
    );
}
