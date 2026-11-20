//! The make jobs of this process's Buildroot builds, shared through one GNU
//! make jobserver.
//!
//! Every Buildroot make is given the job count on its command line, so two
//! builds at once would run twice the count. Instead, the builds of the
//! process share one pool: a named fifo that GNU make 4.4 and later read with
//! `--jobserver-auth=fifo:PATH` in `MAKEFLAGS`. A make takes a token from the
//! pool for each job it runs beyond its own first, and Buildroot's package
//! makes join the same pool, since Buildroot passes `MAKEFLAGS` down and does
//! not add `-j` to a make whose flags already have one.
//!
//! The count. A make runs its first job without a token (its implicit slot),
//! so the jobs in flight are at most the number of makes running plus the
//! tokens taken. A build takes a [`Slot`] before its make runs, and at most
//! [`BUILDS`] slots are given out, so at most that many makes run at once.
//! The pool holds `jobs - builds` tokens, and the bound is `jobs`. The fifo
//! holds fewer tokens when there are fewer job slots than builds (`jobs` below
//! [`BUILDS`]): then each build takes `jobs` slots and no tokens.
//!
//! Without a fifo (a make older than 4.4, or one whose version cannot be read),
//! there is one slot and no `MAKEFLAGS`: the builds take turns, each with the
//! whole job count, as before.
//!
//! The fifo is unlinked as soon as it is open. The pool keeps its descriptor,
//! open across exec, so every process this one starts has it at the same
//! number, and each make opens the same pipe through `/proc/self/fd/<n>`. The
//! pipe lives as long as this process does and leaves no name behind. The
//! name is the make's own, not this process's `/proc/<pid>/fd/<n>`: a make in
//! the build's sandbox (bwrap, in its own user namespace) may not open
//! another process's descriptors, and would fall back to one job.
//!
//! How full the pool is, over a build: a [`Sampler`] reads the fifo's byte
//! count (its free tokens) every [`SAMPLE`] while the build runs, and the
//! build records the series as the [`SAMPLE_DETAIL`] detail. A pool at zero
//! for long stretches is exhausted, and the builds wait on each other's
//! tokens; a pool with tokens left is not what holds a build back.

use rustix::fs::{CWD, Mode, mkfifoat};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The Buildroot builds one image runs at once: the root filesystem, the
/// kernel and the bootloader.
pub(crate) const BUILDS: usize = 3;

/// The pool of this process, made by the first build that asks. Every build
/// of the process has the same job count and make, so the first one sets both.
static POOL: OnceLock<Result<Jobserver, String>> = OnceLock::new();

/// How often a waiting build checks whether its run was cut short.
const WAIT: std::time::Duration = std::time::Duration::from_millis(20);

/// How often a [`Sampler`] reads the pool's free tokens.
const SAMPLE: Duration = Duration::from_secs(1);

/// The detail a build records its pool's free tokens under.
pub(crate) const SAMPLE_DETAIL: &str = "job-pool-free";

/// The pool's name for each fifo it makes, unique within the process.
static FIFOS: AtomicUsize = AtomicUsize::new(0);

/// The job slots of one process. See the module's notes for the count.
pub(crate) struct Jobserver {
    /// The job count the pool was made for.
    jobs: usize,
    /// `MAKEFLAGS` for each make of the pool; `None` without a fifo.
    makeflags: Option<String>,
    /// The fifo's descriptor, read and written, held so the pipe lives.
    _pipe: Option<File>,
    /// Slots not taken.
    free: Mutex<usize>,
    released: Condvar,
}

/// One build's slot. Held while its make runs, and given back when dropped,
/// on success, failure or unwinding alike.
pub(crate) struct Slot<'a> {
    pool: &'a Jobserver,
}

/// The pool of this process for `jobs` jobs, made with `make` on first use.
pub(crate) fn pool(make: &str, jobs: usize) -> Result<&'static Jobserver, String> {
    let jobs = jobs.max(1);
    let pool = POOL.get_or_init(|| {
        let fifo = make_has_fifo_jobserver(make);
        Jobserver::new(jobs, fifo).map_err(|error| format!("the make jobserver: {error}"))
    });
    match pool {
        Ok(pool) if pool.jobs == jobs => Ok(pool),
        Ok(pool) => Err(format!(
            "the make jobserver has {} jobs and a build asks for {jobs}: one process has one job count",
            pool.jobs
        )),
        Err(why) => Err(why.clone()),
    }
}

/// The builds a pool of `jobs` jobs runs at once with a fifo.
pub(crate) fn builds_for(jobs: usize) -> usize {
    jobs.clamp(1, BUILDS)
}

/// The tokens a pool of `jobs` jobs holds when `builds` builds run at once:
/// each running make has one job without one.
pub(crate) fn tokens(jobs: usize, builds: usize) -> usize {
    jobs.saturating_sub(builds)
}

/// Whether GNU make of this version reads a fifo jobserver: 4.4 and later.
pub(crate) fn version_has_fifo(version: &str) -> bool {
    let mut numbers = version
        .split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let major = numbers.next().unwrap_or(0);
    let minor = numbers.next().unwrap_or(0);
    (major, minor) >= (4, 4)
}

/// Whether `make` reads a fifo jobserver, by its `--version`.
pub(crate) fn make_has_fifo_jobserver(make: &str) -> bool {
    Command::new(make)
        .arg("--version")
        .output()
        .map(|out| version_has_fifo(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(false)
}

impl Jobserver {
    /// A pool of `jobs` job slots. With `fifo`, a fifo of `jobs - builds`
    /// tokens; without it, one slot and no tokens (the turns of the old shape).
    pub(crate) fn new(jobs: usize, fifo: bool) -> io::Result<Self> {
        let jobs = jobs.max(1);
        let builds = if fifo { builds_for(jobs) } else { 1 };
        let (pipe, makeflags) = if fifo {
            let (pipe, auth) = fifo_pipe(tokens(jobs, builds))?;
            (Some(pipe), Some(format!("-j --jobserver-auth={auth}")))
        } else {
            (None, None)
        };
        Ok(Self {
            jobs,
            makeflags,
            _pipe: pipe,
            free: Mutex::new(builds),
            released: Condvar::new(),
        })
    }

    /// Waits for a slot, and takes it. The wait ends early, with
    /// [`Refused::Cancelled`], when `cancelled` says the run has been cut
    /// short (Ctrl-C, or a sibling step that faulted under fail-fast): a
    /// build that never got a slot then never starts its make.
    pub(crate) fn admit(&self, cancelled: &dyn Fn() -> bool) -> Result<Slot<'_>, Refused> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancelled() {
                return Err(Refused::Cancelled);
            }
            if *free > 0 {
                *free -= 1;
                return Ok(Slot { pool: self });
            }
            free = self
                .released
                .wait_timeout(free, WAIT)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Slot<'_> {
    /// `MAKEFLAGS` for the make this slot runs; `None` without a fifo, when
    /// the make takes its count on its command line.
    pub(crate) fn makeflags(&self) -> Option<&str> {
        self.pool.makeflags.as_deref()
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut free = self
            .pool
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *free += 1;
        self.pool.released.notify_one();
    }
}

/// Why a build did not get its slot.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The run was cut short while the build waited.
    Cancelled,
    /// The pool could not be made.
    Pool(String),
}

/// This process's slot for a build with `make` and `jobs` jobs, waiting until
/// one of the process's builds gives its own back, or the run is cancelled.
pub(crate) fn admit(
    make: &str,
    jobs: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Slot<'static>, Refused> {
    pool(make, jobs).map_err(Refused::Pool)?.admit(cancelled)
}

/// Reads a pool's free tokens at a steady interval until it is finished.
pub(crate) struct Sampler {
    tokens: usize,
    every: Duration,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Vec<u64>>,
}

impl Sampler {
    /// Starts reading `pipe` (the pool's fifo, holding `tokens` when none is
    /// taken) every `every`.
    fn start(pipe: File, tokens: usize, every: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let begun = Instant::now();
            let mut free = Vec::new();
            let mut next = begun;
            while !stopped.load(Ordering::Relaxed) {
                // A read that fails is the one value no pool holds.
                free.push(rustix::io::ioctl_fionread(&pipe).unwrap_or(u64::MAX));
                next += every;
                if let Some(wait) = next.checked_duration_since(Instant::now()) {
                    std::thread::park_timeout(wait);
                }
            }
            free
        });
        Self {
            tokens,
            every,
            stop,
            thread,
        }
    }

    /// Stops reading, and gives the series as the detail: `{tokens,
    /// every_s, free: [...], empty_s, mean_free}`, `empty_s` the samples
    /// with no token free times the interval.
    pub(crate) fn finish(self) -> serde_json::Value {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.thread().unpark();
        let free = self.thread.join().unwrap_or_default();
        let read: Vec<u64> = free.iter().copied().filter(|n| *n != u64::MAX).collect();
        let empty = read.iter().filter(|n| **n == 0).count();
        let mean = if read.is_empty() {
            0.0
        } else {
            read.iter().sum::<u64>() as f64 / read.len() as f64
        };
        serde_json::json!({
            "tokens": self.tokens,
            "every_s": self.every.as_secs_f64(),
            "free": free,
            "empty_s": empty as f64 * self.every.as_secs_f64(),
            "mean_free": (mean * 100.0).round() / 100.0,
        })
    }
}

impl Jobserver {
    /// A sampler of this pool's free tokens every `every`; `None` without a
    /// fifo or when its descriptor cannot be copied.
    fn sampler(&self, every: Duration) -> Option<Sampler> {
        let pipe = self._pipe.as_ref()?.try_clone().ok()?;
        Some(Sampler::start(
            pipe,
            tokens(self.jobs, builds_for(self.jobs)),
            every,
        ))
    }
}

/// A sampler of this process's pool (for `make` and `jobs`, as [`admit`]),
/// reading every [`SAMPLE`]; `None` without a fifo pool.
pub(crate) fn sample(make: &str, jobs: usize) -> Option<Sampler> {
    pool(make, jobs).ok()?.sampler(SAMPLE)
}

/// A fifo holding `tokens` tokens, and the `fifo:` name of it that a make is
/// given: the pipe, kept open across exec and opened through the make's own
/// `/proc/self` entry.
fn fifo_pipe(tokens: usize) -> io::Result<(File, String)> {
    let index = FIFOS.fetch_add(1, Ordering::Relaxed);
    let path: PathBuf =
        std::env::temp_dir().join(format!("gaia-jobserver-{}-{index}", std::process::id()));
    mkfifoat(CWD, &path, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
    let opened = OpenOptions::new().read(true).write(true).open(&path);
    // The name goes now. A failure to remove it leaves an empty fifo, so it
    // is not an error of its own.
    let _ = fs::remove_file(&path);
    let mut pipe = opened?;
    pipe.write_all(&vec![b'+'; tokens])?;
    rustix::io::fcntl_setfd(&pipe, rustix::io::FdFlags::empty()).map_err(io::Error::from)?;
    let fd = pipe.as_raw_fd();
    Ok((pipe, format!("fifo:/proc/self/fd/{fd}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// The Makefile of the bound tests: sixteen targets, each one logging
    /// its start and its end to `LOGFILE` and sleeping in between. A failing
    /// target is `fail`; the other targets are `t1` to `t16`.
    const MAKEFILE: &str = "\
all: t1 t2 t3 t4 t5 t6 t7 t8 t9 t10 t11 t12 t13 t14 t15 t16
fail:
\t@echo start fail >> $(LOGFILE); exit 1
t%:
\t@echo start $@ >> $(LOGFILE); sleep 0.3; echo end $@ >> $(LOGFILE)
";

    /// The most recipes that ran at once in a log: starts minus ends, in
    /// the order the lines were written.
    fn most_running(log: &str) -> usize {
        let (mut running, mut most) = (0usize, 0usize);
        for line in log.lines() {
            if line.starts_with("start") {
                running += 1;
                most = most.max(running);
            } else if line.starts_with("end") {
                running = running.saturating_sub(1);
            }
        }
        most
    }

    /// The make of one build, started the way the step starts it: the
    /// `MAKEFLAGS` of its slot and no `-j` on the command line, or the job
    /// count on the command line when the slot has none.
    fn build_make(dir: &std::path::Path, slot: &Slot<'_>, jobs: usize, goal: &str) -> Command {
        let mut command = Command::new("/usr/bin/make");
        command
            .current_dir(dir)
            .env_clear()
            .env("PATH", crate::PATH)
            .arg("--no-print-directory")
            .arg(goal)
            .arg(format!("LOGFILE={}", dir.join("log").display()));
        match slot.makeflags() {
            Some(flags) => {
                command.env("MAKEFLAGS", flags);
            }
            None => {
                command.arg(format!("-j{jobs}"));
            }
        }
        command
    }

    fn write_makefile(dir: &std::path::Path) {
        std::fs::write(dir.join("Makefile"), MAKEFILE).expect("the Makefile");
    }

    /// `builds` makes at once, each taking its slot from `pool` the way a
    /// build does, and returns the log's most recipes running at once.
    fn run_builds(pool: &Jobserver, builds: usize, goal: &str) -> usize {
        let dir = tempfile::tempdir().expect("a directory");
        write_makefile(dir.path());
        std::thread::scope(|scope| {
            for _ in 0..builds {
                scope.spawn(|| {
                    let slot = pool.admit(&|| false).expect("a slot");
                    let status = build_make(dir.path(), &slot, pool.jobs, goal)
                        .status()
                        .expect("make");
                    assert!(goal == "fail" || status.success(), "{status}");
                });
            }
        });
        most_running(&std::fs::read_to_string(dir.path().join("log")).unwrap_or_default())
    }

    fn fifo_pool(jobs: usize) -> Option<Jobserver> {
        if !make_has_fifo_jobserver("/usr/bin/make") {
            eprintln!("skipped: /usr/bin/make is older than 4.4");
            return None;
        }
        Some(Jobserver::new(jobs, true).expect("a pool"))
    }

    /// A make in the sandbox the step runs it in (bwrap, with its own user
    /// namespace, for the fixed build path) reads the pool as one outside it
    /// does: recipes overlap, and make never falls back to one job.
    #[test]
    fn a_build_in_the_sandbox_reads_the_pool() {
        const BWRAP: &str = "/usr/bin/bwrap";
        let Some(pool) = fifo_pool(13) else { return };
        if !std::path::Path::new(BWRAP).is_file() {
            eprintln!("skipped: no {BWRAP}");
            return;
        }
        let dir = tempfile::tempdir().expect("a directory");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).expect("out");
        write_makefile(&out);
        let host = gaia_types::BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: String::new(),
            cpus: String::new(),
            jobs: 13,
            download_dir: String::new(),
            download_pool: String::new(),
            ccache_dir: String::new(),
            mirror: String::new(),
            config_only: false,
            dev_reuse: String::new(),
            bwrap: BWRAP.into(),
        };
        let slot = pool.admit(&|| false).expect("a slot");
        let make = build_make(&out, &slot, pool.jobs, "all");
        let mut words = vec![make.get_program().to_string_lossy().into_owned()];
        words.extend(
            make.get_args()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        let words = crate::place::wrapped(&host, &out, words);
        let mut command = Command::new(&words[0]);
        command.args(&words[1..]).current_dir(&out).env_clear();
        for (key, value) in make.get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        let output = command.output().expect("bwrap");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(!stderr.contains("jobserver"), "{stderr}");
        let most = most_running(&std::fs::read_to_string(out.join("log")).unwrap_or_default());
        assert!(most > 1, "{most} recipe at once in the sandbox: {stderr}");
    }

    /// The sampler reads the tokens left in the fifo: all of them, then two
    /// fewer while two are taken; a pool without a fifo has no sampler.
    #[test]
    fn the_sampler_reads_the_free_tokens() {
        let pool = Jobserver::new(8, true).expect("a pool");
        let sampler = pool.sampler(Duration::from_millis(10)).expect("a sampler");
        std::thread::sleep(Duration::from_millis(80));
        let mut taker = pool
            ._pipe
            .as_ref()
            .expect("a fifo")
            .try_clone()
            .expect("dup");
        let mut taken = [0_u8; 2];
        taker.read_exact(&mut taken).expect("two tokens");
        std::thread::sleep(Duration::from_millis(80));
        let detail = sampler.finish();
        let free: Vec<u64> = detail["free"]
            .as_array()
            .expect("free")
            .iter()
            .map(|n| n.as_u64().expect("a count"))
            .collect();
        assert_eq!(detail["tokens"], 5, "8 jobs less 3 builds");
        assert_eq!(free.first(), Some(&5), "{free:?}");
        assert_eq!(free.last(), Some(&3), "{free:?}");
        assert!(free.iter().all(|n| *n == 5 || *n == 3), "{free:?}");
        assert!(free.len() >= 8, "about one sample per 10 ms: {free:?}");
        assert_eq!(detail["empty_s"], 0.0);
        assert!(
            Jobserver::new(8, false)
                .expect("no fifo")
                .sampler(SAMPLE)
                .is_none()
        );
    }

    #[test]
    fn make_versions_with_a_fifo_are_four_four_and_later() {
        assert!(version_has_fifo("GNU Make 4.4\n"));
        assert!(version_has_fifo("GNU Make 4.4.1\nCopyright"));
        assert!(version_has_fifo("GNU Make 5.0"));
        assert!(!version_has_fifo("GNU Make 4.3\n"));
        assert!(!version_has_fifo("GNU Make 3.81\n"));
        assert!(!version_has_fifo(""));
    }

    /// The count: `builds` makes each run one job without a token, so the
    /// tokens are the job count less the builds.
    #[test]
    fn the_pool_holds_the_job_count_less_the_builds() {
        assert_eq!(builds_for(13), 3);
        assert_eq!(tokens(13, builds_for(13)), 10);
        assert_eq!(builds_for(2), 2);
        assert_eq!(tokens(2, builds_for(2)), 0);
        assert_eq!(builds_for(1), 1);
        assert_eq!(tokens(1, builds_for(1)), 0);
    }

    /// The bound, with real makes: three builds at once, each started as the
    /// step starts it, share the thirteen jobs. More than one build's worth
    /// of recipes must overlap, and never more than the bound.
    #[test]
    fn three_builds_share_the_bound_and_overlap() {
        let Some(pool) = fifo_pool(13) else { return };
        let most = run_builds(&pool, 3, "all");
        eprintln!(
            "measured: {most} recipes at once, three builds, bound {}",
            pool.jobs
        );
        assert!(most <= 13, "{most} recipes ran at once, the bound is 13");
        assert!(
            most > 3,
            "only {most} recipes ran at once: no overlap beyond one job per make"
        );
    }

    /// The old shape: a turn for each build, each make with the whole job
    /// count. Nothing overlaps across builds.
    #[test]
    fn the_turn_shape_runs_one_build_at_a_time() {
        let pool = Jobserver::new(13, false).expect("a pool");
        assert_eq!(*pool.free.lock().expect("the slots"), 1);
        let dir = tempfile::tempdir().expect("a directory");
        write_makefile(dir.path());
        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| {
                    let slot = pool.admit(&|| false).expect("a slot");
                    let status = build_make(dir.path(), &slot, 13, "all")
                        .status()
                        .expect("make");
                    assert!(status.success());
                });
            }
        });
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap_or_default();
        eprintln!(
            "measured: {} recipes at once, turn shape",
            most_running(&log)
        );
        assert_eq!(
            most_running(&log),
            13,
            "one build's own jobs, not three builds'"
        );
    }

    /// A build whose make fails: its tokens go back as its jobs finish, so
    /// the other build finishes, and a later build still gets the whole pool.
    #[test]
    fn a_failing_build_gives_its_tokens_back() {
        let Some(pool) = fifo_pool(13) else { return };
        let dir = tempfile::tempdir().expect("a directory");
        write_makefile(dir.path());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let slot = pool.admit(&|| false).expect("a slot");
                let status = build_make(dir.path(), &slot, 13, "fail")
                    .status()
                    .expect("make");
                assert!(!status.success());
            });
            scope.spawn(|| {
                let slot = pool.admit(&|| false).expect("a slot");
                let status = build_make(dir.path(), &slot, 13, "all")
                    .status()
                    .expect("make");
                assert!(status.success(), "the other build failed: {status}");
            });
        });
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap_or_default();
        assert_eq!(
            log.matches("end t").count(),
            16,
            "the other build did not finish"
        );
        // The pool is whole again: a later build overlaps as before.
        let later = run_builds(&pool, 1, "all");
        assert!(
            later > 3,
            "after the failure only {later} recipes ran at once"
        );
    }

    /// A make killed while it holds tokens: the pool loses those tokens, and
    /// the other build still finishes, since each make keeps its own job.
    #[test]
    fn a_killed_build_does_not_wedge_the_others() {
        let Some(pool) = fifo_pool(13) else { return };
        let dir = tempfile::tempdir().expect("a directory");
        write_makefile(dir.path());
        let victim_slot = pool.admit(&|| false).expect("a slot");
        let mut victim = build_make(dir.path(), &victim_slot, 13, "all")
            .spawn()
            .expect("make");
        std::thread::sleep(Duration::from_millis(600));
        victim.kill().expect("the kill");
        let _ = victim.wait();
        drop(victim_slot);
        let other_slot = pool.admit(&|| false).expect("a slot");
        let mut other = build_make(dir.path(), &other_slot, 13, "all")
            .spawn()
            .expect("make");
        let started = Instant::now();
        loop {
            if let Some(status) = other.try_wait().expect("wait") {
                assert!(status.success(), "{status}");
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the build hangs after its neighbour was killed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A build waiting for a slot returns when the run is cancelled, and
    /// does not wait for the slot to be given back.
    #[test]
    fn a_waiting_build_returns_when_the_run_is_cancelled() {
        use std::sync::atomic::AtomicBool;
        let pool = Jobserver::new(4, false).expect("a pool");
        let held = pool.admit(&|| false).expect("the one slot");
        let cancel = AtomicBool::new(false);
        let (sent, outcome) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let refused = pool.admit(&|| cancel.load(Ordering::SeqCst)).is_err();
                let _ = sent.send(refused);
            });
            std::thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::SeqCst);
            let seen = outcome.recv_timeout(Duration::from_secs(5));
            // The slot goes back before the check, so a wait that did not
            // end on the cancel ends here and the test fails, not hangs.
            drop(held);
            assert_eq!(seen, Ok(true), "the waiting build did not see the cancel");
        });
    }

    /// The slot is given back when dropped, so a waiting build gets it.
    #[test]
    fn a_slot_is_given_back_when_dropped() {
        let pool = Jobserver::new(4, false).expect("a pool");
        let first = pool.admit(&|| false).expect("a slot");
        drop(first);
        let _second = pool.admit(&|| false).expect("a slot");
    }
}
