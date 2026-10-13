//! An artifact that reads a source besides its own (`sources = [...]`, or a
//! `${source.<id>.*}` token in its build fields) depends on that source, so a
//! change to the source rebuilds the artifact.

pub mod support;

use gaia_plan::{
    OperationId, OperationReuse, ReuseState, invalidation_summary, operation_components,
    operation_input_signature, plan_build, plan_build_with_reuse_state,
};
use gaia_spec::{
    ResolvedBuildSpec, SourceDefinition, SourceId, SourcePinPolicySpec, SourceRef,
    SourceRefreshPolicySpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, reuse_state_for_ids, test_spec};

const OLD_COMMIT: &str = "3175ad9c0ffee00000000000000000000000001a";
const NEW_COMMIT: &str = "e565dcf0ddba11000000000000000000000000b2";

/// `gaia-app` builds from `gaia-upstream` and also reads `eidos`, a second
/// git source, through `extra_sources`.
fn spec_with_extra_source(scratch: &TempDir, commit: &str) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let mut upstream = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "gaia-upstream")
        .cloned()
        .expect("gaia-upstream source");
    upstream.id = SourceId::new("eidos");
    spec.sources.push(upstream);
    set_commit(&mut spec, "gaia-upstream", commit);
    set_commit(&mut spec, "eidos", commit);
    spec.artifacts[0].source = Some(SourceRef::new("gaia-upstream"));
    spec.artifacts[0].extra_sources = vec![SourceId::new("eidos")];
    spec
}

fn set_commit(spec: &mut ResolvedBuildSpec, id: &str, commit: &str) {
    let source = spec
        .sources
        .iter_mut()
        .find(|source| source.id.as_str() == id)
        .expect("source");
    let SourceDefinition::Git(git) = &mut source.definition else {
        panic!("{id} is a git source");
    };
    git.refresh_policy = SourceRefreshPolicySpec::Never;
    git.pin_policy = SourcePinPolicySpec::Locked;
    git.locked_commit = Some(commit.to_string());
}

/// What the last materialization of `id` left on disk.
fn write_source_state(spec: &ResolvedBuildSpec, id: &str, commit: &str, tree_digest: &str) {
    let dir = PathBuf::from(&spec.workspace.build_dir).join(format!("sources/{id}"));
    fs::create_dir_all(&dir).expect("source dir");
    fs::write(dir.join("source.txt"), "ok").expect("source marker");
    fs::write(
        dir.join(".gaia-source-state.txt"),
        format!(
            "provider=source.git\nsource={id}\nresolved_mode=locked\n\
             resolved_commit_sha={commit}\nmaterialized_tree_digest={tree_digest}\n"
        ),
    )
    .expect("source state");
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

struct Baseline {
    spec: ResolvedBuildSpec,
    state: ReuseState,
    recorded_input: u64,
}

/// Before the bump: both sources and the artifact recorded as done, the
/// artifact's inputs recorded from the old trees.
fn baseline(scratch: &TempDir, eidos_tree: &str) -> Baseline {
    let spec = spec_with_extra_source(scratch, OLD_COMMIT);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    write_source_state(&spec, "gaia-upstream", OLD_COMMIT, "sha-upstream-tree");
    write_source_state(&spec, "eidos", OLD_COMMIT, eidos_tree);
    write_artifact_output(&spec);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let mut state = reuse_state_for_ids(
        &spec,
        &plan,
        &["source:gaia-upstream", "source:eidos", "artifact:gaia-app"],
    );
    let recorded_input = operation_input_signature(&spec, &plan, op(&plan, "artifact:gaia-app"));
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
fn artifact_depends_on_each_extra_source_once() {
    let scratch = TempDir::new("gaia-plan-test");
    let mut spec = spec_with_extra_source(&scratch, OLD_COMMIT);
    // Naming the artifact's own source adds no second edge to it.
    spec.artifacts[0].extra_sources = vec![SourceId::new("eidos"), SourceId::new("gaia-upstream")];
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let depends_on = &op(&plan, "artifact:gaia-app").depends_on;
    let count = |id: &str| depends_on.iter().filter(|dep| dep.as_str() == id).count();
    assert_eq!(count("source:eidos"), 1);
    assert_eq!(count("source:gaia-upstream"), 1);
}

#[test]
fn components_name_the_extra_sources_only_when_there_are_some() {
    let scratch = TempDir::new("gaia-plan-test");
    let spec = spec_with_extra_source(&scratch, OLD_COMMIT);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let named = |spec: &ResolvedBuildSpec| {
        let plan = plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog);
        operation_components(spec, &plan, op(&plan, "artifact:gaia-app"))
            .into_iter()
            .find(|(name, _)| name == "extra sources")
    };
    assert!(named(&spec).is_some(), "extra sources are a named input");
    let mut plain = spec.clone();
    plain.artifacts[0].extra_sources.clear();
    assert!(named(&plain).is_none());
    assert!(!operation_components(&plain, &plan, op(&plan, "artifact:gaia-app")).is_empty());
}

#[test]
fn artifact_without_extra_sources_has_no_edge_to_other_sources() {
    let scratch = TempDir::new("gaia-plan-test");
    let mut spec = spec_with_extra_source(&scratch, OLD_COMMIT);
    spec.artifacts[0].extra_sources.clear();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    assert!(
        !op(&plan, "artifact:gaia-app")
            .depends_on
            .contains(&OperationId::source(&SourceId::new("eidos")))
    );
}

#[test]
fn extra_source_rev_bump_reruns_the_source_and_its_artifact() {
    let scratch = TempDir::new("gaia-plan-test");
    let base = baseline(&scratch, "sha-eidos-tree");
    let mut bumped = base.spec.clone();
    set_commit(&mut bumped, "eidos", NEW_COMMIT);
    // Not yet re-materialized: the state still records the old commit.
    let plan = plan_with(&bumped, &base.state);

    assert_eq!(
        code(&plan, "source:eidos"),
        "operation_fingerprint_mismatch"
    );
    assert_eq!(code(&plan, "artifact:gaia-app"), "dependency_rebuilt");
    assert_eq!(
        message(&plan, "artifact:gaia-app"),
        "operation 'artifact:gaia-app' will execute because source:eidos runs \
         (operation_fingerprint_mismatch); reused instead if the rebuilt inputs turn out unchanged"
    );
    assert_eq!(
        op(&plan, "artifact:gaia-app").cutoff_input_signature,
        Some(base.recorded_input)
    );
    let summary = invalidation_summary(&plan);
    assert!(summary.cascaded.contains(&"artifact:gaia-app".to_string()));
}

#[test]
fn extra_source_rev_bump_with_identical_tree_keeps_the_artifact_on_its_cutoff() {
    let scratch = TempDir::new("gaia-plan-test");
    let base = baseline(&scratch, "sha-eidos-tree");
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let mut bumped = base.spec.clone();
    set_commit(&mut bumped, "eidos", NEW_COMMIT);
    write_source_state(&bumped, "eidos", NEW_COMMIT, "sha-eidos-tree");
    let plan = plan_build(&bumped, &source_catalog, &artifact_catalog, &image_catalog);
    assert_eq!(
        operation_input_signature(&bumped, &plan, op(&plan, "artifact:gaia-app")),
        base.recorded_input,
        "the artifact reads eidos' files, which hash the same"
    );
}

#[test]
fn extra_source_content_change_moves_the_artifacts_input_signature() {
    let scratch = TempDir::new("gaia-plan-test");
    let base = baseline(&scratch, "sha-eidos-tree");
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let mut bumped = base.spec.clone();
    set_commit(&mut bumped, "eidos", NEW_COMMIT);
    write_source_state(&bumped, "eidos", NEW_COMMIT, "sha-eidos-tree-2");
    let plan = plan_build(&bumped, &source_catalog, &artifact_catalog, &image_catalog);
    assert_ne!(
        operation_input_signature(&bumped, &plan, op(&plan, "artifact:gaia-app")),
        base.recorded_input,
        "a changed eidos tree is a changed input of the artifact"
    );
}
