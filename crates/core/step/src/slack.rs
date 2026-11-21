//! Which steps can wait, and whether waiting made a run's join later.
//!
//! A run's graph, as it ran, is in its step records: each record has the
//! digests of its key's inputs and the objects it output, so a step that
//! took another's output is downstream of it. Over that graph, with each
//! step's recorded time, a step's slack is how much later it could have
//! ended without delaying the run: the latest it could end (the earliest of
//! its consumers' latest starts; a consumer's latest start is its own
//! latest end less its time) less when it did end. A step on the critical
//! path has none. A step no record takes from is taken to be needed when it
//! ended: not every edge is in the records (a step whose key is made from
//! another digest than its input's, as a file system image of a merged tree
//! is), and a step taken to end with the run would hand the steps before it
//! a slack they did not have. So slack is never more than the step had, and
//! can be less.
//!
//! A step's join is when the first step downstream of both it and a step
//! that ran beside it started, counted from its own start: for a kernel
//! built beside a root filesystem, the time from both starting to the first
//! step that needs both. Yielding is for an earlier join, so the join is
//! what decides.
//!
//! A step is known across runs by its shape (see [`crate::Report::shape`]),
//! not its key, which changes with its content, and only runs at the same
//! job count are compared. [`decide`] says whether a step yields the shared
//! job pool to the others in this run:
//!
//! - With no run of it that yielded, it tries yielding when the last run
//!   that made it recorded a slack of more than [`FIRST_TRY`] of its own
//!   time.
//! - With a run that yielded and none that did not, there is nothing to
//!   compare against, and it keeps yielding until there is.
//! - With both, it compares the last yielded join with the last unyielded
//!   one: it yields when the yielded join was earlier by more than
//!   [`HYSTERESIS`] of the unyielded one, and stops when it was later by
//!   more. A near-tie keeps what the latest of the two runs did, as does a
//!   comparison of runs whose loads at their starts are more than
//!   [`LOAD_APART`] times apart.
//!
//! Never with no history, when two steps of one run share the shape, when
//! no recorded step took from it, or when [`OFF`] is set.
//!
//! The decision changes how jobs are shared while the steps run, never what
//! a step makes or its key: the shape, the records and [`OFF`] are outside
//! every key.

use crate::StepRecord;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;

/// The slack a step must have had, as a share of its own time, for its
/// first try at yielding.
pub const FIRST_TRY: f64 = 0.10;

/// How much later than the unyielded join, as a share of it, a yielded join
/// must be to stop yielding (and how much earlier to start again); a
/// smaller difference keeps the current state.
pub const HYSTERESIS: f64 = 0.10;

/// Two runs whose loads at their starts (the 1-minute load average) are
/// more than this many times apart are not compared.
pub const LOAD_APART: f32 = 2.0;

/// The environment variable that turns yielding off: set to anything but
/// empty or `0`, no step yields.
pub const OFF: &str = "GAIA_NO_YIELD";

/// Whether `value`, the value of [`OFF`], turns yielding off.
pub fn off(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether a step yields, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub yields: bool,
    pub why: Why,
}

/// Why a step yields or does not. Times are in nanoseconds.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    /// [`OFF`] is set.
    Off,
    /// No step key of this process names its shape (a step that made its
    /// key without [`crate::KeyBuilder`]).
    NoShape,
    /// No run made a step of this shape at this job count.
    NoHistory,
    /// Every run that made it made another step of the same shape.
    Shared,
    /// No step of the last run that made it took from it.
    NoConsumer,
    /// It never yielded: its slack and its time in the last run that made
    /// it. It yields when the slack was more than [`FIRST_TRY`] of the time.
    FirstTry { slack_ns: i128, time_ns: i128 },
    /// It yielded, with this join, and no run made it without yielding.
    Uncompared { join_ns: i128 },
    /// The last yielded and unyielded joins, and what held the state when
    /// they did not decide it.
    Joins {
        yielded_ns: i128,
        unyielded_ns: i128,
        held: Option<Held>,
    },
}

/// Why a comparison of joins kept the current state.
#[derive(Debug, Clone, PartialEq)]
pub enum Held {
    /// The joins were within [`HYSTERESIS`] of each other.
    NearTie,
    /// The loads at the two runs' starts were more than [`LOAD_APART`]
    /// times apart.
    Load { yielded: f32, unyielded: f32 },
}

impl Decision {
    pub fn yields(&self) -> bool {
        self.yields
    }
}

fn secs(ns: i128) -> String {
    format!("{:.0} s", ns as f64 / 1e9)
}

fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.yields {
            "yielding"
        } else {
            "not yielding"
        };
        match &self.why {
            Why::Off => write!(f, "turned off by {OFF}"),
            Why::NoShape => write!(f, "no shape"),
            Why::NoHistory => write!(f, "no history"),
            Why::Shared => write!(f, "its runs had another step of its shape"),
            Why::NoConsumer => write!(f, "no step took from it last run"),
            Why::FirstTry { slack_ns, time_ns } if self.yields => write!(
                f,
                "first try: slack {} over {} last run, more than {}",
                secs(*slack_ns),
                secs(*time_ns),
                percent(FIRST_TRY)
            ),
            Why::FirstTry { slack_ns, time_ns } => write!(
                f,
                "slack {} over {} last run, not more than {}",
                secs(*slack_ns),
                secs(*time_ns),
                percent(FIRST_TRY)
            ),
            Why::Uncompared { join_ns } => write!(
                f,
                "last join {} yielded, and no unyielded run of its shape and job count to compare \
                 against: it keeps yielding until there is one",
                secs(*join_ns)
            ),
            Why::Joins {
                yielded_ns,
                unyielded_ns,
                held,
            } => {
                write!(
                    f,
                    "last join {} yielded against {} unyielded",
                    secs(*yielded_ns),
                    secs(*unyielded_ns)
                )?;
                match held {
                    None if self.yields => Ok(()),
                    None => write!(f, ", more than {} later", percent(HYSTERESIS)),
                    Some(Held::NearTie) => {
                        write!(f, ", within {}: it keeps {state}", percent(HYSTERESIS))
                    }
                    Some(Held::Load { yielded, unyielded }) => write!(
                        f,
                        ", but the loads at their starts ({yielded:.1} and {unyielded:.1}) are \
                         more than {LOAD_APART}x apart: it keeps {state}"
                    ),
                }
            }
        }
    }
}

fn time(record: &StepRecord) -> i128 {
    record.end_ns as i128 - record.start_ns as i128
}

/// Each step of one run (`run`, indexes into `records`) that a step of the
/// run took from, and the steps that took from it.
fn consumers(records: &[StepRecord], run: &[usize]) -> BTreeMap<usize, Vec<usize>> {
    // Each output digest's maker in the run.
    let mut maker: BTreeMap<String, usize> = BTreeMap::new();
    for &index in run {
        for object in records[index].outputs.values() {
            maker.insert(object.digest.to_string(), index);
        }
    }
    let mut consumers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &index in run {
        for input in &records[index].report.inputs {
            if let Some(&from) = maker.get(input)
                && from != index
                && !consumers.entry(from).or_default().contains(&index)
            {
                consumers.entry(from).or_default().push(index);
            }
        }
    }
    consumers
}

/// The slack of each record of one run (`run`, indexes into `records`), in
/// nanoseconds: `None` for a step no step of the run took from.
pub fn slacks(records: &[StepRecord], run: &[usize]) -> BTreeMap<usize, Option<i128>> {
    let consumers = consumers(records, run);
    // A consumer starts after each step it takes from has ended, so latest
    // first is consumers before the steps they take from.
    let mut order = run.to_vec();
    order.sort_by_key(|&index| std::cmp::Reverse((records[index].start_ns, index)));
    let mut latest_end: BTreeMap<usize, i128> = BTreeMap::new();
    for &index in &order {
        let end = consumers
            .get(&index)
            .into_iter()
            .flatten()
            .filter_map(|consumer| {
                latest_end
                    .get(consumer)
                    .map(|end| end - time(&records[*consumer]))
            })
            .min()
            .unwrap_or(records[index].end_ns as i128);
        latest_end.insert(index, end);
    }
    run.iter()
        .map(|&index| {
            let slack = consumers
                .contains_key(&index)
                .then(|| latest_end[&index] - records[index].end_ns as i128);
            (index, slack)
        })
        .collect()
}

/// The steps downstream of `from`: those that took from it, and from them.
fn downstream(consumers: &BTreeMap<usize, Vec<usize>>, from: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut next = vec![from];
    while let Some(index) = next.pop() {
        for &consumer in consumers.get(&index).into_iter().flatten() {
            if seen.insert(consumer) {
                next.push(consumer);
            }
        }
    }
    seen
}

/// The join of `records[at]` in its run (`run`), in nanoseconds: from its
/// start to the start of the first step downstream of both it and a step
/// that ran beside it (one whose time overlapped its own; a step upstream
/// or downstream of it cannot). `None` when there is no such step.
pub fn join(records: &[StepRecord], run: &[usize], at: usize) -> Option<i128> {
    let consumers = consumers(records, run);
    let mine = downstream(&consumers, at);
    let (start, end) = (records[at].start_ns, records[at].end_ns);
    run.iter()
        .filter(|&&index| {
            index != at && records[index].start_ns < end && start < records[index].end_ns
        })
        .flat_map(|&beside| downstream(&consumers, beside))
        .filter(|index| mine.contains(index))
        .map(|index| records[index].start_ns as i128)
        .min()
        .map(|first| first - start as i128)
}

/// Whether the step of shape `shape`, given `jobs` jobs, yields this run,
/// from `records` (the step records, oldest first).
pub fn decide(records: &[StepRecord], shape: Option<&str>, jobs: Option<u32>) -> Decision {
    let stay = |why| Decision { yields: false, why };
    let Some(shape) = shape else {
        return stay(Why::NoShape);
    };
    let of_shape = |record: &StepRecord| record.report.shape.as_deref() == Some(shape);
    // The records of each run: those of its process.
    let mut runs: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        runs.entry(record.pid).or_default().push(index);
    }
    let run_of = |at: usize| &runs[&records[at].pid];

    // The latest usable record, and the latest yielded and unyielded ones
    // that something took from, with their joins.
    let mut shared = false;
    let mut last = None;
    let mut yielded: Option<(usize, i128)> = None;
    let mut unyielded: Option<(usize, i128)> = None;
    for at in (0..records.len()).rev() {
        let record = &records[at];
        if !of_shape(record) || !record.made || record.report.failed || record.report.jobs != jobs {
            continue;
        }
        let run = run_of(at);
        if run
            .iter()
            .filter(|&&index| of_shape(&records[index]))
            .count()
            > 1
        {
            shared = true;
            continue;
        }
        last.get_or_insert(at);
        let found = if record.report.yielded {
            &mut yielded
        } else {
            &mut unyielded
        };
        if found.is_none()
            && let Some(join) = join(records, run, at)
        {
            *found = Some((at, join));
        }
        if yielded.is_some() && unyielded.is_some() {
            break;
        }
    }
    let Some(last) = last else {
        return stay(if shared { Why::Shared } else { Why::NoHistory });
    };
    match (yielded, unyielded) {
        (None, _) => {
            let Some(slack_ns) = slacks(records, run_of(last))[&last] else {
                return stay(Why::NoConsumer);
            };
            let time_ns = time(&records[last]);
            Decision {
                yields: slack_ns as f64 > FIRST_TRY * time_ns as f64,
                why: Why::FirstTry { slack_ns, time_ns },
            }
        }
        (Some((_, join_ns)), None) => Decision {
            yields: true,
            why: Why::Uncompared { join_ns },
        },
        (Some((y, yielded_ns)), Some((u, unyielded_ns))) => {
            let joins = |yields, held| Decision {
                yields,
                why: Why::Joins {
                    yielded_ns,
                    unyielded_ns,
                    held,
                },
            };
            // What the latest of the two runs did.
            let now = y > u;
            let load = |at: usize| records[at].report.load.map(|load| load[0]);
            if let (Some(a), Some(b)) = (load(y), load(u))
                && (a > LOAD_APART * b || b > LOAD_APART * a)
            {
                return joins(
                    now,
                    Some(Held::Load {
                        yielded: a,
                        unyielded: b,
                    }),
                );
            }
            if yielded_ns as f64 > (1.0 + HYSTERESIS) * unyielded_ns as f64 {
                joins(false, None)
            } else if (1.0 + HYSTERESIS) * (yielded_ns as f64) < unyielded_ns as f64 {
                joins(true, None)
            } else {
                joins(now, Some(Held::NearTie))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Report;
    use gaia_store::{Digest, ObjectRef, StepKey};

    const S: u128 = 1_000_000_000;
    const JOBS: Option<u32> = Some(26);

    fn digest(name: &str) -> Digest {
        Digest::of_bytes(name.as_bytes())
    }

    /// A made step of run `pid` from `start` to `end` seconds, its shape
    /// `name`, taking the outputs named in `inputs` and outputting `name`,
    /// given [`JOBS`] jobs at a load of 20.
    fn step(pid: u32, name: &str, start: u128, end: u128, inputs: &[&str]) -> StepRecord {
        StepRecord {
            start_ns: start * S,
            end_ns: end * S,
            pid,
            made: true,
            step: "test".into(),
            key: StepKey(digest(&format!("{pid}{name}"))),
            outputs: [(
                "out".to_string(),
                format!("tree:{}", digest(&format!("out {name}")))
                    .parse::<ObjectRef>()
                    .expect("ref"),
            )]
            .into(),
            work_bytes: None,
            report: Report {
                shape: Some(format!("shape {name}")),
                inputs: inputs
                    .iter()
                    .map(|from| digest(&format!("out {from}")).to_string())
                    .collect(),
                jobs: JOBS,
                load: Some([20.0, 20.0]),
                ..Report::default()
            },
        }
    }

    /// The pool-sample run's shape: host tools, then the kernel (ends at
    /// 763 s) and the root filesystem (ends at 1008 s) side by side, then the
    /// image from both. The kernel's join is 1008 s.
    fn the_run(pid: u32) -> Vec<StepRecord> {
        vec![
            step(pid, "host", 0, 100, &[]),
            step(pid, "kernel", 100, 863, &["host"]),
            step(pid, "rootfs", 100, 1108, &["host"]),
            step(pid, "image", 1108, 1130, &["kernel", "rootfs"]),
        ]
    }

    /// A run in which the kernel yielded and its join was `join` seconds:
    /// the kernel and the root filesystem both end by then, the later of
    /// them at it.
    fn yielded_run(pid: u32, join: u128) -> Vec<StepRecord> {
        let mut run = vec![
            step(pid, "host", 0, 100, &[]),
            step(pid, "kernel", 100, 100 + join, &["host"]),
            step(pid, "rootfs", 100, 100 + join - 50, &["host"]),
            step(pid, "image", 100 + join, 120 + join, &["kernel", "rootfs"]),
        ];
        run[1].report.yielded = true;
        run
    }

    fn shape(name: &str) -> String {
        format!("shape {name}")
    }

    fn kernel(records: &[StepRecord]) -> Decision {
        decide(records, Some(&shape("kernel")), JOBS)
    }

    #[test]
    fn the_step_beside_the_critical_path_has_its_slack_and_its_join() {
        let records = the_run(1);
        let run: Vec<usize> = (0..records.len()).collect();
        let slacks = slacks(&records, &run);
        assert_eq!(slacks[&0], Some(0), "host is on the path");
        assert_eq!(slacks[&1], Some(245 * S as i128), "kernel");
        assert_eq!(slacks[&2], Some(0), "rootfs is on the path");
        assert_eq!(slacks[&3], None, "nothing takes from the image");
        assert_eq!(join(&records, &run, 1), Some(1008 * S as i128));
        assert_eq!(join(&records, &run, 0), None, "nothing ran beside it");
        assert_eq!(join(&records, &run, 3), None);
    }

    /// The proof run's graph: the kernel's modules are taken first, and the
    /// merge takes them and the root filesystem. The kernel's join is the
    /// merge's start, not the modules', and the root filesystem's is the
    /// same.
    #[test]
    fn the_join_is_where_a_step_and_what_ran_beside_it_meet() {
        let records = vec![
            step(1, "host", 0, 85, &[]),
            step(1, "rootfs", 85, 734, &["host"]),
            step(1, "kernel", 85, 618, &["host"]),
            step(1, "modules", 618, 619, &["kernel"]),
            step(1, "merge", 734, 735, &["modules", "rootfs"]),
        ];
        let run: Vec<usize> = (0..records.len()).collect();
        assert_eq!(join(&records, &run, 2), Some(649 * S as i128), "kernel");
        assert_eq!(join(&records, &run, 1), Some(649 * S as i128), "rootfs");
        assert_eq!(join(&records, &run, 3), Some(116 * S as i128), "modules");
        assert_eq!(join(&records, &run, 4), None, "the merge");
    }

    /// The proof run's case: the step after the merge does not record the
    /// merge's output as its input. The kernel, which the merge waited for,
    /// has no slack, not the time from the merge to the run's end.
    #[test]
    fn a_step_whose_consumer_is_not_recorded_hands_back_no_slack() {
        let records = vec![
            step(1, "host", 0, 100, &[]),
            step(1, "rootfs", 100, 410, &["host"]),
            step(1, "kernel", 100, 532, &["host"]),
            step(1, "merge", 532, 533, &["rootfs", "kernel"]),
            // Keyed from another digest than the merge's output.
            step(1, "erofs", 533, 546, &[]),
            step(1, "disk", 546, 586, &["erofs"]),
        ];
        let run: Vec<usize> = (0..records.len()).collect();
        let slacks = slacks(&records, &run);
        assert_eq!(slacks[&2], Some(0), "the kernel was on the path");
        assert_eq!(
            slacks[&1],
            Some(122 * S as i128),
            "the root filesystem waited"
        );
        assert_eq!(slacks[&3], None, "nothing recorded takes from the merge");
    }

    #[test]
    fn a_first_try_needs_slack_beyond_a_tenth_of_its_time() {
        let records = the_run(1);
        let decision = kernel(&records);
        assert_eq!(
            decision,
            Decision {
                yields: true,
                why: Why::FirstTry {
                    slack_ns: 245 * S as i128,
                    time_ns: 763 * S as i128
                }
            }
        );
        assert_eq!(
            decision.to_string(),
            "first try: slack 245 s over 763 s last run, more than 10%"
        );
        let rootfs = decide(&records, Some(&shape("rootfs")), JOBS);
        assert!(!rootfs.yields(), "{rootfs}");
        assert_eq!(
            decide(&records, Some(&shape("image")), JOBS),
            Decision {
                yields: false,
                why: Why::NoConsumer
            }
        );

        // A 1000 s kernel with 90 s of slack: not more than a tenth.
        let records = vec![
            step(1, "host", 0, 100, &[]),
            step(1, "kernel", 100, 1100, &["host"]),
            step(1, "rootfs", 100, 1190, &["host"]),
            step(1, "image", 1190, 1200, &["kernel", "rootfs"]),
        ];
        let decision = kernel(&records);
        assert!(!decision.yields());
        assert_eq!(
            decision.to_string(),
            "slack 90 s over 1000 s last run, not more than 10%"
        );
    }

    #[test]
    fn with_no_history_at_its_job_count_nothing_yields() {
        let none = Decision {
            yields: false,
            why: Why::NoHistory,
        };
        assert_eq!(kernel(&[]), none);
        assert_eq!(decide(&the_run(1), Some(&shape("kernel")), Some(8)), none);
        assert_eq!(
            decide(&the_run(1), None, JOBS),
            Decision {
                yields: false,
                why: Why::NoShape
            }
        );
    }

    /// With a yielded run and none without, there is nothing to compare
    /// against: it keeps yielding, and says so.
    #[test]
    fn a_yielded_join_with_nothing_to_compare_keeps_yielding() {
        let mut records = yielded_run(1, 900);
        // An unyielded run at another job count is not comparable.
        let mut other = the_run(2);
        for record in &mut other {
            record.report.jobs = Some(8);
        }
        records.extend(other);
        let decision = kernel(&records);
        assert_eq!(
            decision,
            Decision {
                yields: true,
                why: Why::Uncompared {
                    join_ns: 900 * S as i128
                }
            }
        );
        assert_eq!(
            decision.to_string(),
            "last join 900 s yielded, and no unyielded run of its shape and job count to compare \
             against: it keeps yielding until there is one"
        );
    }

    #[test]
    fn an_earlier_yielded_join_keeps_yielding() {
        let mut records = the_run(1);
        records.extend(yielded_run(2, 800));
        let decision = kernel(&records);
        assert!(decision.yields());
        assert_eq!(
            decision.to_string(),
            "last join 800 s yielded against 1008 s unyielded"
        );
    }

    /// A planted history: yielding made the join later by more than the
    /// margin. It stops, and later unyielded runs at the same join do not
    /// start it again.
    #[test]
    fn a_yielded_join_later_by_more_than_the_margin_stops_yielding() {
        let mut records = the_run(1);
        // 1120 s against 1008 s: 11% later.
        records.extend(yielded_run(2, 1120));
        let decision = kernel(&records);
        assert!(!decision.yields());
        assert_eq!(
            decision.to_string(),
            "last join 1120 s yielded against 1008 s unyielded, more than 10% later"
        );
        records.extend(the_run(3));
        assert!(!kernel(&records).yields());
    }

    /// A near-tie keeps what the latest run did, whichever way.
    #[test]
    fn a_near_tie_does_not_flip() {
        // Yielded last, 1100 s against 1008 s: 9% later, so it keeps
        // yielding.
        let mut records = the_run(1);
        records.extend(yielded_run(2, 1100));
        let decision = kernel(&records);
        assert!(decision.yields());
        assert_eq!(
            decision.to_string(),
            "last join 1100 s yielded against 1008 s unyielded, within 10%: it keeps yielding"
        );
        // Not yielded last (yielded 920 s, 9% earlier): it keeps not
        // yielding.
        let mut records = yielded_run(1, 920);
        records.extend(the_run(2));
        let decision = kernel(&records);
        assert!(!decision.yields());
        assert_eq!(
            decision.to_string(),
            "last join 920 s yielded against 1008 s unyielded, within 10%: it keeps not yielding"
        );
    }

    /// Runs whose loads at their starts are more than twice apart are not
    /// compared: the state stays, though the join was much later.
    #[test]
    fn a_load_mismatch_does_not_change_state() {
        let mut records = the_run(1);
        let mut busy = yielded_run(2, 1500);
        for record in &mut busy {
            record.report.load = Some([45.0, 45.0]);
        }
        records.extend(busy);
        let decision = kernel(&records);
        assert!(decision.yields());
        assert_eq!(
            decision.to_string(),
            "last join 1500 s yielded against 1008 s unyielded, but the loads at their starts \
             (45.0 and 20.0) are more than 2x apart: it keeps yielding"
        );
        // At twice the load, they are compared.
        for record in &mut records[4..] {
            record.report.load = Some([40.0, 40.0]);
        }
        assert!(!kernel(&records).yields());
    }

    #[test]
    fn two_steps_of_one_shape_in_a_run_give_no_history() {
        let mut records = the_run(1);
        records[2].report.shape = Some(shape("kernel"));
        assert_eq!(
            kernel(&records),
            Decision {
                yields: false,
                why: Why::Shared
            }
        );
    }

    #[test]
    fn the_variable_turns_yielding_off() {
        assert!(!off(None));
        assert!(!off(Some(OsStr::new(""))));
        assert!(!off(Some(OsStr::new("0"))));
        assert!(off(Some(OsStr::new("1"))));
        assert!(off(Some(OsStr::new("yes"))));
    }
}
