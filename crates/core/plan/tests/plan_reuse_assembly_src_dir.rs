//! Reuse of the image assembly for a `src_dir` file entry: the copied
//! directory is fingerprinted by its whole tree (names, content, modes, link
//! targets), so a nested change rebuilds the assembly, while rewriting the
//! tree with the same bytes does not.

pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec, ImageAssemblySpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec_with_root};

/// An assembly that copies `site` (a nested tree with a chunk) into the
/// `web` tree's root.
fn spec_with_site(scratch: &TempDir, name: &str) -> (ResolvedBuildSpec, PathBuf) {
    let root_dir = scratch.join(name).display().to_string();
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = test_spec_with_root(scratch, root_dir);
    let site = scratch.join(name).join("site");
    fs::create_dir_all(site.join("_app/immutable/chunks")).expect("site dirs");
    fs::write(site.join("index.html"), "one").expect("index");
    fs::write(site.join("_app/immutable/chunks/abc.js"), "chunk").expect("chunk");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "web".into(),
            path: "$assembly.out/web".into(),
        }],
        files: vec![AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            exclude: Vec::new(),
            tree: "web".into(),
            src: None,
            src_glob: None,
            src_dir: Some(site.display().to_string().into()),
            content: None,
            json: None,
            dest: ".".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        }],
        ..ImageAssemblySpec::default()
    });
    (spec, site)
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

#[test]
fn plan_keeps_assembly_when_the_copied_directory_is_unchanged() {
    let scratch = TempDir::new("gaia-plan-src-dir");
    let (spec, _site) = spec_with_site(&scratch, "gaia-plan-src-dir-same");
    let state = reuse_state_for_assembly(&spec);
    assert_ne!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "an unchanged src_dir keeps the assembly's fingerprint"
    );
}

#[test]
fn plan_rebuilds_assembly_when_a_nested_file_of_the_copied_directory_changes() {
    let scratch = TempDir::new("gaia-plan-src-dir");
    let (spec, site) = spec_with_site(&scratch, "gaia-plan-src-dir-changed");
    let state = reuse_state_for_assembly(&spec);
    fs::write(site.join("_app/immutable/chunks/abc.js"), "chunk, longer").expect("changed");
    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "a nested file of the copied tree changed"
    );
}

#[test]
fn plan_rebuilds_assembly_when_a_nested_file_is_added() {
    let scratch = TempDir::new("gaia-plan-src-dir");
    let (spec, site) = spec_with_site(&scratch, "gaia-plan-src-dir-added");
    let state = reuse_state_for_assembly(&spec);
    fs::write(site.join("_app/immutable/chunks/new.js"), "new").expect("added");
    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "a nested file was added to the copied tree"
    );
}

#[cfg(unix)]
#[test]
fn plan_rebuilds_assembly_when_a_file_mode_in_the_copied_directory_changes() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-plan-src-dir");
    let (spec, site) = spec_with_site(&scratch, "gaia-plan-src-dir-mode");
    let state = reuse_state_for_assembly(&spec);
    fs::set_permissions(site.join("index.html"), fs::Permissions::from_mode(0o755)).expect("mode");
    assert_eq!(
        assembly_reuse_code(&spec, &state).as_deref(),
        Some("operation_fingerprint_mismatch"),
        "a mode in the copied tree changed"
    );
}
