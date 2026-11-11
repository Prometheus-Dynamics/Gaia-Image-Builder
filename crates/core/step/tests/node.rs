//! A step written as a Daedalus node, run by a Daedalus host: a tool that
//! succeeds, one that exits 3, and one cancelled mid-run.

use daedalus::prelude::*;
use daedalus_cpu::CpuBackend;
use gaia_step::{Ctx, Digest, KeyBuilder, ObjectRef, Step, StepError, Store, Tool};
use std::path::Path;
use std::time::{Duration, Instant};

#[node(id = "test.tool", inputs(store, script), outputs(out))]
fn tool(store: &str, script: &str, cx: &Ctx) -> Result<String, StepError> {
    const NAME: &str = "test.tool";
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let key = KeyBuilder::new(NAME, 1).config("script", script).finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let out = work.dir().join("out");
            work.run(
                &Tool::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .env("PATH", "/usr/bin:/bin")
                    .env("OUT", out),
            )?;
            Ok(())
        })?;
    Ok(made.outputs["out"].to_string())
}

fn load(store: &Path, script: &str) -> LoadedHost {
    let text = format!(
        r#"{{
  "format": "daedalus.graph",
  "schema_version": 1,
  "graph": {{
    "nodes": [
      {{"id": "host", "kind": "host_bridge", "port_types": {{"out": "string"}}}},
      {{"id": "test.tool", "consts": {{
        "store": {{"type": "str", "value": {store}}},
        "script": {{"type": "str", "value": {script}}}
      }}}}
    ],
    "edges": [
      {{"from": {{"node": 1, "port": "out"}}, "to": {{"node": 0, "port": "out"}}}}
    ]
  }}
}}"#,
        store = json_string(&store.display().to_string()),
        script = json_string(script),
    );
    let graph = GraphDoc::parse(&text).expect("the graph parses").graph;
    let mut registry = Registry::new();
    registry.register_all(&[&TOOL_NODE]).expect("registers");
    let planned = plan(&graph, &registry, &[&CpuBackend], &Config::default()).expect("plans");
    LoadedHost::load(planned, LoadOptions::default()).expect("loads")
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn faults(host: &mut LoadedHost) -> Vec<(Option<String>, u32, String)> {
    host.fault_reports()
        .map(|report| {
            (
                report.node().map(str::to_string),
                report.consecutive,
                report.reason().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_tool_that_prints_and_exits_0_makes_its_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_root = dir.path().join("store");
    let mut host = load(
        &store_root,
        r#"echo building; echo "a warning" >&2; printf hi > "$OUT""#,
    );
    host.step().expect("tick");
    let out = host
        .host()
        .take::<String>("out")
        .expect("take")
        .expect("an output");
    let object: ObjectRef = out.parse().expect("an object reference");
    let store = Store::open(&store_root).expect("store");
    assert_eq!(object.digest, Digest::of_bytes(b"hi"));
    assert_eq!(std::fs::read(store.path(&object)).expect("read"), b"hi");
    assert!(faults(&mut host).is_empty());
    println!("exit 0: output {out}");
}

#[test]
fn a_tool_that_exits_3_is_a_fault_naming_the_step_with_its_output_in_a_run_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_root = dir.path().join("store");
    let mut host = load(
        &store_root,
        "echo 'checking things' >&2; echo 'cannot find libfoo' >&2; exit 3",
    );
    host.step().expect("tick");
    assert!(host.host().take::<String>("out").expect("take").is_none());
    let faults = faults(&mut host);
    println!("exit 3: fault record {faults:?}");
    assert_eq!(faults.len(), 1, "{faults:?}");
    let (node, consecutive, reason) = &faults[0];
    assert_eq!(node.as_deref(), Some("test.tool#0"));
    assert_eq!(*consecutive, 1);
    assert_eq!(reason, "test.tool: exited 3");

    // The run log, next to the store, holds what the tool printed.
    let logs = std::fs::read_dir(dir.path().join("logs/test.tool"))
        .expect("logs")
        .map(|entry| std::fs::read_to_string(entry.expect("entry").path()).expect("log"))
        .collect::<Vec<_>>();
    assert_eq!(logs.len(), 1);
    assert!(
        logs[0].contains("err| cannot find libfoo\n# end "),
        "{}",
        logs[0]
    );

    // Nothing was committed: no entry, no object, no scratch.
    for sub in ["entries", "objects/file", "objects/tree", "scratch"] {
        let count = std::fs::read_dir(store_root.join(sub)).expect("ls").count();
        assert_eq!(count, 0, "{sub}");
    }
}

#[test]
fn a_cancelled_tool_loses_its_whole_process_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_root = dir.path().join("store");
    let pids = dir.path().join("pids");
    // The shell starts a grandchild, records both pids, and waits.
    let script = format!(
        "sleep 300 & echo $! > {pids}.tmp; echo $$ >> {pids}.tmp; mv {pids}.tmp {pids}; wait",
        pids = pids.display()
    );
    let mut host = load(&store_root, &script);
    let cancel = host.cancel_handle();
    let watcher = {
        let pids = pids.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !pids.exists() {
                assert!(Instant::now() < deadline, "the tool never started");
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.cancel();
            Instant::now()
        })
    };
    host.step().expect("tick");
    let returned = Instant::now();
    let cancelled = watcher.join().expect("watcher");
    println!(
        "cancel: step returned {:?} after the cancel",
        returned.duration_since(cancelled)
    );
    assert!(returned.duration_since(cancelled) < Duration::from_secs(5));

    let pids: Vec<i32> = std::fs::read_to_string(&pids)
        .expect("pids")
        .split_whitespace()
        .map(|pid| pid.parse().expect("a pid"))
        .collect();
    assert_eq!(pids.len(), 2);
    // The shell is the group leader: its pid is the group id.
    let group = rustix::process::Pid::from_raw(pids[1]).expect("pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Gone, not even a zombie: no /proc entry for either.
        let gone = pids
            .iter()
            .all(|pid| !Path::new(&format!("/proc/{pid}")).exists());
        if gone {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a process of the group is left: {pids:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        rustix::process::test_kill_process_group(group).is_err(),
        "the process group still exists"
    );
    let faults = faults(&mut host);
    println!("cancel: fault record {faults:?}");
    assert_eq!(faults.len(), 1);
    assert!(
        faults[0].2.starts_with("test.tool: cancelled"),
        "{faults:?}"
    );
    assert_eq!(
        std::fs::read_dir(store_root.join("scratch"))
            .expect("ls")
            .count(),
        0
    );
}
