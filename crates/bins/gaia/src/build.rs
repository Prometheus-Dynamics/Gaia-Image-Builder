//! `gaia build`: plan, run on Daedalus's lanes, take the outputs, record
//! the run.

use crate::project::Project;
use crate::record::{self, Origin, Output, Run};
use crate::{CANCELLED, FAILED, Stop, args};
use daedalus::prelude::{LoadOptions, LoadedHost};
use daedalus::runner::{FailurePolicy, Lanes, Pool, RunError};
use gaia_progress::{Reader, Watch};
use gaia_step::{logs_dir, now_ns, step_records};
use gaia_store::{ObjectRef, Store};
use gaia_types::{Artifact, Image, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

pub fn build(options: &args::Project) -> Result<(), Stop> {
    let project = Project::open(options)?;
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
    ctrlc::set_handler(move || {
        if INTERRUPTS.fetch_add(1, Ordering::SeqCst) > 0 {
            std::process::exit(i32::from(CANCELLED));
        }
        eprintln!("gaia: stopping at the next step boundary (Ctrl-C again quits now)");
        cancel.cancel();
    })
    .map_err(Stop::usage)?;

    let jobs = options.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    });
    let start_ns = now_ns();
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
    let steps: Vec<record::Step> = step_records(&logs_dir(&store))
        .into_iter()
        .filter(|r| r.pid == pid && r.start_ns >= start_ns)
        .map(|r| record::Step {
            step: r.step,
            key: r.key.0.to_string(),
            made: r.made,
            ms: u64::try_from(r.end_ns.saturating_sub(r.start_ns) / 1_000_000).unwrap_or(u64::MAX),
            outputs: r.outputs,
        })
        .collect();

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
                    Reader::new(logs_dir(&store), std::process::id(), start_ns).failed_log(step)
                });
                let shown = if cut {
                    format!("{reason}\n(reason cut; the whole message is in the run log)")
                } else {
                    reason.to_string()
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
    let run = Run {
        product: std::path::absolute(&project.product).unwrap_or(project.product.clone()),
        start_ns: u64::try_from(start_ns).unwrap_or(u64::MAX),
        end_ns: u64::try_from(end_ns).unwrap_or(u64::MAX),
        outcome: outcome.into(),
        outputs,
        steps,
    };
    let path = run.write(&store)?;
    for (port, output) in &run.outputs {
        println!("{port}  {}", store.path(&output.object).display());
    }
    println!(
        "{outcome} in {:.1} s; recorded in {}",
        (end_ns - start_ns) as f64 / 1e9,
        path.display()
    );
    stop.map_or(Ok(()), Err)
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
