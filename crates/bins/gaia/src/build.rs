//! `gaia build`: plan, run on Daedalus's lanes, take the outputs, record
//! the run.

use crate::project::Project;
use crate::record::{self, Origin, Output, Run};
use crate::timing;
use crate::{CANCELLED, FAILED, Stop, args};
use daedalus::prelude::{LoadOptions, LoadedHost};
use daedalus::runner::{FailurePolicy, Lanes, Pool, RunError};
use gaia_progress::{Reader, Watch};
use gaia_step::{logs_dir, now_ns, step_records};
use gaia_store::{ObjectRef, Store};
use gaia_types::{Artifact, Image, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);
static HANDLER: AtomicBool = AtomicBool::new(false);
static CANCEL: Mutex<Option<daedalus::runner::owned_run::CancelHandle>> = Mutex::new(None);

pub fn build(options: &args::Project) -> Result<(), Stop> {
    let dev_run = build_one(options)?;
    if !options.verify {
        return Ok(());
    }
    let mut clean = options.clone();
    clean.dev_reuse = false;
    clean.verify = false;
    eprintln!("gaia: --verify: building the same product clean, to compare");
    let clean_run = build_one(&clean)?;
    let project = Project::open(&clean)?;
    let store = Store::open(&project.setup.store).map_err(Stop::usage)?;
    let lines = crate::dev::compare(&store, &dev_run, &clean_run);
    if lines.is_empty() {
        println!("verify: the dev build equals the clean build");
        return Ok(());
    }
    println!(
        "verify: {} difference(s) between the dev and the clean build:",
        lines.len()
    );
    for line in &lines {
        println!("  {line}");
    }
    Err(Stop {
        code: FAILED,
        message: "verify: the dev build differs from the clean build: a stale reuse. \
                  Two clean builds of the same inputs are equal in all of these."
            .into(),
    })
}

/// One build of `options`; its record.
fn build_one(options: &args::Project) -> Result<Run, Stop> {
    let project = Project::open(options)?;
    if options.dev_reuse {
        let doc = serde_json::to_value(&project.built.doc).map_err(Stop::usage)?;
        crate::dev::check(&doc)?;
        crate::dev::check_erofs(&project.built.doc, &project.dev_erofs)?;
        for note in project
            .built
            .notes
            .iter()
            .filter(|note| note.starts_with("dev erofs:"))
        {
            eprintln!("{note}");
        }
    }
    let planned = project.plan()?;
    let store = Store::open(&project.setup.store).map_err(Stop::usage)?;
    let mut loaded = LoadedHost::load(planned, LoadOptions::default()).map_err(Stop::usage)?;

    // Values a graph constant can't carry yet (X7), fed before the run.
    for (port, input) in &project.built.inputs {
        let fed = match input {
            gaia_graph::HostInput::Control(control) => {
                loaded.host().feed_typed(port, Arc::new(control.clone()))
            }
            gaia_graph::HostInput::Disk(layout) => {
                loaded.host().feed_typed(port, Arc::new(layout.clone()))
            }
            gaia_graph::HostInput::Os(config) => {
                loaded.host().feed_typed(port, Arc::new(config.clone()))
            }
            gaia_graph::HostInput::Buildroot(host) => {
                loaded.host().feed_typed(port, Arc::new(host.clone()))
            }
            gaia_graph::HostInput::Placements(placements) => {
                loaded.host().feed_typed(port, Arc::new(placements.clone()))
            }
        };
        fed.map_err(|e| Stop::usage(format!("feeding {port}: {e}")))?;
    }

    let cancel = loaded.cancel_handle();
    // A second build of one process (--verify) has its own graph: the
    // handler cancels whichever is running.
    *CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel);
    if !HANDLER.swap(true, Ordering::SeqCst) {
        ctrlc::set_handler(move || {
            if INTERRUPTS.fetch_add(1, Ordering::SeqCst) > 0 {
                std::process::exit(i32::from(CANCELLED));
            }
            eprintln!("gaia: stopping at the next step boundary (Ctrl-C again quits now)");
            if let Some(cancel) = CANCEL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                cancel.cancel();
            }
        })
        .map_err(Stop::usage)?;
    }

    let jobs = options.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    });
    let start_ns = now_ns();
    // The record exists from the start, with this process and no outcome, so
    // a build that dies leaves a record that says so.
    let product = std::path::absolute(&project.product).unwrap_or(project.product.clone());
    let mut run = Run::started(product, u64::try_from(start_ns).unwrap_or(u64::MAX));
    run.write(&store)?;
    let watch = Watch::start(gaia_progress::Options::for_stderr(
        logs_dir(&store),
        std::process::id(),
        start_ns,
        project.built.doc.graph.nodes.len().saturating_sub(1),
    ));
    let result = {
        let runner = loaded.host().runner();
        runner.set_failure_policy(FailurePolicy::DEFAULT.with_fail_fast(true));
        match Lanes::new(runner, Arc::new(Pool::new(jobs)), &[]) {
            Ok(mut lanes) => runner.tick_parallel(&mut lanes).map(drop),
            Err(e) => return Err(Stop::usage(format!("lanes: {e:?}"))),
        }
    };
    let end_ns = now_ns();
    watch.finish();

    let pid = std::process::id();
    // A step without a job count of its own was given the build's budget.
    let budget = u32::try_from(jobs).unwrap_or(u32::MAX);
    let records: Vec<gaia_step::StepRecord> = step_records(&logs_dir(&store))
        .into_iter()
        .filter(|r| r.pid == pid && r.start_ns >= start_ns)
        .collect();
    let yields = timing::yield_summary(&records);
    let steps: Vec<record::Step> = records
        .into_iter()
        .map(|r| record::Step {
            step: r.step,
            key: r.key.0.to_string(),
            made: r.made,
            ms: u64::try_from(r.end_ns.saturating_sub(r.start_ns) / 1_000_000).unwrap_or(u64::MAX),
            outputs: r.outputs,
            start_ns: u64::try_from(r.start_ns).unwrap_or(u64::MAX),
            end_ns: u64::try_from(r.end_ns).unwrap_or(u64::MAX),
            cpu_ms: r.report.cpu_ns / 1_000_000,
            own_cpu_ms: r.report.own_cpu_ns / 1_000_000,
            load: r.report.load,
            jobs: r.report.jobs.or(Some(budget)),
            failed: r.report.failed,
            details: r.report.details,
        })
        .collect();
    let deps = dependencies(&project);
    let critical_path = timing::critical_path(&steps, &deps);
    let pool = timing::pool_summary(&steps);
    let items = timing::top_items(&steps, timing::TOP);

    let (outcome, stop) = match result {
        Ok(()) => ("done", None),
        // A step whose tool was stopped by the cancel ends as a fault
        // (`<step>: cancelled`), not as Daedalus's skip, so a fault after
        // Ctrl-C is the cancel too.
        Err(RunError::Cancelled { .. }) | Err(RunError::Fault { .. })
            if INTERRUPTS.load(Ordering::SeqCst) > 0 =>
        {
            (
                "cancelled",
                Some(Stop {
                    code: CANCELLED,
                    message: "cancelled; what finished is kept in the store".into(),
                }),
            )
        }
        Err(e) => {
            let mut message = String::new();
            // Each failure its own log, when several nodes of one step fail.
            let mut taken = std::collections::BTreeSet::new();
            for fault in loaded.fault_reports() {
                let at = fault
                    .at
                    .map(str::to_string)
                    .unwrap_or_else(|| fault.covers.join(" + "));
                // Daedalus keeps a fixed length of a reason and says when it
                // cut one; the whole message is then in the step's run log.
                let cut = fault.reason_cut;
                let reason = fault.reason();
                let log = gaia_progress::reason_step(reason).and_then(|step| {
                    Reader::new(logs_dir(&store), std::process::id(), start_ns)
                        .failed_log(step, reason, &taken)
                });
                taken.extend(log.clone());
                // A cut reason is shown whole when this process still has it
                // (a step that refused before it ran anything has no run
                // log); otherwise it says where the rest is, or that it is
                // nowhere.
                let shown = match (cut, gaia_step::whole_reason(reason), &log) {
                    (false, _, _) => reason.to_string(),
                    (true, Some(whole), _) => whole,
                    (true, None, Some(_)) => {
                        format!("{reason}\n(reason cut; the whole message is in the run log)")
                    }
                    (true, None, None) => {
                        format!("{reason}\n(reason cut, and the step kept no run log of it)")
                    }
                };
                message.push_str(&gaia_progress::failure(&at, &shown, log.as_deref()));
            }
            if message.is_empty() {
                message = format!("the build failed: {e:?}");
            }
            let message = message.trim_end().to_string();
            (
                "failed",
                Some(Stop {
                    code: FAILED,
                    message,
                }),
            )
        }
    };

    let mut outputs = BTreeMap::new();
    if stop.is_none() {
        let mut ports = project.built.doc.graph.nodes[0].port_types.clone();
        ports.retain(|port, _| !project.built.inputs.contains_key(port));
        for (port, ty) in ports {
            let object = take(&mut loaded, &port, &ty)?;
            let from = upstream(&project, &port);
            outputs.insert(port, Output { ty, object, from });
        }
    }
    run.end_ns = u64::try_from(end_ns).unwrap_or(u64::MAX);
    run.outcome = Some(outcome.into());
    run.outputs = outputs;
    run.steps = steps;
    run.critical_path = critical_path;
    if options.dev_reuse {
        run.build = Some(crate::dev::BUILD.into());
        run.dev_erofs = project.dev_erofs.clone();
    }
    let path = run.write(&store)?;
    if options.dev_reuse {
        for (port, copy) in crate::dev::mark_outputs(&store, &run, &path)? {
            println!("{port}  {}", copy.display());
        }
    } else {
        for (port, output) in &run.outputs {
            println!("{port}  {}", store.path(&output.object).display());
        }
    }
    // On stderr, so the port lines on stdout stay as they are; the last
    // line on stdout is the outcome and the record.
    eprint!("{}", timing::report(&items, &run.critical_path));
    // What the root lost and kept of the toolchain's runtime libraries, made
    // now or found in the store: each step's report, as it wrote it. An
    // image with a slot prunes two trees (the root, and the same without
    // /boot's kernel files), and their reports are the same lines: said
    // once, with how many trees it is for.
    let reports: Vec<String> = run
        .steps
        .iter()
        .filter(|step| step.step == "gaia.strip.runtime-libs")
        .filter_map(|step| step.outputs.get("report"))
        .filter_map(|report| std::fs::read_to_string(store.path(report)).ok())
        .collect();
    eprint!("{}", runtime_reports(&reports));
    if let Some(pool) = &pool {
        eprintln!("{pool}");
    }
    if let Some(yields) = &yields {
        eprintln!("{yields}");
    }
    println!(
        "{outcome} in {:.1} s; recorded in {}",
        (end_ns - start_ns) as f64 / 1e9,
        path.display()
    );
    if options.dev_reuse {
        eprintln!("DEV BUILD — not for release (gaia build --dev-reuse; the record says so)");
    }
    stop.map_or(Ok(run), Err)
}

/// The graph's node ids, and each node's direct upstream nodes, for the
/// critical path. Node 0 is the host, which no step is.
fn dependencies(project: &Project) -> timing::Deps {
    let graph = &project.built.doc.graph;
    let ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    let mut upstream: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in &graph.edges {
        let (Some(from), Some(to)) = (
            ids.get(edge.from.node as usize),
            ids.get(edge.to.node as usize),
        ) else {
            continue;
        };
        if edge.from.node != 0 {
            upstream.entry(to.clone()).or_default().insert(from.clone());
        }
    }
    timing::Deps {
        nodes: ids.into_iter().collect(),
        upstream,
    }
}

/// The object at host port `port`.
fn take(loaded: &mut LoadedHost, port: &str, ty: &str) -> Result<ObjectRef, Stop> {
    let missing = || Stop::usage(format!("the run gave no `{port}`"));
    let reference = match ty {
        "gaia:artifact" => {
            loaded
                .host()
                .take::<Artifact>(port)
                .map_err(Stop::usage)?
                .ok_or_else(missing)?
                .object
        }
        "gaia:image" => {
            loaded
                .host()
                .take::<Image>(port)
                .map_err(Stop::usage)?
                .ok_or_else(missing)?
                .object
        }
        "gaia:tree" => {
            loaded
                .host()
                .take::<Tree>(port)
                .map_err(Stop::usage)?
                .ok_or_else(missing)?
                .object
        }
        other => {
            return Err(Stop::usage(format!(
                "output `{port}` is a {other}; gaia keeps artifacts, images and trees"
            )));
        }
    };
    reference
        .object_ref()
        .ok_or_else(|| Stop::usage(format!("`{port}` names no store object")))
}

/// The nodes `port` came from, in graph order.
fn upstream(project: &Project, port: &str) -> Vec<Origin> {
    let graph = &project.built.doc.graph;
    let mut seen = BTreeSet::new();
    let mut todo: Vec<usize> = graph
        .edges
        .iter()
        .filter(|e| e.to.node == 0 && e.to.port == port)
        .map(|e| e.from.node as usize)
        .collect();
    while let Some(node) = todo.pop() {
        if node == 0 || !seen.insert(node) {
            continue;
        }
        todo.extend(
            graph
                .edges
                .iter()
                .filter(|e| e.to.node as usize == node)
                .map(|e| e.from.node as usize),
        );
    }
    seen.into_iter()
        .map(|node| Origin {
            label: project.label(node).to_string(),
            id: graph.nodes[node].id.clone(),
            file: project.built.origins[node].file.clone(),
        })
        .collect()
}

/// The runtime-library reports of a build as its last lines say them: each
/// different report once, in the order first seen, its lines prefixed, and
/// when several of the build's trees gave the same one, how many.
fn runtime_reports(reports: &[String]) -> String {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for report in reports {
        match seen.iter_mut().find(|(text, _)| *text == report.as_str()) {
            Some((_, count)) => *count += 1,
            None => seen.push((report, 1)),
        }
    }
    let mut out = String::new();
    for (text, count) in seen {
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            out.push_str(&format!("runtime libraries: {line}\n"));
        }
        if count > 1 {
            out.push_str(&format!(
                "runtime libraries: the same for {count} trees of this build\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod runtime_report_tests {
    use super::runtime_reports;

    /// Two trees with the same report say it once and say so; a different
    /// one is said as well; comment lines are left out; none says nothing.
    #[test]
    fn the_same_report_is_said_once() {
        let a = "# header\nremoved libgomp: x\nsaved   5 B in 1 file\n".to_string();
        let b = "kept    libstdc++: needed by usr/bin/app\nsaved   0 B in 0 files\n".to_string();
        assert_eq!(
            runtime_reports(&[a.clone(), a.clone(), b]),
            "runtime libraries: removed libgomp: x\n\
             runtime libraries: saved   5 B in 1 file\n\
             runtime libraries: the same for 2 trees of this build\n\
             runtime libraries: kept    libstdc++: needed by usr/bin/app\n\
             runtime libraries: saved   0 B in 0 files\n"
        );
        assert_eq!(
            runtime_reports(&[a]),
            "runtime libraries: removed libgomp: x\nruntime libraries: saved   5 B in 1 file\n"
        );
        assert_eq!(runtime_reports(&[]), "");
    }
}
