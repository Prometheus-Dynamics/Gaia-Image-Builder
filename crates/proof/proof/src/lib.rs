//! Proof: three real build steps as Daedalus nodes, run by Daedalus.
//!
//! Each chain clones a git repository at a fixed commit, builds one binary
//! in it with `cargo`, and packs the binary into a sorted tar with fixed
//! timestamps. [`graph`] puts any number of chains into one graph, built in
//! code; [`run`] runs it on Daedalus's parallel lanes, so independent chains
//! run at the same time. Every step goes through the step kit: it looks its
//! key up in the store and makes only what is missing, and every tool run
//! leaves a run log, which [`timeline`] reads back.
//!
//! What this crate does not do: retry, cancel other chains or schedule
//! anything. Those are Daedalus's.

mod nodes;
mod tar;

pub use nodes::digest_of;

use daedalus::prelude::*;
use daedalus::runner::{FailurePolicy, LaneError, Lanes, Pool, RunError, TickReport};
use daedalus_cpu::CpuBackend;
use gaia_types::Artifact;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What every step of a run shares: the store, and the pinned tools.
#[derive(Debug, Clone)]
pub struct Setup {
    pub store: PathBuf,
    /// The `git` program.
    pub git: PathBuf,
    /// A Rust sysroot: `bin/cargo` and `bin/rustc` are used from it.
    pub toolchain: PathBuf,
}

/// One chain: clone, build, pack.
#[derive(Debug, Clone)]
pub struct Chain {
    /// Names the chain's host output, `<label>_tar`, and its nodes.
    pub label: String,
    pub url: String,
    pub commit: String,
    /// A cargo profile: `release` or `dev`.
    pub profile: String,
    pub bin: String,
    /// Where the binary sits in the tar.
    pub path: String,
}

/// The graph for `chains`, as a Daedalus graph document.
pub fn graph(setup: &Setup, chains: &[Chain]) -> String {
    let text = |value: &str| format!(r#"{{"type": "str", "value": {}}}"#, json(value));
    let store = text(&setup.store.display().to_string());
    // Each build step is a Daedalus step of its own, so a fault names it,
    // and fail-fast stops at its boundary. Without this the CPU backend
    // merges a chain into one step.
    let own_step = r#""metadata": {"daedalus.node.fusion": {"type": "bool", "value": false}}"#;
    let mut ports = Vec::new();
    let mut nodes = vec![String::new()];
    let mut edges = Vec::new();
    for chain in chains {
        let label = &chain.label;
        let at = nodes.len();
        nodes.push(format!(
            r#"{{"id": "gaia.proof.git", "label": "{label}.git", {own_step}, "consts": {{"store": {store}, "git": {}, "url": {}, "commit": {}}}}}"#,
            text(&setup.git.display().to_string()),
            text(&chain.url),
            text(&chain.commit),
        ));
        nodes.push(format!(
            r#"{{"id": "gaia.proof.cargo", "label": "{label}.cargo", {own_step}, "consts": {{"store": {store}, "toolchain": {}, "profile": {}, "bin": {}}}}}"#,
            text(&setup.toolchain.display().to_string()),
            text(&chain.profile),
            text(&chain.bin),
        ));
        nodes.push(format!(
            r#"{{"id": "gaia.proof.pack", "label": "{label}.pack", {own_step}, "consts": {{"store": {store}, "path": {}}}}}"#,
            text(&chain.path),
        ));
        let edge = |from: usize, out: &str, to: usize, input: &str| {
            format!(
                r#"{{"from": {{"node": {from}, "port": "{out}"}}, "to": {{"node": {to}, "port": "{input}"}}}}"#
            )
        };
        edges.push(edge(at, "source", at + 1, "source"));
        edges.push(edge(at + 1, "binary", at + 2, "binary"));
        edges.push(edge(at + 2, "archive", 0, &format!("{label}_tar")));
        ports.push(format!(r#""{label}_tar": "gaia:artifact""#));
    }
    nodes[0] = format!(
        r#"{{"id": "host", "kind": "host_bridge", "port_types": {{{}}}}}"#,
        ports.join(", ")
    );
    format!(
        r#"{{"format": "daedalus.graph", "schema_version": 1, "graph": {{"nodes": [{}], "edges": [{}]}}}}"#,
        nodes.join(",\n"),
        edges.join(",\n")
    )
}

/// The proof nodes, registered.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register_all(&nodes::NODES)
        .expect("the proof nodes register");
    registry
}

/// Plans and loads a graph document.
pub fn load(document: &str) -> Result<LoadedHost, String> {
    let graph = GraphDoc::parse(document).map_err(|e| e.to_string())?.graph;
    let planned =
        plan(&graph, &registry(), &[&CpuBackend], &Config::default()).map_err(|e| e.to_string())?;
    LoadedHost::load(planned, LoadOptions::default()).map_err(|e| e.to_string())
}

/// Runs one tick of `loaded` on `workers` parallel lanes. With `fail_fast`,
/// a failing step stops the run: steps not started yet are not started.
pub fn run(
    loaded: &mut LoadedHost,
    workers: usize,
    fail_fast: bool,
) -> Result<TickReport, Stopped> {
    let runner = loaded.host().runner();
    runner.set_failure_policy(FailurePolicy::DEFAULT.with_fail_fast(fail_fast));
    let mut lanes =
        Lanes::new(runner, Arc::new(Pool::new(workers)), &[]).map_err(Stopped::Lanes)?;
    runner.tick_parallel(&mut lanes).map_err(Stopped::Run)
}

/// Why [`run`] stopped.
#[derive(Debug)]
pub enum Stopped {
    /// The lanes could not be sized for the graph.
    Lanes(LaneError),
    /// The run stopped: with `fail_fast`, [`RunError::Fault`] names the step.
    Run(RunError),
}

/// Takes a chain's tar from the host.
pub fn take_tar(loaded: &mut LoadedHost, label: &str) -> Option<Artifact> {
    loaded
        .host()
        .take::<Artifact>(&format!("{label}_tar"))
        .ok()
        .flatten()
}

/// One tool run, read back from its run log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub step: String,
    pub tool: String,
    pub start_ns: u128,
    pub end_ns: u128,
    pub status: String,
}

/// Every tool run whose log is under `logs`, by start time.
pub fn timeline(logs: &Path) -> Vec<Span> {
    let mut spans = Vec::new();
    for step in std::fs::read_dir(logs).into_iter().flatten().flatten() {
        for log in std::fs::read_dir(step.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(text) = std::fs::read_to_string(log.path()) else {
                continue;
            };
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(&format!("# {name} ")))
                    .unwrap_or_default()
                    .to_string()
            };
            let end = field("end");
            let (end_ns, status) = end.split_once(' ').unwrap_or((&end, ""));
            spans.push(Span {
                step: field("step"),
                tool: field("tool"),
                start_ns: field("start").parse().unwrap_or_default(),
                end_ns: end_ns.parse().unwrap_or_default(),
                status: status.to_string(),
            });
        }
    }
    spans.sort_by_key(|span| span.start_ns);
    spans
}

/// The timeline as text: milliseconds from the first start.
pub fn show(spans: &[Span]) -> String {
    let origin = spans.first().map_or(0, |span| span.start_ns);
    let ms = |ns: u128| ns.saturating_sub(origin) / 1_000_000;
    let mut out = String::new();
    for span in spans {
        let tool: String = span.tool.chars().take(70).collect();
        let _ = writeln!(
            out,
            "{:>6} ms .. {:>6} ms  {:<17} {:<9} {tool}",
            ms(span.start_ns),
            ms(span.end_ns),
            span.step,
            span.status,
        );
    }
    out
}

fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
