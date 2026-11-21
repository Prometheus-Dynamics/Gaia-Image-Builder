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
//! Yielding. A build with slack (see `gaia_step::slack`) takes a yielding
//! slot: its make reads a second fifo, with tokens of its own (as many as
//! the main fifo's), and runs [`NICE_STEP`] nicer than Gaia itself. The
//! builds without slack keep every token of the main fifo, and the
//! processor goes to them first: on a processor both want they get
//! [`SPLIT`] against a yielding build, which runs at full speed on what they
//! leave idle. The step is relative, not the lowest priority: niceness
//! orders work inside one cgroup only (here, Gaia's unit, which other work
//! on the machine is weighed against as a whole), and at the lowest the
//! steps of a yielding build that hold no tokens to give back sat behind
//! every job of the builds beside it (1024 : 15). In a cgroup shared with
//! unrelated processes (a terminal's scope) the step orders Gaia's work
//! against theirs too. A yielding make has its own first job like any
//! other. Its jobs are beside the bound: they take only
//! what the builds of the bound leave, so what is beyond it is processes
//! waiting, not CPU taken from them. (Tokens are not lent from one fifo to
//! the other: a make takes a token it gives back again at once, so tokens
//! once lent could not be taken back for the builds without slack.) A
//! [`Yield`] says how many of its tokens a yielding build held, over time,
//! while builds without slack ran beside it.
//!
//! The yielding fifo is made with the first yielding build, and only where
//! niceness takes effect ([`nice_takes_effect`]): under cgroup
//! v1, or in a cgroup without the cpu controller, the kernel's autogroups
//! give each session (each sandbox) an equal share whatever its niceness,
//! and a second pool there would be plain oversubscription. Then a yielding
//! build shares the main fifo as any build does, and says why. Its tokens
//! are sized from memory as well as jobs ([`yield_tokens`]), since its jobs
//! are beside the bound: as many as the main fifo's, but no more than the
//! memory available beyond Gaia's reserve holds at [`JOB_BYTES`] a job, and
//! none when that is less than one. A yielding make always has its first
//! job.
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

/// How much nicer than Gaia a yielding build's make runs. Each step is a
/// factor of about 1.25 in the kernel's weights; measured with two burners
/// on one processor (`a_yielding_build_gets_about_a_fifth_of_a_contended_processor`),
/// three runs at a load of 55 to 75 on 32 processors gave 3.1 to 3.8 : 1 at
/// +5 and 4.7 to 4.9 : 1 at +7 (see [`SPLIT`]).
pub(crate) const NICE_STEP: i32 = 7;

/// The share of a contended processor the builds beside a yielding one get
/// against it, at [`NICE_STEP`], as the records say it.
pub(crate) const SPLIT: &str = "about 5 : 1 inside Gaia's unit";

/// The niceness a yielding build's make runs at: Gaia's own and
/// [`NICE_STEP`] more, at most 19.
pub(crate) fn yielding_niceness() -> i32 {
    niceness_beside(rustix::process::getpriority_process(None).unwrap_or(0))
}

/// The niceness a yielding build runs at beside a process at `own`.
fn niceness_beside(own: i32) -> i32 {
    (own + NICE_STEP).min(19)
}

/// The memory one job of a yielding build is taken to need, for sizing its
/// fifo: a compiler job of a large C or C++ file. A link of a large program
/// takes more, and is one job among many.
pub(crate) const JOB_BYTES: u64 = 1 << 30;

/// How often the count of a yielding build's held tokens is read.
const COUNT: Duration = Duration::from_millis(100);

/// The detail a build records what yielding did under.
pub(crate) const YIELD_DETAIL: &str = "job-pool-yield";

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
    /// The yielding fifo, made with the first yielding slot of a pool with a
    /// fifo; or why the yielding builds share the main one.
    yielding: Arc<OnceLock<Result<Yielding, String>>>,
    /// How the yielding fifo's tokens are counted: [`yield_size`], or a
    /// test's.
    size: fn(usize) -> Result<usize, String>,
    /// The builds running, and what the yielding ones held.
    count: Arc<Mutex<Count>>,
    /// The counting thread, started with the yielding fifo.
    counting: OnceLock<()>,
}

/// The second fifo of a pool, for the builds that yield.
struct Yielding {
    /// The fifo's descriptor, held so the pipe lives.
    pipe: File,
    /// Its tokens when none is taken.
    tokens: u64,
    makeflags: String,
}

/// The builds running, and what the yielding ones held beside main builds.
#[derive(Debug, Default)]
struct Count {
    /// The labels of the main builds running, one entry per slot.
    main: Vec<String>,
    /// The yielding builds running.
    yielding: usize,
    /// The yielding fifo's tokens held while main builds ran, summed over
    /// time, in token-nanoseconds.
    beside_ns: u128,
    /// The labels of the main builds seen running beside a yielding one.
    seen: Vec<String>,
}

/// One build's slot. Held while its make runs, and given back when dropped,
/// on success, failure or unwinding alike.
pub(crate) struct Slot<'a> {
    pool: &'a Jobserver,
    /// Whether the build yields, and its label.
    yields: bool,
    label: String,
    /// Why a build asked to yield shares the main fifo, when it does.
    shares: Option<String>,
    /// The count when the slot was taken.
    beside_ns: u128,
}

/// What yielding did over one yielding build: the main builds it yielded
/// to, the jobs of the yielding pool (its tokens and a make's own job), and
/// the token-seconds the yielding builds held while main builds ran beside
/// them, at the yielding niceness.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Yield {
    pub(crate) to: Vec<String>,
    pub(crate) pool_jobs: u64,
    pub(crate) beside_token_s: f64,
}

/// Whether niceness takes effect for this process's children,
/// from the autogroup switch (`/proc/sys/kernel/sched_autogroup_enabled`,
/// `None` when absent), `/proc/self/cgroup`, and the `cgroup.controllers`
/// of a cgroup v2 path. With autogroups off it does. With them on, a
/// process in the root cgroup, or under cgroup v1, has its session's
/// autogroup weighed against the others' whatever its niceness; in a cgroup
/// v2 group with the cpu controller, the group is weighed and niceness
/// counts within it.
pub(crate) fn nice_takes_effect(
    autogroup: Option<&str>,
    cgroup: &str,
    controllers: impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    if autogroup.is_none_or(|on| on.trim() == "0") {
        return Ok(());
    }
    let mut lines = cgroup.lines().filter(|line| !line.is_empty());
    let path = match (
        lines.next().and_then(|line| line.strip_prefix("0::")),
        lines.next(),
    ) {
        (Some(path), None) => path.trim(),
        _ => return Err("cgroup v1, where autogroups ignore niceness".into()),
    };
    if path == "/" {
        return Err("the root cgroup, where autogroups ignore niceness".into());
    }
    let delegated = controllers(path).is_some_and(|list| {
        list.split_whitespace()
            .any(|controller| controller == "cpu")
    });
    if delegated {
        Ok(())
    } else {
        Err(format!(
            "no cpu controller in cgroup {path}, where autogroups ignore niceness"
        ))
    }
}

/// The tokens of a yielding fifo: the main fifo's `main_tokens`, but no
/// more than `available` bytes of memory (MemAvailable, less what this
/// process holds) beyond Gaia's reserve (`gaia_store::RESERVE_BYTES`) hold
/// at [`JOB_BYTES`] a job; none when the memory is unknown.
pub(crate) fn yield_tokens(main_tokens: usize, available: Option<u64>) -> usize {
    let headroom = available.map_or(0, |bytes| bytes.saturating_sub(gaia_store::RESERVE_BYTES));
    main_tokens.min(usize::try_from(headroom / JOB_BYTES).unwrap_or(usize::MAX))
}

/// This machine's yielding fifo size for a pool of `main_tokens` tokens, or
/// why there is none.
fn yield_size(main_tokens: usize) -> Result<usize, String> {
    let autogroup = fs::read_to_string("/proc/sys/kernel/sched_autogroup_enabled").ok();
    let cgroup = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    nice_takes_effect(autogroup.as_deref(), &cgroup, |path| {
        fs::read_to_string(format!("/sys/fs/cgroup{path}/cgroup.controllers")).ok()
    })?;
    let available = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| gaia_store::parse_mem_available(&text))
        .map(|bytes| bytes.saturating_sub(gaia_store::held_memory()));
    Ok(yield_tokens(main_tokens, available))
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
            yielding: Arc::new(OnceLock::new()),
            size: yield_size,
            count: Arc::new(Mutex::new(Count::default())),
            counting: OnceLock::new(),
        })
    }

    /// This pool with its yielding fifo sized by `size` instead of
    /// [`yield_size`].
    #[cfg(test)]
    fn sized(mut self, size: fn(usize) -> Result<usize, String>) -> Self {
        self.size = size;
        self
    }

    /// The yielding fifo, made now if it is not yet; or why there is none.
    fn yielding(&self) -> Result<&Yielding, &str> {
        let made = self.yielding.get_or_init(|| {
            if self.makeflags.is_none() {
                return Err("the pool has no fifo".into());
            }
            let tokens = (self.size)(tokens(self.jobs, builds_for(self.jobs)))?;
            let (pipe, auth) =
                fifo_pipe(tokens).map_err(|error| format!("the yielding fifo: {error}"))?;
            Ok(Yielding {
                pipe,
                tokens: tokens as u64,
                makeflags: format!("-j --jobserver-auth={auth}"),
            })
        });
        made.as_ref().map_err(String::as_str)
    }

    /// [`Jobserver::admit_as`] for a main build with no label.
    #[cfg(test)]
    pub(crate) fn admit(&self, cancelled: &dyn Fn() -> bool) -> Result<Slot<'_>, Refused> {
        self.admit_as(cancelled, false, "")
    }

    /// Waits for a slot, and takes it. The wait ends early, with
    /// [`Refused::Cancelled`], when `cancelled` says the run has been cut
    /// short (Ctrl-C, or a sibling step that faulted under fail-fast): a
    /// build that never got a slot then never starts its make.
    ///
    /// With `yields`, the build yields to the main builds (see the module's
    /// notes), when the pool has a fifo; `label` names the build in the
    /// other builds' [`Yield`].
    pub(crate) fn admit_as(
        &self,
        cancelled: &dyn Fn() -> bool,
        yields: bool,
        label: &str,
    ) -> Result<Slot<'_>, Refused> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancelled() {
                return Err(Refused::Cancelled);
            }
            if *free > 0 {
                *free -= 1;
                drop(free);
                return Ok(self.slot(yields, label));
            }
            free = self
                .released
                .wait_timeout(free, WAIT)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Jobserver {
    /// A slot taken from the count, registered with the count of the
    /// builds; a yielding one makes the yielding fifo, or shares the main
    /// one and says why.
    fn slot(&self, yields: bool, label: &str) -> Slot<'_> {
        let (yields, shares) = match yields.then(|| self.yielding()) {
            Some(Ok(_)) => (true, None),
            Some(Err(why)) => (false, Some(why.to_string())),
            None => (false, None),
        };
        let mut count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        if yields {
            count.yielding += 1;
            if count.yielding == 1 {
                count.seen.clear();
            }
            let main = count.main.clone();
            count.seen.extend(main);
            count.seen.sort();
            count.seen.dedup();
        } else {
            count.main.push(label.to_string());
            if count.yielding > 0 && !count.seen.iter().any(|seen| seen == label) {
                count.seen.push(label.to_string());
            }
        }
        let beside_ns = count.beside_ns;
        drop(count);
        if yields {
            self.counting.get_or_init(|| self.start_count());
        }
        Slot {
            pool: self,
            yields,
            label: label.to_string(),
            shares,
            beside_ns,
        }
    }

    /// Starts the thread that counts, every [`COUNT`] for the life of the
    /// process, the yielding fifo's tokens held while main builds run.
    fn start_count(&self) {
        let Ok(yielding) = self.yielding() else {
            return;
        };
        let Ok(pipe) = yielding.pipe.try_clone() else {
            return;
        };
        let (tokens, count) = (yielding.tokens, Arc::clone(&self.count));
        std::thread::spawn(move || {
            let mut last = Instant::now();
            loop {
                std::thread::sleep(COUNT);
                let now = Instant::now();
                let held =
                    tokens.saturating_sub(rustix::io::ioctl_fionread(&pipe).unwrap_or(tokens));
                let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
                if count.yielding > 0 && !count.main.is_empty() {
                    count.beside_ns += u128::from(held) * now.duration_since(last).as_nanos();
                }
                last = now;
            }
        });
    }
}

impl Slot<'_> {
    /// `MAKEFLAGS` for the make this slot runs; `None` without a fifo, when
    /// the make takes its count on its command line. A yielding slot's make
    /// reads the yielding fifo.
    pub(crate) fn makeflags(&self) -> Option<&str> {
        match (self.yields, self.pool.yielding.get()) {
            (true, Some(Ok(yielding))) => Some(&yielding.makeflags),
            _ => self.pool.makeflags.as_deref(),
        }
    }

    /// The niceness this slot's make runs at, when not the step's own:
    /// [`yielding_niceness`], for a yielding build.
    pub(crate) fn nice(&self) -> Option<i32> {
        self.yields.then(yielding_niceness)
    }

    /// Why this build, asked to yield, shares the main fifo instead.
    pub(crate) fn shares(&self) -> Option<&str> {
        self.shares.as_deref()
    }

    /// What yielding did since this yielding slot was taken; `None` for a
    /// main slot.
    pub(crate) fn yielded(&self) -> Option<Yield> {
        let Some(Ok(yielding)) = self.pool.yielding.get().filter(|_| self.yields) else {
            return None;
        };
        let count = self
            .pool
            .count
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Some(Yield {
            to: count.seen.clone(),
            pool_jobs: yielding.tokens + 1,
            beside_token_s: (count.beside_ns - self.beside_ns) as f64 / 1e9,
        })
    }
}

impl Yield {
    /// As the step records it.
    pub(crate) fn detail(&self, why: &str) -> serde_json::Value {
        serde_json::json!({
            "yielded": true,
            "why": why,
            "to": self.to,
            "nice": yielding_niceness(),
            "nice_step": NICE_STEP,
            "split": SPLIT,
            "pool_jobs": self.pool_jobs,
            "pool_rule": format!(
                "min(main tokens, (MemAvailable - {} GiB reserve) / {} GiB a job) tokens, and make's own job",
                gaia_store::RESERVE_BYTES >> 30,
                JOB_BYTES >> 30
            ),
            "beside_token_s": (self.beside_token_s * 10.0).round() / 10.0,
        })
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut count = self
            .pool
            .count
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.yields {
            count.yielding = count.yielding.saturating_sub(1);
        } else if let Some(at) = count.main.iter().position(|label| *label == self.label) {
            count.main.remove(at);
        }
        drop(count);
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
/// one of the process's builds gives its own back, or the run is cancelled;
/// a yielding one with `yields` (see [`Jobserver::admit_as`]).
pub(crate) fn admit(
    make: &str,
    jobs: usize,
    cancelled: &dyn Fn() -> bool,
    yields: bool,
    label: &str,
) -> Result<Slot<'static>, Refused> {
    pool(make, jobs)
        .map_err(Refused::Pool)?
        .admit_as(cancelled, yields, label)
}

/// Reads a pool's free tokens at a steady interval until it is finished:
/// the main fifo's, and the yielding fifo's apart.
pub(crate) struct Sampler {
    tokens: usize,
    every: Duration,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<(Vec<u64>, Vec<Option<u64>>)>,
}

impl Sampler {
    /// Starts reading `pipe` (the pool's fifo, holding `tokens` when none is
    /// taken) every `every`.
    fn start(
        pipe: File,
        low: Arc<OnceLock<Result<Yielding, String>>>,
        tokens: usize,
        every: Duration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let begun = Instant::now();
            let (mut free, mut free_yielding) = (Vec::new(), Vec::new());
            let mut next = begun;
            while !stopped.load(Ordering::Relaxed) {
                // A read that fails is the one value no pool holds.
                free.push(rustix::io::ioctl_fionread(&pipe).unwrap_or(u64::MAX));
                free_yielding.push(match low.get() {
                    Some(Ok(low)) => rustix::io::ioctl_fionread(&low.pipe).ok(),
                    _ => None,
                });
                next += every;
                if let Some(wait) = next.checked_duration_since(Instant::now()) {
                    std::thread::park_timeout(wait);
                }
            }
            (free, free_yielding)
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
    /// with no token free times the interval, all of the main fifo; and
    /// `free_yielding: [...]`, the yielding fifo's once it is made (null
    /// before), when it was made in that time.
    pub(crate) fn finish(self) -> serde_json::Value {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.thread().unpark();
        let (free, free_yielding) = self.thread.join().unwrap_or_default();
        let read: Vec<u64> = free.iter().copied().filter(|n| *n != u64::MAX).collect();
        let empty = read.iter().filter(|n| **n == 0).count();
        let mean = if read.is_empty() {
            0.0
        } else {
            read.iter().sum::<u64>() as f64 / read.len() as f64
        };
        let mut detail = serde_json::json!({
            "tokens": self.tokens,
            "every_s": self.every.as_secs_f64(),
            "free": free,
            "empty_s": empty as f64 * self.every.as_secs_f64(),
            "mean_free": (mean * 100.0).round() / 100.0,
        });
        // The yielding fifo's, once it was made, when a build ran in it in
        // this one's time (null before it was made).
        if free_yielding.iter().any(Option::is_some) {
            detail["free_yielding"] = free_yielding.into();
        }
        detail
    }
}

impl Jobserver {
    /// A sampler of this pool's free tokens every `every`; `None` without a
    /// fifo or when its descriptor cannot be copied.
    fn sampler(&self, every: Duration) -> Option<Sampler> {
        let pipe = self._pipe.as_ref()?.try_clone().ok()?;
        Some(Sampler::start(
            pipe,
            Arc::clone(&self.yielding),
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
    use std::os::unix::process::CommandExt;

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

    /// The Makefile of the yield tests: `main` (8 recipes) and `side` (24),
    /// each recipe burning the processor for a while and logging its start
    /// and end with the time in nanoseconds.
    const BURN: &str = "\
main: a1 a2 a3 a4 a5 a6 a7 a8
side: b1 b2 b3 b4 b5 b6 b7 b8 b9 b10 b11 b12 b13 b14 b15 b16 b17 b18 b19 b20 b21 b22 b23 b24
a% b%:
\t@echo start $$(date +%s%N) >> $(LOGFILE); i=0; while [ $$i -lt 40000 ]; do i=$$((i+1)); done; echo end $$(date +%s%N) >> $(LOGFILE)
";

    /// The free tokens in each fifo of `pool`: main, yielding.
    fn free(pool: &Jobserver) -> (u64, u64) {
        let main = pool._pipe.as_ref().expect("a fifo");
        let low = &pool
            .yielding
            .get()
            .expect("made")
            .as_ref()
            .expect("a yielding fifo")
            .pipe;
        (
            rustix::io::ioctl_fionread(main).expect("main"),
            rustix::io::ioctl_fionread(low).expect("low"),
        )
    }

    /// A yielding build's make reads a fifo of its own, sized as the pool
    /// says (here as many tokens as the main one), and runs at the lowest
    /// priority; a main build's make reads the main fifo at the step's own.
    /// The tokens it holds while a main build runs are counted, and the main
    /// builds it ran beside named.
    #[test]
    fn a_yielding_build_has_a_fifo_of_its_own_and_the_lowest_priority() {
        let pool = Jobserver::new(13, true).expect("a pool").sized(Ok);
        let main = pool
            .admit_as(&|| false, false, "path")
            .expect("a main slot");
        let low = pool
            .admit_as(&|| false, true, "beside")
            .expect("a yielding slot");
        assert_ne!(main.makeflags(), low.makeflags());
        assert!(
            low.makeflags()
                .is_some_and(|flags| flags.starts_with("-j --jobserver-auth=fifo:"))
        );
        assert_eq!((main.nice(), low.nice()), (None, Some(yielding_niceness())));
        assert_eq!(free(&pool), (10, 10), "13 jobs less 3 builds, in each");
        let mut taker = pool
            .yielding
            .get()
            .expect("made")
            .as_ref()
            .expect("a yielding fifo")
            .pipe
            .try_clone()
            .expect("dup");
        let mut taken = [0_u8; 3];
        taker.read_exact(&mut taken).expect("three tokens");
        std::thread::sleep(Duration::from_millis(500));
        let done = low.yielded().expect("a yielding slot");
        assert_eq!(done.to, vec!["path".to_string()]);
        assert_eq!(done.pool_jobs, 11, "ten tokens and make's own job");
        assert!(
            (0.9..=2.1).contains(&done.beside_token_s),
            "three tokens for about half a second: {done:?}"
        );
        assert!(main.yielded().is_none());
        // With no main build beside it, nothing more is counted.
        drop(main);
        std::thread::sleep(Duration::from_millis(300));
        let after = low.yielded().expect("a yielding slot").beside_token_s;
        assert!(after - done.beside_token_s < 0.4, "{after} after {done:?}");
        taker.write_all(&taken).expect("given back");
        assert_eq!(free(&pool), (10, 10));
        // A pool without a fifo has no yielding slot.
        let turns = Jobserver::new(4, false).expect("no fifo");
        let slot = turns.admit_as(&|| false, true, "beside").expect("a slot");
        assert_eq!((slot.nice(), slot.makeflags()), (None, None));
        assert_eq!(slot.shares(), Some("the pool has no fifo"));
    }

    /// Where niceness would not take effect, a build asked to
    /// yield shares the main fifo at the step's own priority, and says why.
    #[test]
    fn where_niceness_has_no_effect_a_yielding_build_shares_the_main_fifo() {
        let pool = Jobserver::new(13, true)
            .expect("a pool")
            .sized(|_| Err("cgroup v1, where autogroups ignore niceness".into()));
        let main = pool
            .admit_as(&|| false, false, "path")
            .expect("a main slot");
        let asked = pool.admit_as(&|| false, true, "beside").expect("a slot");
        assert_eq!(asked.makeflags(), main.makeflags());
        assert_eq!(asked.nice(), None);
        assert!(asked.yielded().is_none());
        assert_eq!(
            asked.shares(),
            Some("cgroup v1, where autogroups ignore niceness")
        );
    }

    /// With little memory beyond the reserve the yielding fifo is small, and
    /// with none it is empty: the yielding make runs its own job only.
    #[test]
    fn a_plant_with_little_memory_gives_a_small_yielding_pool() {
        const GIB: u64 = 1 << 30;
        let reserve = gaia_store::RESERVE_BYTES;
        assert_eq!(
            yield_tokens(23, Some(reserve + 100 * GIB)),
            23,
            "jobs bound it"
        );
        assert_eq!(yield_tokens(23, Some(reserve + 4 * GIB + GIB / 2)), 4);
        assert_eq!(yield_tokens(23, Some(reserve)), 0);
        assert_eq!(yield_tokens(23, Some(8 * GIB)), 0, "a 16 GB machine");
        assert_eq!(yield_tokens(23, None), 0, "memory unknown");
        let pool = Jobserver::new(13, true).expect("a pool").sized(|_| Ok(0));
        let low = pool.admit_as(&|| false, true, "beside").expect("a slot");
        assert_eq!(low.nice(), Some(yielding_niceness()));
        assert_eq!(free(&pool).1, 0);
        assert_eq!(low.yielded().expect("yielding").pool_jobs, 1);
    }

    /// Niceness takes effect with autogroups off, or in a cgroup v2 group
    /// with the cpu controller; not under cgroup v1, in the root cgroup, or
    /// in a group without the cpu controller.
    #[test]
    fn niceness_takes_effect_only_where_the_cpu_controller_weighs_it() {
        let with_cpu = |path: &str| {
            (path == "/user.slice/app.slice/gaia-x.service")
                .then(|| "cpu io memory pids\n".to_string())
        };
        let v2 = "0::/user.slice/app.slice/gaia-x.service\n";
        assert_eq!(nice_takes_effect(Some("1\n"), v2, with_cpu), Ok(()));
        assert_eq!(
            nice_takes_effect(Some("0\n"), "12:cpu,cpuacct:/\n0::/\n", |_| None),
            Ok(())
        );
        assert_eq!(nice_takes_effect(None, "0::/\n", |_| None), Ok(()));
        assert!(
            nice_takes_effect(
                Some("1"),
                "12:cpu,cpuacct:/user\n1:name=systemd:/user\n",
                |_| None
            )
            .is_err_and(|why| why.starts_with("cgroup v1"))
        );
        assert!(
            nice_takes_effect(Some("1"), "0::/\n", |_| None)
                .is_err_and(|why| why.starts_with("the root cgroup"))
        );
        assert!(
            nice_takes_effect(Some("1"), v2, |_| Some("io memory pids".into()))
                .is_err_and(|why| why.starts_with("no cpu controller"))
        );
    }

    /// The first two CPUs this process may run on.
    fn two_cpus() -> Option<String> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let list = status
            .lines()
            .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))?
            .trim()
            .to_string();
        let mut cpus = Vec::new();
        for part in list.split(',') {
            let (from, to) = part.split_once('-').unwrap_or((part, part));
            let (from, to): (u32, u32) = (from.parse().ok()?, to.parse().ok()?);
            cpus.extend(from..=to);
        }
        (cpus.len() >= 2).then(|| format!("{},{}", cpus[0], cpus[1]))
    }

    /// The (start, end) times of each recipe in a timed log, in nanoseconds.
    fn spans(log: &str) -> Vec<(u128, u128)> {
        let (mut starts, mut ends) = (Vec::new(), Vec::new());
        for line in log.lines() {
            let mut words = line.split_whitespace();
            let (Some(what), Some(time)) = (words.next(), words.next()) else {
                continue;
            };
            let time: u128 = time.parse().expect("a time");
            if what == "start" {
                starts.push(time);
            } else {
                ends.push(time);
            }
        }
        starts.sort_unstable();
        ends.sort_unstable();
        starts.into_iter().zip(ends).collect()
    }

    /// Two real builds at once on two CPUs, the main one (`main`) starting
    /// 200 ms after the other (`side`), which yields when `yields`: its make
    /// started as the step starts a yielding one, at the slot's niceness.
    /// The main build's wall time, from its first recipe's start to its
    /// last's end.
    fn pair(pool: &Jobserver, cpus: &str, yields: bool) -> u128 {
        let dirs: Vec<_> = (0..2)
            .map(|_| tempfile::tempdir().expect("a directory"))
            .collect();
        for dir in &dirs {
            std::fs::write(dir.path().join("Makefile"), BURN).expect("the Makefile");
        }
        std::thread::scope(|scope| {
            for (index, dir) in dirs.iter().enumerate() {
                scope.spawn(move || {
                    let side = index == 1;
                    let label = if side { "side" } else { "main" };
                    let slot = pool
                        .admit_as(&|| false, side && yields, label)
                        .expect("a slot");
                    if !side {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    let make = build_make(dir.path(), &slot, 13, label);
                    let mut command = Command::new("/usr/bin/taskset");
                    command
                        .arg("-c")
                        .arg(cpus)
                        .arg(make.get_program())
                        .args(make.get_args())
                        .current_dir(dir.path())
                        .env_clear()
                        .process_group(0);
                    for (key, value) in make.get_envs() {
                        if let Some(value) = value {
                            command.env(key, value);
                        }
                    }
                    let mut child = command.spawn().expect("make");
                    if let (Some(nice), Some(pid)) = (
                        slot.nice(),
                        rustix::process::Pid::from_raw(child.id() as i32),
                    ) {
                        rustix::process::setpriority_pgrp(Some(pid), nice).expect("renice");
                    }
                    let status = child.wait().expect("make");
                    assert!(status.success(), "{status}");
                });
            }
        });
        let main_log = std::fs::read_to_string(dirs[0].path().join("log")).unwrap_or_default();
        let side_log = std::fs::read_to_string(dirs[1].path().join("log")).unwrap_or_default();
        assert_eq!(spans(&side_log).len(), 24, "the side build did not finish");
        let main_ran = spans(&main_log);
        assert_eq!(main_ran.len(), 8, "the main build did not finish");
        main_ran.iter().map(|span| span.1).max().unwrap_or(0)
            - main_ran.iter().map(|span| span.0).min().unwrap_or(0)
    }

    /// The builds without slack have the first claim on the processor:
    /// beside a yielding build on the same two CPUs the main build runs much
    /// faster than beside an equal one, and the yielding build still
    /// finishes.
    #[test]
    fn a_yielding_build_leaves_the_processor_to_the_main_build() {
        let Some(pool) = fifo_pool(13) else { return };
        // Where niceness has no effect a yielding build shares the main
        // fifo instead (the test above), and there is nothing to measure.
        if let Err(why) = yield_size(10) {
            eprintln!("skipped: {why}");
            return;
        }
        let pool = pool.sized(Ok);
        let Some(cpus) = two_cpus() else {
            eprintln!("skipped: fewer than two CPUs");
            return;
        };
        let equal = pair(&pool, &cpus, false);
        let yielded = pair(&pool, &cpus, true);
        eprintln!(
            "measured: on CPUs {cpus}, main build {:.2} s beside an equal build, {:.2} s beside a yielding one",
            equal as f64 / 1e9,
            yielded as f64 / 1e9
        );
        assert!(
            yielded * 10 < equal * 7,
            "beside a yielding build the main build took {yielded} ns, beside an equal one {equal} ns"
        );
    }

    /// The niceness is Gaia's own and the step, at most 19: relative to a
    /// Gaia that runs nice itself, not the lowest priority.
    #[test]
    fn a_yielding_build_is_the_step_nicer_than_gaia() {
        assert_eq!(NICE_STEP, 7);
        assert_eq!(niceness_beside(0), 7);
        assert_eq!(niceness_beside(5), 12);
        assert_eq!(niceness_beside(-5), 2);
        assert_eq!(niceness_beside(15), 19);
        let own = rustix::process::getpriority_process(None).expect("own niceness");
        assert_eq!(yielding_niceness(), niceness_beside(own));
    }

    /// The iterations a shell burner on `cpu` counted in `window`, one at the
    /// test's niceness and one `step` nicer, as a yielding build's make is
    /// reniced: its process group, just after it starts.
    fn burn_pair(cpu: &str, step: i32, window: Duration) -> (u64, u64) {
        let own = rustix::process::getpriority_process(None).expect("own niceness");
        let burner = |nice: Option<i32>| {
            let child = Command::new("/usr/bin/taskset")
                .args(["-c", cpu, "bash", "-c"])
                .arg("i=0; trap 'echo $i; exit 0' TERM; while :; do i=$((i+1)); done")
                .stdout(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .expect("a burner");
            let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("a pid");
            if let Some(nice) = nice {
                rustix::process::setpriority_pgrp(Some(pid), nice).expect("renice");
            }
            (child, pid)
        };
        let main = burner(None);
        let side = burner(Some((own + step).min(19)));
        std::thread::sleep(window);
        let count = |(mut child, pid): (std::process::Child, rustix::process::Pid)| {
            rustix::process::kill_process_group(pid, rustix::process::Signal::TERM).expect("kill");
            let mut out = String::new();
            std::io::Read::read_to_string(child.stdout.as_mut().expect("stdout"), &mut out)
                .expect("read");
            child.wait().expect("wait");
            out.trim().parse::<u64>().expect("a count")
        };
        (count(main), count(side))
    }

    /// Two burners contending for one processor, one [`NICE_STEP`] nicer:
    /// the other gets about [`SPLIT`]. Prints the split at +5 and +7 too,
    /// the measurement the step was chosen by.
    #[test]
    fn a_yielding_build_gets_about_a_fifth_of_a_contended_processor() {
        if let Err(why) = yield_size(10) {
            eprintln!("skipped: {why}");
            return;
        }
        let Some(cpus) = two_cpus() else {
            eprintln!("skipped: fewer than two CPUs");
            return;
        };
        let cpu = cpus.split(',').next().expect("a cpu");
        let window = Duration::from_secs(3);
        for step in [5, 7] {
            let (main, side) = burn_pair(cpu, step, window);
            eprintln!(
                "measured: +{step}: {main} against {side} iterations, {:.1} : 1",
                main as f64 / side.max(1) as f64
            );
        }
        let (main, side) = burn_pair(cpu, NICE_STEP, window);
        let split = main as f64 / side.max(1) as f64;
        assert!(
            (3.5..8.0).contains(&split),
            "+{NICE_STEP}: {main} against {side} iterations, {split:.1} : 1"
        );
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
