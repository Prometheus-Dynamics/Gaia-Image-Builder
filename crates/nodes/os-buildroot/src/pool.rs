//! Which of a build's makes left the shared job pool, read from make's own
//! warning as the build prints it.
//!
//! A Buildroot build runs in the process's make jobserver (see `jobs`). A
//! make started with its own `-jN` inside it prints
//! `make[K]: warning: -jN forced in submake: resetting jobserver mode.` and
//! runs a pool of its own. With N of 1 that is the package's choice to
//! build serially (Buildroot's `MAKE1`, syncconfig, `modules_install`),
//! which takes nothing from the shared pool. With N above 1 the package runs
//! N jobs beside the shared pool (pkg-cmake's `cmake --build -jN` with the
//! Makefile generator, for one): bounded by `PARALLEL_JOBS`, which Gaia sets
//! to the job count, but outside the pool. Each such warning is put on the
//! package whose `>>> name version Step` line came last before it (the
//! build's lines interleave, so this is the package being built nearest to
//! it) and recorded in the step's details as [`DETAIL`].

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// The step detail the forced job counts are recorded under.
pub(crate) const DETAIL: &str = "job-pool";

/// What a build's lines said about its makes' job pools.
#[derive(Debug, Default)]
pub(crate) struct Pool {
    /// The package whose `>>>` line came last.
    package: Option<String>,
    /// How many times each package's make forced each job count.
    forced: BTreeMap<(String, u32), u32>,
}

impl Pool {
    /// Takes one line the build printed.
    pub(crate) fn line(&mut self, line: &str) {
        if let Some(at) = line.rfind(">>> ") {
            let mut words = line[at + 4..].split_whitespace();
            if let (Some(name), Some(_version), Some(_step)) =
                (words.next(), words.next(), words.next())
            {
                self.package = Some(name.to_string());
            }
        }
        if let Some(jobs) = forced_jobs(line) {
            let package = self.package.clone().unwrap_or_else(|| "?".into());
            *self.forced.entry((package, jobs)).or_default() += 1;
        }
    }

    /// The detail: `left` the makes that ran a pool of more than one job,
    /// `serial` those forced to one, each by package with the job count and
    /// how often.
    pub(crate) fn json(&self) -> Value {
        let list = |keep: fn(u32) -> bool| -> Vec<Value> {
            self.forced
                .iter()
                .filter(|((_, jobs), _)| keep(*jobs))
                .map(|((package, jobs), times)| {
                    json!({ "package": package, "jobs": jobs, "times": times })
                })
                .collect()
        };
        json!({ "left": list(|jobs| jobs > 1), "serial": list(|jobs| jobs == 1) })
    }
}

/// The `N` of make's `-jN forced in submake` warning, if `line` is one.
fn forced_jobs(line: &str) -> Option<u32> {
    let rest = &line[line.find("warning: -j")? + "warning: -j".len()..];
    let (jobs, tail) = rest.split_once(' ')?;
    tail.starts_with("forced in submake")
        .then(|| jobs.parse().ok())
        .flatten()
}

/// A pool shared with a tool's line sink.
pub(crate) type Shared = Arc<Mutex<Pool>>;

/// Takes `line` into `pool`.
pub(crate) fn feed(pool: &Shared, line: &str) {
    pool.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .line(line);
}

/// Records `pool` in `work`'s details.
pub(crate) fn record(work: &mut gaia_step::Work<'_>, pool: &Shared) {
    let value = pool.lock().unwrap_or_else(PoisonError::into_inner).json();
    work.detail(DETAIL, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as a Buildroot build printed them: a warning goes to the
    /// package whose `>>>` line came last, even mid-line; -j1 is serial,
    /// more than one left the pool.
    #[test]
    fn forced_job_counts_go_to_the_package_built_nearest() {
        let mut pool = Pool::default();
        for line in [
            "/usr/bin/make -j1  O=/out syncconfig",
            "make[1]: warning: -j1 forced in submake: resetting jobserver mode.",
            "checking for suffix of executables... >>> host-libzlib 1.3.2 Building",
            "make[1]: warning: -j1 forced in submake: resetting jobserver mode.",
            ">>> host-ninja 1.13.2 Building",
            "cmake --build /out/build/host-ninja-1.13.2/ -j26",
            "make[2]: warning: -j26 forced in submake: resetting jobserver mode.",
            "ninja: Jobserver mode detected:  -j --jobserver-auth=fifo:/proc/self/fd/8",
            ">>> host-libzlib 1.3.2 Installing to host directory",
            "make[1]: warning: -j1 forced in submake: resetting jobserver mode.",
            "make: warning: -jN forced in submake",
        ] {
            pool.line(line);
        }
        assert_eq!(
            pool.json(),
            json!({
                "left": [{ "package": "host-ninja", "jobs": 26, "times": 1 }],
                "serial": [
                    { "package": "?", "jobs": 1, "times": 1 },
                    { "package": "host-libzlib", "jobs": 1, "times": 2 },
                ],
            })
        );
    }
}
