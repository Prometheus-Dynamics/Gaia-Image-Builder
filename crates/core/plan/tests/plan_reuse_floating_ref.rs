//! A git source that tracks a floating branch (refresh `auto`) is resolved
//! at plan time with `git ls-remote`. An unchanged branch keeps the source
//! and what was built from it; a moved branch rebuilds both; a branch that
//! cannot be resolved materializes the source and says why.

pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, PlannedOperation, ReuseState, plan_build_with_reuse_state,
};
use gaia_spec::{
    ResolvedBuildSpec, SourceDefinition, SourceId, SourcePinPolicySpec, SourceRef,
    SourceRefreshPolicySpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{provider_catalogs, reuse_state_for_ids, test_spec};

const SOURCE: &str = "source:eidos";
const ARTIFACT: &str = "artifact:gaia-app";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Gaia Test",
            "-c",
            "user.email=gaia-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare repository `eidos-remote.git` with one commit on `main`, and the
/// working clone that makes further commits.
fn init_remote(scratch: &TempDir) -> (PathBuf, PathBuf) {
    let remote = scratch.join("eidos-remote.git");
    let work = scratch.join("eidos-work");
    fs::create_dir_all(&remote).expect("remote dir");
    git(&remote, &["init", "--bare", "-b", "main"]);
    fs::create_dir_all(&work).expect("work dir");
    git(&work, &["init", "-b", "main"]);
    (remote, work)
}

/// Commits `contents` to `src/lib.rs` on the working clone and pushes `main`.
fn push_commit(remote: &Path, work: &Path, contents: &str) -> String {
    fs::create_dir_all(work.join("src")).expect("src dir");
    fs::write(work.join("src/lib.rs"), contents).expect("lib.rs");
    git(work, &["add", "-A"]);
    git(work, &["commit", "-m", contents]);
    git(work, &["push", &remote.display().to_string(), "HEAD:main"]);
    git(work, &["rev-parse", "HEAD"])
}

/// The default test build, with `eidos` as the floating remote git source
/// that `gaia-app` builds from.
fn floating_spec(scratch: &TempDir, remote: &Path) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    spec.policy.providers.git.allow_remote_resolution = true;
    let mut eidos = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "gaia-upstream")
        .cloned()
        .expect("gaia-upstream source");
    eidos.id = SourceId::new("eidos");
    let SourceDefinition::Git(git) = &mut eidos.definition else {
        panic!("gaia-upstream is a git source");
    };
    git.repo = format!("file://{}", remote.display());
    git.branch = Some("main".into());
    git.tag = None;
    git.rev = None;
    git.locked_commit = None;
    git.refresh_policy = SourceRefreshPolicySpec::Auto;
    git.pin_policy = SourcePinPolicySpec::Floating;
    spec.sources.push(eidos);
    spec.artifacts[0].source = Some(SourceRef::new("eidos"));
    spec
}

/// What a materialization of `eidos` at `commit` leaves on disk.
fn record_materialization(spec: &ResolvedBuildSpec, commit: &str) {
    let dir = PathBuf::from(&spec.workspace.build_dir).join("sources/eidos");
    fs::create_dir_all(&dir).expect("eidos source dir");
    fs::write(dir.join("source.txt"), "ok").expect("eidos marker");
    fs::write(
        dir.join(".gaia-source-state.txt"),
        format!(
            "provider=source.git\nsource=eidos\nselected_ref_type=branch\n\
             selected_ref_value=main\nresolved_mode=remote-clone\n\
             resolved_commit_sha={commit}\n"
        ),
    )
    .expect("eidos state");
}

fn write_artifact_output(spec: &ResolvedBuildSpec) {
    let output = PathBuf::from(&spec.artifacts[0].output.path);
    fs::create_dir_all(output.parent().expect("artifact parent")).expect("artifact dir");
    fs::write(output, "artifact").expect("artifact output");
}

fn op<'plan>(plan: &'plan ExecutionPlan, id: &str) -> &'plan PlannedOperation {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} is in the plan"))
}

fn reuse_message(operation: &PlannedOperation) -> Option<&str> {
    match &operation.reuse {
        OperationReuse::Reuse { source } => Some(source.as_str()),
        OperationReuse::Execute(_) => None,
    }
}

fn execute_code(operation: &PlannedOperation) -> Option<(&'static str, &str)> {
    match &operation.reuse {
        OperationReuse::Execute(reason) => Some((reason.code, reason.message.as_str())),
        OperationReuse::Reuse { .. } => None,
    }
}

fn plan(spec: &ResolvedBuildSpec, state: Option<&ReuseState>) -> ExecutionPlan {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        state,
    )
}

#[test]
fn first_plan_materializes_floating_remote_source() {
    let scratch = TempDir::new("gaia-plan-floating-first");
    let (remote, work) = init_remote(&scratch);
    push_commit(&remote, &work, "v1");
    let spec = floating_spec(&scratch, &remote);

    let plan = plan(&spec, None);

    let (code, _) = execute_code(op(&plan, SOURCE)).expect("source executes");
    assert_eq!(code, "source_materialization_required");
}

#[test]
fn unchanged_floating_ref_reuses_source_and_its_artifact() {
    let scratch = TempDir::new("gaia-plan-floating-same");
    let (remote, work) = init_remote(&scratch);
    let commit = push_commit(&remote, &work, "v1");
    let spec = floating_spec(&scratch, &remote);
    let baseline = plan(&spec, None);

    record_materialization(&spec, &commit);
    write_artifact_output(&spec);
    let state = reuse_state_for_ids(&spec, &baseline, &[SOURCE, ARTIFACT]);
    let plan = plan(&spec, Some(&state));

    let note = reuse_message(op(&plan, SOURCE)).expect("source is reused");
    assert_eq!(
        note,
        format!("floating ref 'main' still at {}", &commit[..7])
    );
    assert!(
        reuse_message(op(&plan, ARTIFACT)).is_some(),
        "artifact built from the unchanged source is reused: {:?}",
        op(&plan, ARTIFACT).reuse
    );
}

#[test]
fn moved_floating_ref_materializes_source_and_rebuilds_artifact() {
    let scratch = TempDir::new("gaia-plan-floating-moved");
    let (remote, work) = init_remote(&scratch);
    let first = push_commit(&remote, &work, "v1");
    let spec = floating_spec(&scratch, &remote);
    let baseline = plan(&spec, None);

    record_materialization(&spec, &first);
    write_artifact_output(&spec);
    let state = reuse_state_for_ids(&spec, &baseline, &[SOURCE, ARTIFACT]);

    let second = push_commit(&remote, &work, "v2");
    let moved = plan(&spec, Some(&state));

    let (code, message) = execute_code(op(&moved, SOURCE)).expect("source executes");
    assert_eq!(code, "remote_floating_source");
    assert!(
        message.contains(&format!("moved from {} to {}", &first[..7], &second[..7])),
        "message names both commits: {message}"
    );
    let (code, _) = execute_code(op(&moved, ARTIFACT)).expect("artifact executes");
    assert_eq!(code, "dependency_rebuilt");

    let after = plan(&spec, None);
    assert_ne!(
        op(&baseline, SOURCE).fingerprint,
        op(&after, SOURCE).fingerprint,
        "the resolved commit is part of the source fingerprint"
    );
}

#[test]
fn unresolvable_floating_ref_materializes_with_a_note() {
    let scratch = TempDir::new("gaia-plan-floating-offline");
    let (remote, work) = init_remote(&scratch);
    let commit = push_commit(&remote, &work, "v1");
    let spec = floating_spec(&scratch, &remote);
    let baseline = plan(&spec, None);

    record_materialization(&spec, &commit);
    write_artifact_output(&spec);
    let state = reuse_state_for_ids(&spec, &baseline, &[SOURCE, ARTIFACT]);

    fs::rename(&remote, scratch.join("eidos-remote-away.git")).expect("remote moved away");
    let plan = plan(&spec, Some(&state));

    let (code, message) = execute_code(op(&plan, SOURCE)).expect("source executes");
    assert_eq!(code, "remote_floating_source");
    assert!(
        message.contains("could not be resolved"),
        "message says the ref was unresolvable: {message}"
    );
}
