//! Where a build's time went: the items that took the most wall time, and
//! the critical path. Both are read from a run's steps, which a build
//! prints at its end and keeps in its record.

use crate::record::{PathStep, Step};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

/// How many items the table shows.
pub const TOP: usize = 10;
/// The step detail that names a Buildroot make's packages. The packages
/// are items of their own in the table.
const PACKAGES: &str = "packages";
/// The step detail of a Buildroot make's job pool: `left` lists the
/// packages whose make ran a pool of its own beside the shared one.
const JOB_POOL: &str = "job-pool";

/// One line for the build: how many packages' makes left the shared job
/// pool, and which, each with the job count it forced (for example
/// `1 package left the job pool (host-ninja -j26)`). `None` when no step
/// recorded its job pool.
pub fn pool_summary(steps: &[Step]) -> Option<String> {
    let mut seen = false;
    let mut left = BTreeSet::new();
    for step in steps {
        let Some(pool) = step.details.get(JOB_POOL) else {
            continue;
        };
        seen = true;
        for entry in pool["left"].as_array().into_iter().flatten() {
            let package = entry["package"].as_str().unwrap_or("?");
            let jobs = entry["jobs"].as_u64().unwrap_or(0);
            left.insert(format!("{package} -j{jobs}"));
        }
    }
    if !seen {
        return None;
    }
    let names: Vec<String> = left.into_iter().collect();
    let count = names.len();
    let noun = if count == 1 { "package" } else { "packages" };
    Some(if names.is_empty() {
        format!("{count} {noun} left the job pool")
    } else {
        format!("{count} {noun} left the job pool ({})", names.join(", "))
    })
}

/// The detail a step records what yielding did under (see the Buildroot
/// step's job pool).
const YIELD: &str = "job-pool-yield";

/// One line for the build: each step that yielded the shared job pool,
/// why (the decision of `gaia_step::slack`), to which steps, the
/// token-seconds it held beside them at its niceness step, and its join
/// this run; or that nothing yielded, and why. `None` when no step recorded
/// what yielding did. `records` are this run's step records.
pub fn yield_summary(records: &[gaia_step::StepRecord]) -> Option<String> {
    let label = |record: &gaia_step::StepRecord| {
        let detail = &record.report.details[YIELD];
        detail["label"].as_str().unwrap_or(&record.step).to_string()
    };
    let with: Vec<usize> = (0..records.len())
        .filter(|&index| records[index].report.details.contains_key(YIELD))
        .collect();
    if with.is_empty() {
        return None;
    }
    let yielded: Vec<usize> = with
        .iter()
        .copied()
        .filter(|&index| records[index].report.yielded)
        .collect();
    if yielded.is_empty() {
        let mut why: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for &index in &with {
            let reason = records[index].report.details[YIELD]["why"]
                .as_str()
                .unwrap_or("?")
                .to_string();
            why.entry(reason).or_default().push(label(&records[index]));
        }
        let why: Vec<String> = why
            .into_iter()
            .map(|(reason, labels)| format!("{}: {reason}", labels.join(", ")))
            .collect();
        return Some(format!("yield: nothing yielded ({})", why.join("; ")));
    }
    let run: Vec<usize> = (0..records.len()).collect();
    let parts: Vec<String> = yielded
        .iter()
        .map(|&index| {
            let detail = &records[index].report.details[YIELD];
            let to: Vec<&str> = detail["to"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str())
                .collect();
            let mut part = format!(
                "{} yields: {}; to {}: its own pool of {} jobs at nice +{} ({}), {:.0} token-s held beside them",
                label(&records[index]),
                detail["why"].as_str().unwrap_or("?"),
                if to.is_empty() {
                    "nothing".to_string()
                } else {
                    to.join(", ")
                },
                detail["pool_jobs"].as_u64().unwrap_or(0),
                detail["nice_step"].as_i64().unwrap_or(0),
                detail["split"].as_str().unwrap_or("?"),
                detail["beside_token_s"].as_f64().unwrap_or(0.0),
            );
            if let Some(join) = gaia_step::slack::join(records, &run, index) {
                part.push_str(&format!("; join this run {:.0} s", join as f64 / 1e9));
            }
            part
        })
        .collect();
    Some(format!("yield: {}", parts.join("; ")))
}

/// The graph the steps came from: every node's id, and the ids of the nodes
/// it takes from directly.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Deps {
    pub nodes: BTreeSet<String>,
    pub upstream: BTreeMap<String, BTreeSet<String>>,
}

/// What a row of the table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Step,
    Package,
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub wall_ms: u64,
    /// CPU time of the step's tools and of its work inside Gaia's own
    /// process. A package's CPU time is not measured, so a package has none.
    pub cpu_ms: Option<u64>,
    pub jobs: Option<u32>,
    /// Whether it was made (not found in the store). A package always was.
    pub made: bool,
}

/// The `n` items that took the most wall time, most first: the steps and
/// the packages their details name, ties broken by kind (steps first) and
/// name.
pub fn top_items(steps: &[Step], n: usize) -> Vec<Item> {
    let mut items = Vec::new();
    for step in steps {
        items.push(Item {
            kind: Kind::Step,
            name: step.step.clone(),
            wall_ms: step.ms,
            cpu_ms: Some(step.cpu_ms + step.own_cpu_ms),
            jobs: step.jobs,
            made: step.made,
        });
        let Some(packages) = step.details.get(PACKAGES).and_then(|v| v.as_array()) else {
            continue;
        };
        for package in packages {
            let (Some(name), Some(total)) = (
                package.get("name").and_then(|v| v.as_str()),
                package.get("total_s").and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            items.push(Item {
                kind: Kind::Package,
                name: name.to_string(),
                wall_ms: (total * 1_000.0).round().max(0.0) as u64,
                cpu_ms: None,
                jobs: step.jobs,
                made: true,
            });
        }
    }
    items.sort_by_key(|item| (Reverse(item.wall_ms), item.kind, item.name.clone()));
    items.truncate(n);
    items
}

/// The node a step belongs to: the step's own name when it is a node, else
/// the longest node whose id the name extends with `-` (a node's helper
/// step, such as `<node>-plan`).
fn node_of<'a>(step: &'a str, deps: &'a Deps) -> Option<&'a str> {
    if let Some(node) = deps.nodes.get(step) {
        return Some(node.as_str());
    }
    deps.nodes
        .iter()
        .filter(|node| {
            step.strip_prefix(node.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|node| node.len())
        .map(String::as_str)
}

/// Every node `node` takes from, directly or through others.
fn upstream_of(node: &str, deps: &Deps) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![node.to_string()];
    while let Some(current) = todo.pop() {
        for dep in deps.upstream.get(&current).into_iter().flatten() {
            if seen.insert(dep.clone()) {
                todo.push(dep.clone());
            }
        }
    }
    seen
}

/// The chain of steps that set the run's end, first to last. It starts at
/// the step that ended last, and each step before it is the one that ended
/// last among those it depends on, which had to end before it began. A
/// step with no node in the graph depends on nothing and is not chained
/// to. Steps without times (a record from before they were kept) give no
/// path.
pub fn critical_path(steps: &[Step], deps: &Deps) -> Vec<PathStep> {
    let timed = |step: &Step| step.end_ns > 0;
    let Some(last) = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| timed(step))
        .max_by_key(|(index, step)| (step.end_ns, *index))
        .map(|(index, _)| index)
    else {
        return Vec::new();
    };
    let mut chain = vec![last];
    while let Some(&current) = chain.last() {
        let Some(node) = node_of(&steps[current].step, deps) else {
            break;
        };
        let upstream = upstream_of(node, deps);
        let started = steps[current].start_ns;
        let before = (0..steps.len()).filter(|&other| {
            other != current
                && timed(&steps[other])
                && steps[other].end_ns <= started
                && node_of(&steps[other].step, deps)
                    .is_some_and(|other_node| other_node == node || upstream.contains(other_node))
        });
        match before.max_by_key(|&other| (steps[other].end_ns, other)) {
            Some(other) => chain.push(other),
            None => break,
        }
    }
    chain
        .into_iter()
        .rev()
        .map(|index| PathStep {
            step: steps[index].step.clone(),
            ms: steps[index].ms,
        })
        .collect()
}

/// Seconds, to a tenth, for the table.
fn secs(ms: u64) -> String {
    format!("{:.1}", ms as f64 / 1_000.0)
}

/// The table and the critical path, as the lines `gaia build` prints.
pub fn report(items: &[Item], path: &[PathStep]) -> String {
    let mut text = String::new();
    if !items.is_empty() {
        let width = items
            .iter()
            .map(|item| item.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        text.push_str(&format!(
            "top {} by wall time:\n  {:<5} {:<width$}  {:>8}  {:>7}  {:>4}  {}\n",
            items.len(),
            "kind",
            "name",
            "wall s",
            "cpu s",
            "-j",
            "hit/miss",
        ));
        for item in items {
            let kind = match item.kind {
                Kind::Step => "step",
                Kind::Package => "pkg",
            };
            text.push_str(&format!(
                "  {:<5} {:<width$}  {:>8}  {:>7}  {:>4}  {}\n",
                kind,
                item.name,
                secs(item.wall_ms),
                item.cpu_ms.map_or("-".to_string(), secs),
                item.jobs.map_or("-".to_string(), |jobs| jobs.to_string()),
                if item.made { "miss" } else { "hit" },
            ));
        }
    }
    if !path.is_empty() {
        let chain: Vec<String> = path
            .iter()
            .map(|step| format!("{} {} s", step.step, secs(step.ms)))
            .collect();
        text.push_str(&format!("critical path: {}\n", chain.join(" -> ")));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step record of one run for the yield line: `name` from `start` to
    /// `end` seconds, taking the outputs of `inputs`, with what yielding did.
    fn yield_record(
        name: &str,
        start: u128,
        end: u128,
        inputs: &[&str],
        detail: Option<serde_json::Value>,
    ) -> gaia_step::StepRecord {
        let digest = |of: &str| gaia_store::Digest::of_bytes(format!("out {of}").as_bytes());
        let mut report = gaia_step::Report {
            inputs: inputs.iter().map(|from| digest(from).to_string()).collect(),
            ..gaia_step::Report::default()
        };
        if let Some(detail) = detail {
            report.yielded = detail["yielded"] == true;
            report.details.insert(YIELD.into(), detail);
        }
        gaia_step::StepRecord {
            start_ns: start * 1_000_000_000,
            end_ns: end * 1_000_000_000,
            pid: 1,
            made: true,
            step: "gaia.os.buildroot".into(),
            key: gaia_store::StepKey(gaia_store::Digest::of_bytes(name.as_bytes())),
            outputs: [(
                "out".to_string(),
                format!("tree:{}", digest(name)).parse().expect("a ref"),
            )]
            .into(),
            work_bytes: None,
            report,
        }
    }

    fn stayed(label: &str, why: &str) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "yielded": false, "why": why, "label": label }))
    }

    fn went(label: &str) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "yielded": true, "why": "last join 432 s yielded against 649 s unyielded", "label": label,
            "to": ["root filesystem"], "nice": 7, "nice_step": 7, "split": "about 5 : 1 inside Gaia's unit", "pool_jobs": 24, "beside_token_s": 3456.7,
        }))
    }

    #[test]
    fn the_yield_line_says_nothing_yielded_and_why() {
        let records = vec![
            yield_record("host", 0, 100, &[], stayed("host tools", "no history")),
            yield_record(
                "kernel",
                100,
                863,
                &["host"],
                stayed("kernel", "no history"),
            ),
            yield_record(
                "rootfs",
                100,
                1108,
                &["host"],
                stayed("root filesystem", "no history"),
            ),
            yield_record("image", 1108, 1130, &["kernel", "rootfs"], None),
        ];
        assert_eq!(
            yield_summary(&records).as_deref(),
            Some("yield: nothing yielded (host tools, kernel, root filesystem: no history)")
        );
        assert_eq!(yield_summary(&records[3..]), None);
    }

    #[test]
    fn the_yield_line_names_what_yielded_why_to_what_and_its_join() {
        let records = vec![
            yield_record(
                "host",
                0,
                100,
                &[],
                stayed("host tools", "slack 0 s over 100 s"),
            ),
            yield_record("kernel", 100, 900, &["host"], went("kernel")),
            yield_record(
                "rootfs",
                100,
                1108,
                &["host"],
                stayed("root filesystem", "slack 0 s"),
            ),
            yield_record("image", 1108, 1130, &["kernel", "rootfs"], None),
        ];
        assert_eq!(
            yield_summary(&records).as_deref(),
            Some(
                "yield: kernel yields: last join 432 s yielded against 649 s unyielded; to root \
                 filesystem: its own pool of 24 jobs at nice +7 (about 5 : 1 inside Gaia's unit), 3457 token-s held beside them; \
                 join this run 1008 s"
            )
        );
    }
    use std::collections::BTreeMap;

    fn step(name: &str, start_s: u64, end_s: u64) -> Step {
        Step {
            step: name.into(),
            key: "sha256:00".into(),
            made: true,
            ms: (end_s - start_s) * 1_000,
            outputs: BTreeMap::new(),
            start_ns: start_s * 1_000_000_000,
            end_ns: end_s * 1_000_000_000,
            cpu_ms: 0,
            own_cpu_ms: 0,
            load: None,
            jobs: None,
            failed: false,
            details: BTreeMap::new(),
        }
    }

    fn packages(list: &[(&str, f64)]) -> serde_json::Value {
        serde_json::Value::Array(
            list.iter()
                .map(|(name, total)| serde_json::json!({"name": name, "total_s": total, "phases": {}}))
                .collect(),
        )
    }

    fn deps(edges: &[(&str, &str)], nodes: &[&str]) -> Deps {
        let mut upstream: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (from, to) in edges {
            upstream
                .entry((*to).into())
                .or_default()
                .insert((*from).into());
        }
        Deps {
            nodes: nodes.iter().map(|n| (*n).to_string()).collect(),
            upstream,
        }
    }

    #[test]
    fn top_items_sort_by_wall_time_then_kind_then_name() {
        let mut buildroot = step("gaia.os.buildroot", 0, 100);
        buildroot.cpu_ms = 90_000;
        buildroot.own_cpu_ms = 500;
        buildroot.jobs = Some(13);
        buildroot.details.insert(
            PACKAGES.into(),
            packages(&[("glibc", 40.0), ("zlib", 100.0), ("busybox", 2.5)]),
        );
        let steps = vec![buildroot, step("gaia.fs.erofs", 100, 101)];
        let items = top_items(&steps, 10);
        let order: Vec<(&str, Kind)> = items.iter().map(|i| (i.name.as_str(), i.kind)).collect();
        assert_eq!(
            order,
            [
                ("gaia.os.buildroot", Kind::Step),
                ("zlib", Kind::Package),
                ("glibc", Kind::Package),
                ("busybox", Kind::Package),
                ("gaia.fs.erofs", Kind::Step),
            ]
        );
        // A package has no CPU time of its own; the step's job count is
        // the packages' too.
        assert_eq!(items[1].cpu_ms, None);
        assert_eq!(items[1].jobs, Some(13));
        assert_eq!(items[0].cpu_ms, Some(90_500));
        assert_eq!(items[0].jobs, Some(13));
    }

    #[test]
    fn top_items_keep_ten_at_most_and_ties_by_name() {
        let steps: Vec<Step> = (0..12)
            .map(|i| step(&format!("s{:02}", 11 - i), 0, 5))
            .collect();
        let items = top_items(&steps, TOP);
        assert_eq!(items.len(), TOP);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names[0], "s00");
        assert_eq!(names[9], "s09");
    }

    #[test]
    fn critical_path_follows_dependencies_not_just_time() {
        // a -> b -> c is the chain; d is independent and ends after c, so
        // it is the last step but no part of the chain that set c's start.
        let graph = deps(&[("a", "b"), ("b", "c")], &["a", "b", "c", "d"]);
        let steps = vec![
            step("a", 0, 10),
            step("d", 1, 50),
            step("b", 10, 30),
            step("c", 30, 40),
        ];
        let path = critical_path(&steps, &graph);
        let names: Vec<&str> = path.iter().map(|p| p.step.as_str()).collect();
        // d ended last, so the chain starts there, and d has no dependency.
        assert_eq!(names, ["d"]);

        let steps = vec![
            step("a", 0, 10),
            step("x", 2, 20),
            step("b", 10, 30),
            step("c", 30, 40),
        ];
        let graph = deps(&[("a", "b"), ("b", "c")], &["a", "b", "c", "x"]);
        let names: Vec<String> = critical_path(&steps, &graph)
            .into_iter()
            .map(|p| p.step)
            .collect();
        assert_eq!(names, ["a", "b", "c"], "x is not upstream of c");
    }

    #[test]
    fn critical_path_takes_the_latest_dependency_and_reads_helper_steps() {
        // Node `host` has a helper step `host-plan` that runs first; its
        // seed step is `host`. `top` takes from `host` and from `slow`,
        // which ended later than `host` did, so `slow` is the one on the path.
        let graph = deps(
            &[("host", "top"), ("slow", "top")],
            &["host", "slow", "top"],
        );
        let steps = vec![
            step("host-plan", 0, 1),
            step("host", 1, 5),
            step("slow", 0, 8),
            step("top", 8, 12),
        ];
        let names: Vec<String> = critical_path(&steps, &graph)
            .into_iter()
            .map(|p| p.step)
            .collect();
        assert_eq!(names, ["slow", "top"]);
    }

    #[test]
    fn steps_without_times_give_no_path() {
        let graph = deps(&[], &["a"]);
        let mut old = step("a", 0, 1);
        old.end_ns = 0;
        assert!(critical_path(&[old], &graph).is_empty());
    }

    /// The packages that left the pool, over every Buildroot step; none is
    /// still a line when a step recorded its pool, and no line without one.
    #[test]
    fn the_pool_summary_names_the_packages_that_left() {
        let mut host = step("gaia.os.buildroot.host", 0, 10);
        host.details.insert(
            JOB_POOL.into(),
            serde_json::json!({
                "left": [{ "package": "host-ninja", "jobs": 26, "times": 1 }],
                "serial": [{ "package": "host-libzlib", "jobs": 1, "times": 2 }],
            }),
        );
        let mut os = step("gaia.os.buildroot", 10, 20);
        os.details.insert(
            JOB_POOL.into(),
            serde_json::json!({
                "left": [{ "package": "host-cmake-thing", "jobs": 26, "times": 2 }],
                "serial": [],
            }),
        );
        assert_eq!(
            pool_summary(&[host.clone(), os]).as_deref(),
            Some("2 packages left the job pool (host-cmake-thing -j26, host-ninja -j26)")
        );
        assert_eq!(
            pool_summary(std::slice::from_ref(&host)).as_deref(),
            Some("1 package left the job pool (host-ninja -j26)")
        );
        host.details.insert(
            JOB_POOL.into(),
            serde_json::json!({ "left": [], "serial": [] }),
        );
        assert_eq!(
            pool_summary(&[host]).as_deref(),
            Some("0 packages left the job pool")
        );
        assert_eq!(pool_summary(&[step("gaia.fs.erofs", 0, 1)]), None);
    }

    #[test]
    fn report_has_the_table_and_the_path() {
        let items = vec![Item {
            kind: Kind::Package,
            name: "glibc".into(),
            wall_ms: 1_375_000,
            cpu_ms: None,
            jobs: Some(13),
            made: true,
        }];
        let path = vec![PathStep {
            step: "gaia.os.buildroot".into(),
            ms: 3_331_000,
        }];
        let text = report(&items, &path);
        assert!(text.starts_with("top 1 by wall time:\n"), "{text}");
        assert!(text.contains("pkg   glibc"), "{text}");
        assert!(text.contains("1375.0"), "{text}");
        assert!(text.contains("-    13  miss"), "{text}");
        assert!(
            text.ends_with("critical path: gaia.os.buildroot 3331.0 s\n"),
            "{text}"
        );
        assert_eq!(report(&[], &[]), "");
    }
}
