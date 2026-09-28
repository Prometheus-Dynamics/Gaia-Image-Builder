//! Keep-going, batching, CPU budgets, timings and log retention, driven by a
//! scripted artifact provider so the tests are fast and deterministic.

use gaia_artifact_providers::{
    ArtifactBatchItem, ArtifactExecutionContract, ArtifactProvider, ArtifactProviderCatalog,
    ArtifactProviderError, ArtifactProviderErrorKind, ProcessCancelCheck, ProcessLogLine,
    ProcessLogSink, ProcessLogStream,
};
use gaia_exec::{
    ExecutionCancellation, ExecutionEvent, ExecutionOutcome, ExecutionProviders,
    OperationTimingStatus, execute_plan_with_cancellation_and_observer,
};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, OperationParallelism, OperationParallelismDomain,
    PlannedOperation,
};
use gaia_source_providers::SourceProviderCatalog;
use gaia_spec::{
    ArtifactDefinition, ArtifactOutputSpec, ArtifactProviderKind, ArtifactSpec,
    ArtifactVariantSpec, ResolvedBuildSpec, RustArtifactSpec,
};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the scripted provider does for one artifact id.
#[derive(Clone)]
enum Script {
    Succeed,
    Fail,
    /// Succeeds after the delay unless cancelled first.
    Slow(Duration),
    /// Streams this many lines, then succeeds or fails.
    Stream(usize, bool),
}

#[derive(Default)]
struct Journal {
    batches: Vec<Vec<String>>,
    singles: Vec<String>,
    budgets: BTreeMap<String, Option<usize>>,
}

struct ScriptedProvider {
    scripts: BTreeMap<String, Script>,
    batchable: bool,
    journal: Arc<Mutex<Journal>>,
}

impl ScriptedProvider {
    fn run(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        let id = artifact.id.as_str().to_string();
        self.journal
            .lock()
            .expect("journal")
            .budgets
            .insert(id.clone(), contract.job_budget);
        match self.scripts.get(&id).cloned().unwrap_or(Script::Succeed) {
            Script::Succeed => Ok(vec![format!("built {id}")]),
            Script::Fail => Err(ArtifactProviderError::backend_command(format!(
                "{id} exploded"
            ))),
            Script::Slow(delay) => {
                let started = Instant::now();
                while started.elapsed() < delay {
                    if cancel_check.as_ref().is_some_and(|cancel| cancel()) {
                        return Err(ArtifactProviderError::new(
                            ArtifactProviderErrorKind::Cancelled,
                            format!("{id} cancelled"),
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(vec![format!("built {id} slowly")])
            }
            Script::Stream(lines, succeed) => {
                if let Some(sink) = &log_sink {
                    for index in 0..lines {
                        sink(ProcessLogLine {
                            stream: ProcessLogStream::Stdout,
                            line: format!("{id} line {index}"),
                        });
                    }
                }
                if succeed {
                    Ok(vec![format!("built {id}")])
                } else {
                    Err(ArtifactProviderError::backend_command(format!(
                        "{id} failed after output"
                    )))
                }
            }
        }
    }
}

impl ArtifactProvider for ScriptedProvider {
    fn id(&self) -> &'static str {
        "artifact.scripted"
    }

    fn kind(&self) -> ArtifactProviderKind {
        ArtifactProviderKind::Rust
    }

    fn execute_artifact(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        self.journal
            .lock()
            .expect("journal")
            .singles
            .push(artifact.id.as_str().to_string());
        self.run(artifact, contract, log_sink, cancel_check)
    }

    fn batch_key(
        &self,
        _artifact: &ArtifactSpec,
        _contract: &ArtifactExecutionContract,
    ) -> Option<String> {
        self.batchable.then(|| "same-workspace".to_string())
    }

    fn execute_artifact_batch(
        &self,
        items: &[ArtifactBatchItem<'_>],
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Vec<Result<Vec<String>, ArtifactProviderError>> {
        self.journal.lock().expect("journal").batches.push(
            items
                .iter()
                .map(|item| item.artifact.id.as_str().to_string())
                .collect(),
        );
        items
            .iter()
            .map(|item| {
                self.run(
                    item.artifact,
                    item.contract,
                    item.log_sink.clone(),
                    cancel_check.clone(),
                )
            })
            .collect()
    }
}

struct Harness {
    spec: ResolvedBuildSpec,
    plan: ExecutionPlan,
    scripts: BTreeMap<String, Script>,
    batchable: bool,
}

impl Harness {
    /// `artifacts` are `(id, dependencies)`; every artifact builds in the
    /// artifacts parallelism domain.
    fn new(prefix: &str, artifacts: &[(&str, &[&str])]) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir()
            .join("gaia-tests")
            .join(format!("{prefix}-{nonce}"));
        std::fs::create_dir_all(&root).expect("root");
        let mut spec = ResolvedBuildSpec::new(prefix);
        spec.workspace.root_dir = root.display().to_string();
        spec.workspace.build_dir = root.join("build").display().to_string();
        spec.workspace.out_dir = root.join("out").display().to_string();
        spec.policy.execution.jobs = 8;
        let mut operations = Vec::new();
        for (id, dependencies) in artifacts {
            spec.artifacts.push(ArtifactSpec::new(
                *id,
                ArtifactDefinition::Rust(RustArtifactSpec {
                    package: (*id).into(),
                    target_name: None,
                    variant: ArtifactVariantSpec::File,
                    features: Vec::new(),
                    no_default_features: false,
                    all_features: false,
                }),
                None,
                ArtifactOutputSpec {
                    path: root.join("out").join(id).display().to_string(),
                },
            ));
            let mut operation = PlannedOperation::new(
                OperationId::new(format!("artifact:{id}")),
                OperationKind::BuildArtifact {
                    artifact_id: (*id).into(),
                },
            )
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Artifacts,
            ));
            for dependency in *dependencies {
                operation =
                    operation.with_dependency(OperationId::new(format!("artifact:{dependency}")));
            }
            operations.push(operation);
        }
        Self {
            plan: ExecutionPlan {
                build_id: spec.identity.id.clone(),
                operations,
            },
            spec,
            scripts: BTreeMap::new(),
            batchable: false,
        }
    }

    fn script(mut self, id: &str, script: Script) -> Self {
        self.scripts.insert(id.to_string(), script);
        self
    }

    fn run(&self) -> (ExecutionOutcome, Vec<ExecutionEvent>, Arc<Mutex<Journal>>) {
        let journal = Arc::new(Mutex::new(Journal::default()));
        let mut artifact_catalog = ArtifactProviderCatalog::new();
        artifact_catalog.register(Box::new(ScriptedProvider {
            scripts: self.scripts.clone(),
            batchable: self.batchable,
            journal: journal.clone(),
        }));
        let source_catalog = SourceProviderCatalog::new();
        let image_catalog = ImageProviderCatalog::new();
        let (tx, rx) = mpsc::channel();
        let outcome = execute_plan_with_cancellation_and_observer(
            &self.spec,
            &self.plan,
            ExecutionProviders {
                source_catalog: &source_catalog,
                artifact_catalog: &artifact_catalog,
                image_catalog: &image_catalog,
            },
            &ExecutionCancellation::new(),
            Some(tx),
        );
        let observed = rx.try_iter().collect();
        (outcome, observed, journal)
    }
}

fn ids(values: &[OperationId]) -> Vec<&str> {
    values.iter().map(OperationId::as_str).collect()
}

#[test]
fn default_policy_stops_siblings_after_a_failure() {
    let harness = Harness::new("gaia-stop-on-failure", &[("bad", &[]), ("slow", &[])])
        .script("bad", Script::Fail)
        .script("slow", Script::Slow(Duration::from_secs(20)));

    let started = Instant::now();
    let (outcome, _, _) = harness.run();

    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(ids(&outcome.completed_ids), Vec::<&str>::new());
    assert!(outcome.skipped_ids.is_empty());
    assert_eq!(outcome.errors.len(), 1);
}

#[test]
fn keep_going_finishes_independent_work_and_skips_dependents() {
    let mut harness = Harness::new(
        "gaia-keep-going",
        &[
            ("bad", &[]),
            ("slow", &[]),
            ("after-bad", &["bad"]),
            ("after-after-bad", &["after-bad"]),
            ("after-slow", &["slow"]),
        ],
    )
    .script("bad", Script::Fail)
    .script("slow", Script::Slow(Duration::from_millis(300)));
    harness.spec.policy.failure.keep_going = true;

    let (outcome, observed, _) = harness.run();

    assert_eq!(
        {
            let mut completed = ids(&outcome.completed_ids);
            completed.sort();
            completed
        },
        ["artifact:after-slow", "artifact:slow"]
    );
    assert_eq!(
        ids(&outcome
            .errors
            .iter()
            .map(|e| e.operation_id.clone())
            .collect::<Vec<_>>()),
        ["artifact:bad"]
    );
    assert_eq!(
        ids(&outcome.skipped_ids),
        ["artifact:after-bad", "artifact:after-after-bad"]
    );
    // Finished work is kept, not rolled back.
    assert!(outcome.rolled_back_ids.is_empty());
    let skip_reasons = observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Skipped { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(skip_reasons.len(), 2);
    assert!(
        skip_reasons
            .iter()
            .all(|reason| reason.contains("'artifact:bad' failed")),
        "{skip_reasons:?}"
    );
}

#[test]
fn operation_timings_are_recorded_per_operation() {
    let harness = Harness::new("gaia-timings", &[("quick", &[]), ("slow", &["quick"])])
        .script("slow", Script::Slow(Duration::from_millis(150)));

    let (outcome, _, _) = harness.run();

    let timings = outcome
        .operation_timings
        .iter()
        .map(|timing| {
            (
                timing.operation_id.as_str(),
                (timing.duration, timing.status),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(timings.len(), 2);
    let (slow, status) = timings["artifact:slow"];
    assert_eq!(status, OperationTimingStatus::Built);
    assert!(slow >= Duration::from_millis(150), "{slow:?}");
    assert!(timings["artifact:quick"].0 < slow);
}

#[test]
fn streamed_logs_go_live_but_are_not_retained_on_success() {
    let mut harness = Harness::new("gaia-log-retention", &[("chatty", &[]), ("broken", &[])])
        .script("chatty", Script::Stream(5_000, true))
        .script("broken", Script::Stream(500, false));
    harness
        .spec
        .policy
        .execution
        .output_retention
        .failure_tail_lines = 20;
    harness.spec.policy.failure.keep_going = true;

    let (outcome, observed, _) = harness.run();

    let live = observed
        .iter()
        .filter(|event| {
            matches!(event, ExecutionEvent::Log { message, .. } if message.contains(" line "))
        })
        .count();
    assert_eq!(
        live, 5_500,
        "every streamed line reaches the live sink once"
    );
    let retained = outcome
        .events
        .iter()
        .filter(|event| {
            matches!(event, ExecutionEvent::Log { message, .. } if message.contains(" line "))
        })
        .count();
    assert_eq!(retained, 0, "streamed output is not re-emitted or kept");

    let failure = &outcome.errors[0];
    assert_eq!(failure.message, "broken failed after output");
    assert_eq!(failure.output_tail.len(), 20);
    assert_eq!(failure.output_tail[0], "broken line 481");
    assert_eq!(
        failure.output_tail.last().map(String::as_str),
        Some("broken failed after output")
    );
}

#[test]
fn compatible_artifacts_build_in_one_batch_with_per_operation_results() {
    let mut harness = Harness::new(
        "gaia-batch",
        &[
            ("one", &[]),
            ("two", &[]),
            ("three", &[]),
            ("later", &["one"]),
        ],
    );
    harness.batchable = true;

    let (outcome, observed, journal) = harness.run();

    let journal = journal.lock().expect("journal");
    assert_eq!(journal.batches, vec![vec!["one", "two", "three"]]);
    assert_eq!(journal.singles, vec!["later"]);
    assert_eq!(outcome.completed_ids.len(), 4);
    assert!(outcome.errors.is_empty());
    for id in ["one", "two", "three"] {
        let operation_id = format!("artifact:{id}");
        assert!(observed.iter().any(|event| matches!(
            event,
            ExecutionEvent::Succeeded { operation_id: done } if done.as_str() == operation_id
        )));
    }
}

#[test]
fn batching_can_be_disabled_by_policy() {
    let mut harness = Harness::new("gaia-batch-off", &[("one", &[]), ("two", &[])]);
    harness.batchable = true;
    harness.spec.policy.providers.rust.batch_builds = false;

    let (outcome, _, journal) = harness.run();

    let journal = journal.lock().expect("journal");
    assert!(journal.batches.is_empty());
    assert_eq!(journal.singles.len(), 2);
    assert_eq!(outcome.completed_ids.len(), 2);
}

#[test]
fn concurrent_heavy_builds_split_the_cpu_budget() {
    let harness = Harness::new("gaia-budget", &[("left", &[]), ("right", &[])])
        .script("left", Script::Slow(Duration::from_millis(100)))
        .script("right", Script::Slow(Duration::from_millis(100)));

    let (_, _, journal) = harness.run();

    let cores = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let expected = Some((cores / 2).max(1));
    let journal = journal.lock().expect("journal");
    assert_eq!(journal.budgets["left"], expected);
    assert_eq!(journal.budgets["right"], expected);
}

#[test]
fn a_heavy_build_running_alone_keeps_tool_defaults() {
    let mut harness = Harness::new(
        "gaia-budget-alone",
        &[("first", &[]), ("second", &["first"])],
    );
    harness.spec.policy.execution.jobs = 8;

    let (_, _, journal) = harness.run();

    let journal = journal.lock().expect("journal");
    assert_eq!(journal.budgets["first"], None);
    assert_eq!(journal.budgets["second"], None);
}
