//! The A2 proof: two chains in parallel, fail-fast, and what a loaded host
//! does with a step that fails twice.

use daedalus::runner::{Recovery, RunError};
use gaia_proof::{Chain, Setup, Stopped, digest_of, graph, load, run, show, take_tar, timeline};
use std::path::{Path, PathBuf};

const URL: &str = "https://github.com/vasilakisfil/hello.service";
/// A two-file crate with no dependencies, pinned.
const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";

fn setup(root: &Path) -> Setup {
    let sysroot = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc --print sysroot");
    Setup {
        store: root.join("store"),
        git: PathBuf::from("/usr/bin/git"),
        toolchain: PathBuf::from(String::from_utf8(sysroot.stdout).expect("utf-8").trim()),
    }
}

fn chain(label: &str, url: &str, profile: &str, path: &str) -> Chain {
    Chain {
        label: label.into(),
        url: url.into(),
        commit: COMMIT.into(),
        profile: profile.into(),
        bin: "hello-world".into(),
        path: path.into(),
    }
}

fn chains() -> Vec<Chain> {
    vec![
        chain("a", URL, "release", "usr/bin/hello"),
        chain("b", URL, "dev", "usr/lib/debug/hello"),
    ]
}

/// Runs both chains in a fresh store; returns the two tar digests and the
/// timeline.
fn run_once(root: &Path) -> (String, String, String) {
    let setup = setup(root);
    let mut loaded = load(&graph(&setup, &chains())).expect("loads");
    run(&mut loaded, 4, true).expect("the run succeeds");
    assert_eq!(loaded.fault_reports().count(), 0);
    let a = take_tar(&mut loaded, "a").expect("chain a's tar");
    let b = take_tar(&mut loaded, "b").expect("chain b's tar");
    let spans = timeline(&root.join("logs"));
    let builds: Vec<_> = spans
        .iter()
        .filter(|span| span.step == "gaia.proof.cargo")
        .collect();
    assert_eq!(builds.len(), 2, "{}", show(&spans));
    let overlap = builds[0].start_ns < builds[1].end_ns && builds[1].start_ns < builds[0].end_ns;
    assert!(overlap, "the two builds did not overlap:\n{}", show(&spans));
    (
        digest_of(&a).expect("digest").to_string(),
        digest_of(&b).expect("digest").to_string(),
        show(&spans),
    )
}

#[test]
fn two_chains_run_at_the_same_time_and_pack_the_same_tar_twice() {
    let one = tempfile::tempdir().expect("tempdir");
    let two = tempfile::tempdir().expect("tempdir");
    let (a1, b1, timeline_one) = run_once(one.path());
    let (a2, b2, _) = run_once(&two.path().join("elsewhere"));
    println!("timeline, run 1:\n{timeline_one}");
    println!("chain a tar: run 1 {a1}\n             run 2 {a2}");
    println!("chain b tar: run 1 {b1}\n             run 2 {b2}");
    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    assert_ne!(a1, b1);
}

#[test]
fn a_forced_failure_stops_the_build_and_the_other_chain() {
    let root = tempfile::tempdir().expect("tempdir");
    let setup = setup(root.path());
    let broken = chain("broken", "file:///nonexistent/repository", "release", "x");
    let good = chain("good", URL, "release", "usr/bin/hello");
    let mut loaded = load(&graph(&setup, &[good, broken])).expect("loads");
    let stopped = run(&mut loaded, 4, true).expect_err("the run stops");
    println!("run stopped: {stopped:?}");
    assert!(matches!(stopped, Stopped::Run(RunError::Fault { .. })));
    let faults: Vec<_> = loaded
        .fault_reports()
        .map(|report| {
            (
                report.node().map(str::to_string),
                report.reason().to_string(),
            )
        })
        .collect();
    println!("fault record: {faults:?}");
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].1, "gaia.proof.git: exited 128");
    assert!(take_tar(&mut loaded, "good").is_none());
    assert!(take_tar(&mut loaded, "broken").is_none());

    let spans = timeline(&root.path().join("logs"));
    println!("timeline:\n{}", show(&spans));
    // The good chain's clone was already running and finished; nothing
    // after it started.
    assert!(
        spans.iter().all(|span| span.step == "gaia.proof.git"),
        "{}",
        show(&spans)
    );
    // The failing step's run log says why.
    let failed = spans
        .iter()
        .find(|span| span.status == "exit 128")
        .expect("the failed run");
    println!("failed run: {failed:?}");
}

mod flaky {
    use daedalus::prelude::*;
    use daedalus_cpu_author::Fault;

    #[derive(Default)]
    pub struct Calls(pub u32);

    /// Fails on its first two calls, then passes its input on.
    #[node(id = "test.flaky", inputs(x), outputs(y), state(Calls))]
    pub fn flaky(x: i64, calls: &mut Calls) -> Result<i64, Fault> {
        calls.0 += 1;
        if calls.0 <= 2 {
            return Err(Fault {
                code: 1,
                reason: "flaky: not yet",
            });
        }
        Ok(x)
    }
}

/// `Recovery::Retry { max: 3 }` cannot be given to a step of a loaded graph
/// at Daedalus 9d8ed136: every step is loaded with `Recovery::Fail`. This
/// records what a loaded host does instead.
#[test]
fn a_step_that_fails_twice_on_a_loaded_host() {
    use daedalus::prelude::*;
    use std::sync::Arc;
    let text = r#"{"format": "daedalus.graph", "schema_version": 1, "graph": {
        "nodes": [{"id": "host", "kind": "host_bridge", "port_types": {"x": "i64", "y": "i64"}},
                  {"id": "test.flaky"}],
        "edges": [{"from": {"node": 0, "port": "x"}, "to": {"node": 1, "port": "x"}},
                  {"from": {"node": 1, "port": "y"}, "to": {"node": 0, "port": "y"}}]}}"#;
    let graph = GraphDoc::parse(text).expect("parses").graph;
    let mut registry = Registry::new();
    registry
        .register_all(&[&flaky::FLAKY_NODE])
        .expect("registers");
    let planned = plan(
        &graph,
        &registry,
        &[&daedalus_cpu::CpuBackend],
        &Config::default(),
    )
    .expect("plans");
    let mut loaded = LoadedHost::load(planned, LoadOptions::default()).expect("loads");
    let runner = loaded.host().runner();
    let steps = (0..)
        .map_while(|step| runner.step_meta(step))
        .map(|meta| meta.recovery)
        .collect::<Vec<_>>();
    println!("recovery of each step: {steps:?}");
    assert!(steps.iter().all(|recovery| *recovery == Recovery::Fail));

    loaded
        .host()
        .feed_typed("x", Arc::new(7_i64))
        .expect("feed");
    let mut seen = Vec::new();
    for tick in 1..=4 {
        let report = loaded.host().step().expect("tick");
        let out = loaded.host().take_copy::<i64>("y").expect("take");
        seen.push((tick, report.faults, report.retries, out));
    }
    println!("(tick, faults, retries, output) after one feed: {seen:?}");
    let faults: Vec<_> = loaded
        .fault_reports()
        .map(|report| (report.consecutive, report.reason().to_string()))
        .collect();
    println!("fault record: {faults:?}");
    // One call, one fault, and nothing after: the input was consumed by the
    // failed call, so no later tick calls the step again. With
    // `Recovery::Retry { max: 3 }` it would have succeeded on its third call
    // in tick 1.
    assert_eq!(
        seen,
        [
            (1, 1, 0, None),
            (2, 0, 0, None),
            (3, 0, 0, None),
            (4, 0, 0, None)
        ]
    );
    assert_eq!(faults, [(1, "flaky: not yet".to_string())]);
}
