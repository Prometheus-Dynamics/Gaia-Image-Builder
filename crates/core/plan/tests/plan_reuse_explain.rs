pub mod support;

use gaia_plan::{
    InvalidationSummary, OperationKind, OperationReuse, ReuseState, invalidation_summary,
    operation_content_signature, operation_input_signature, operation_output_signature, plan_build,
    plan_build_with_reuse_state,
};
use gaia_spec::{
    ResolvedBuildSpec, SourceDefinition, SourceId, SourcePinPolicySpec, SourceRef,
    SourceRefreshPolicySpec,
};
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, reuse_state_for_ids, test_spec};

const OLD_COMMIT: &str = "3175ad9c0ffee00000000000000000000000001a";
const NEW_COMMIT: &str = "e565dcf0ddba11000000000000000000000000b2";

/// `gaia-app` builds from the pinned `gaia-upstream` git source, so a rev
/// change on that source reaches an artifact. The source is pinned and not
/// refreshed on every run, so the commit is the only thing that changes.
fn upstream_spec(commit: &str) -> ResolvedBuildSpec {
    let mut spec = test_spec();
    set_upstream_commit(&mut spec, commit);
    spec.artifacts[0].source = Some(SourceRef::new("gaia-upstream"));
    spec
}

fn set_upstream_commit(spec: &mut ResolvedBuildSpec, commit: &str) {
    let source = spec
        .sources
        .iter_mut()
        .find(|source| source.id.as_str() == "gaia-upstream")
        .expect("gaia-upstream source");
    let SourceDefinition::Git(git) = &mut source.definition else {
        panic!("gaia-upstream is a git source");
    };
    git.refresh_policy = SourceRefreshPolicySpec::Never;
    git.pin_policy = SourcePinPolicySpec::Locked;
    git.locked_commit = Some(commit.to_string());
}

/// What the last materialization of `gaia-upstream` left on disk.
fn write_upstream_state(spec: &ResolvedBuildSpec, commit: &str, tree_digest: &str) {
    let dir = PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream");
    fs::create_dir_all(&dir).expect("gaia-upstream dir");
    fs::write(dir.join("source.txt"), "ok").expect("source marker");
    fs::write(
        dir.join(".gaia-source-state.txt"),
        format!(
            "provider=source.git\nsource=gaia-upstream\nresolved_mode=locked\n\
             resolved_commit_sha={commit}\nmaterialized_tree_digest={tree_digest}\n"
        ),
    )
    .expect("gaia-upstream state");
}

fn write_artifact_output(spec: &ResolvedBuildSpec) {
    let output = Path::new(&spec.artifacts[0].output.path);
    fs::create_dir_all(output.parent().expect("artifact output parent")).expect("artifact dir");
    fs::write(output, "artifact").expect("artifact output");
}

fn op<'plan>(
    plan: &'plan gaia_plan::ExecutionPlan,
    id: &str,
) -> &'plan gaia_plan::PlannedOperation {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
}

fn message(plan: &gaia_plan::ExecutionPlan, id: &str) -> String {
    match &op(plan, id).reuse {
        OperationReuse::Execute(reason) => reason.message.clone(),
        OperationReuse::Reuse { source } => panic!("{id} is reused from {source}"),
    }
}

fn code(plan: &gaia_plan::ExecutionPlan, id: &str) -> &'static str {
    match &op(plan, id).reuse {
        OperationReuse::Execute(reason) => reason.code,
        OperationReuse::Reuse { source } => panic!("{id} is reused from {source}"),
    }
}

/// Before the bump: the source and the artifact are both recorded as done,
/// with the artifact's inputs recorded from the old tree.
struct Baseline {
    spec: ResolvedBuildSpec,
    state: ReuseState,
    recorded_input: u64,
}

fn baseline(tree_digest: &str) -> Baseline {
    let spec = upstream_spec(OLD_COMMIT);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    write_upstream_state(&spec, OLD_COMMIT, tree_digest);
    write_artifact_output(&spec);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let mut state =
        reuse_state_for_ids(&spec, &plan, &["source:gaia-upstream", "artifact:gaia-app"]);
    let artifact = op(&plan, "artifact:gaia-app");
    let recorded_input = operation_input_signature(&spec, &plan, artifact);
    state
        .operation_input_signatures
        .insert("artifact:gaia-app".into(), recorded_input);
    Baseline {
        spec,
        state,
        recorded_input,
    }
}

fn plan_with(spec: &ResolvedBuildSpec, state: &ReuseState) -> gaia_plan::ExecutionPlan {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(state),
    )
}

#[test]
fn unchanged_source_and_artifact_are_not_invalidated() {
    let base = baseline("sha-tree-1");
    let plan = plan_with(&base.spec, &base.state);

    assert_eq!(code_or_reuse(&plan, "source:gaia-upstream"), "reused");
    assert_eq!(code_or_reuse(&plan, "artifact:gaia-app"), "reused");
    let summary = invalidation_summary(&plan);
    assert!(!summary.direct.contains(&"source:gaia-upstream".to_string()));
    assert!(!summary.cascaded.contains(&"artifact:gaia-app".to_string()));
}

fn code_or_reuse(plan: &gaia_plan::ExecutionPlan, id: &str) -> &'static str {
    match &op(plan, id).reuse {
        OperationReuse::Execute(_) => "executes",
        OperationReuse::Reuse { .. } => "reused",
    }
}

#[test]
fn rev_bump_names_the_rev_and_the_source_it_cascades_from() {
    let base = baseline("sha-tree-1");
    let mut bumped = base.spec.clone();
    set_upstream_commit(&mut bumped, NEW_COMMIT);
    // Not yet re-materialized: the state file still records the old commit.
    let plan = plan_with(&bumped, &base.state);

    assert_eq!(
        code(&plan, "source:gaia-upstream"),
        "operation_fingerprint_mismatch"
    );
    assert_eq!(
        message(&plan, "source:gaia-upstream"),
        "operation 'source:gaia-upstream' will execute because its rev changed: 3175ad9 -> e565dcf"
    );
    assert_eq!(code(&plan, "artifact:gaia-app"), "dependency_rebuilt");
    assert_eq!(
        message(&plan, "artifact:gaia-app"),
        "operation 'artifact:gaia-app' will execute because source:gaia-upstream runs \
         (operation_fingerprint_mismatch); reused instead if the rebuilt inputs turn out unchanged"
    );
    assert_eq!(
        op(&plan, "artifact:gaia-app").cutoff_input_signature,
        Some(base.recorded_input)
    );

    let summary = invalidation_summary(&plan);
    assert!(summary.direct.contains(&"source:gaia-upstream".to_string()));
    assert!(!summary.direct.contains(&"artifact:gaia-app".to_string()));
    assert!(summary.cascaded.contains(&"artifact:gaia-app".to_string()));
    assert!(!summary.direct.iter().any(|id| id == "resolve-build"));
    assert!(!summary.cascaded.iter().any(|id| id == "report:emit"));
}

#[test]
fn rev_bump_with_identical_tree_keeps_dependents_on_their_early_cutoff() {
    let base = baseline("sha-tree-1");
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let mut bumped = base.spec.clone();
    set_upstream_commit(&mut bumped, NEW_COMMIT);
    let before = plan_with(&bumped, &base.state);
    assert_eq!(
        op(&before, "artifact:gaia-app").cutoff_input_signature,
        Some(base.recorded_input)
    );

    // The source re-materializes at the new commit; its checked-out files
    // hash the same, so the dependent's input content is unchanged.
    write_upstream_state(&bumped, NEW_COMMIT, "sha-tree-1");
    let plan = plan_build(&bumped, &source_catalog, &artifact_catalog, &image_catalog);
    let artifact = op(&plan, "artifact:gaia-app");
    assert_eq!(
        operation_input_signature(&bumped, &plan, artifact),
        base.recorded_input,
        "identical tree content keeps the dependent's recorded input"
    );
}

#[test]
fn rev_bump_with_changed_tree_invalidates_dependents() {
    let base = baseline("sha-tree-1");
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let mut bumped = base.spec.clone();
    set_upstream_commit(&mut bumped, NEW_COMMIT);
    write_upstream_state(&bumped, NEW_COMMIT, "sha-tree-2");
    let plan = plan_build(&bumped, &source_catalog, &artifact_catalog, &image_catalog);
    let artifact = op(&plan, "artifact:gaia-app");
    assert_ne!(
        operation_input_signature(&bumped, &plan, artifact),
        base.recorded_input,
        "a real content change moves the dependent's recorded input"
    );
}

#[test]
fn source_content_signature_ignores_the_commit_only_when_a_tree_digest_is_recorded() {
    let spec = upstream_spec(OLD_COMMIT);
    let kind = OperationKind::MaterializeSource {
        source_id: SourceId::new("gaia-upstream"),
    };
    let signature = |commit: &str, digest: Option<&str>| {
        let dir = PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream");
        fs::create_dir_all(&dir).expect("source dir");
        let digest_line = digest
            .map(|digest| format!("materialized_tree_digest={digest}\n"))
            .unwrap_or_default();
        fs::write(
            dir.join(".gaia-source-state.txt"),
            format!("resolved_commit_sha={commit}\n{digest_line}"),
        )
        .expect("state");
        operation_content_signature(&spec, &kind).expect("content signature")
    };

    assert_eq!(
        signature(OLD_COMMIT, Some("tree-a")),
        signature(NEW_COMMIT, Some("tree-a")),
        "a rev bump with the same tree is the same content"
    );
    assert_ne!(
        signature(NEW_COMMIT, Some("tree-a")),
        signature(NEW_COMMIT, Some("tree-b")),
        "a changed tree is a changed content"
    );
    assert_ne!(
        signature(OLD_COMMIT, None),
        signature(NEW_COMMIT, None),
        "without a recorded tree digest the whole state file is the content"
    );
}

#[test]
fn invalidation_summary_of_a_fresh_plan_counts_every_executing_operation_as_direct() {
    let spec = upstream_spec(OLD_COMMIT);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let summary: InvalidationSummary = invalidation_summary(&plan);
    assert!(summary.cascaded.is_empty());
    assert!(summary.direct.contains(&"source:gaia-upstream".to_string()));
    assert!(!summary.direct.contains(&"resolve-build".to_string()));
    assert!(!summary.direct.contains(&"report:emit".to_string()));
}

#[test]
fn prepare_output_signature_ignores_the_shared_collect_state() {
    let spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let config = build_dir.join("image/buildroot-output/.config");
    fs::create_dir_all(config.parent().expect("output dir")).expect("output dir");
    fs::write(&config, "BR2_PACKAGE_FOO=y\n").expect("config");
    let collect = PathBuf::from(
        spec.image
            .output
            .collect_dir
            .clone()
            .expect("image collect dir"),
    );
    fs::create_dir_all(&collect).expect("collect dir");
    let state = collect.join(".gaia-image-state.txt");

    // Prepare's own state, then the state after the image build rewrote the
    // same file on a later run (new archive, new digests and mtimes).
    fs::write(
        &state,
        "provider=image.buildroot\nbackend_mode=buildroot-prepare\ncollect_digest=aaa\nreused=false\n",
    )
    .expect("prepare state");
    let first = operation_output_signature(&spec, &OperationKind::PrepareImage);
    fs::write(
        &state,
        "provider=image.buildroot\nbackend_mode=buildroot\ncollect_digest=bbb\narchive_sha256=ccc\nreused=false\nemit_report=true\n",
    )
    .expect("build state");
    let second = operation_output_signature(&spec, &OperationKind::PrepareImage);
    assert!(first.is_some());
    assert_eq!(
        first, second,
        "identical prepares must give identical signatures"
    );

    // A real change to the prepared tree's configuration does change it.
    fs::write(&config, "BR2_PACKAGE_FOO=n\n").expect("changed config");
    assert_ne!(
        operation_output_signature(&spec, &OperationKind::PrepareImage),
        first
    );
}
