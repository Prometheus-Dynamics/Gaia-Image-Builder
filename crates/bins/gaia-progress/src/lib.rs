//! Build progress for `gaia build`: the steps running now, how many of the
//! plan's steps are done, and, when the build fails, the failing step with
//! the tail of its stderr.
//!
//! Everything is read from what the step kit writes ([`gaia_step`]'s run
//! logs and step record), from a thread of its own, so the build's lanes are
//! never touched. A run log with a `# start` line and no `# end` line is a
//! tool still running; a step record line is a step done. A step whose body
//! runs no tool shows only when it is done.
//!
//! On a terminal the running steps are a block redrawn in place under the
//! lines of finished steps. Anywhere else (a pipe, CI) every change is a
//! plain line of its own, and nothing is redrawn.

use gaia_step::{StepRecord, now_ns, step_records_from};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// What to watch and how to print it.
#[derive(Debug, Clone)]
pub struct Options {
    /// The run log directory ([`gaia_step::logs_dir`]).
    pub logs: PathBuf,
    /// The build's process: other processes' logs are not this build's.
    pub pid: u32,
    /// When the build started; older logs are not this build's.
    pub since_ns: u128,
    /// How many steps the plan has.
    pub total: usize,
    /// Redraw a block in place, or print plain lines.
    pub terminal: bool,
    /// Columns to cut lines to on a terminal.
    pub width: usize,
}

impl Options {
    /// Options for stderr: a terminal when stderr is one and `TERM` is not
    /// `dumb`, `COLUMNS` wide (100 when unset).
    pub fn for_stderr(logs: PathBuf, pid: u32, since_ns: u128, total: usize) -> Self {
        use std::io::IsTerminal;
        let terminal =
            io::stderr().is_terminal() && !std::env::var("TERM").is_ok_and(|term| term == "dumb");
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|columns| columns.parse().ok())
            .filter(|&columns| columns >= 40)
            .unwrap_or(100);
        Self {
            logs,
            pid,
            since_ns,
            total,
            terminal,
            width,
        }
    }
}

/// A tool run that has started and not ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub step: String,
    pub tool: String,
    pub start_ns: u128,
    /// The last line the tool printed, without its `out|`/`err|` prefix.
    pub last: Option<String>,
}

/// Reads one build's progress from the run log directory.
#[derive(Debug)]
pub struct Reader {
    logs: PathBuf,
    pid: u32,
    since_ns: u128,
    offset: u64,
    ended: BTreeSet<PathBuf>,
    heads: BTreeMap<PathBuf, (String, String)>,
}

impl Reader {
    pub fn new(logs: PathBuf, pid: u32, since_ns: u128) -> Self {
        Self {
            logs,
            pid,
            since_ns,
            offset: 0,
            ended: BTreeSet::new(),
            heads: BTreeMap::new(),
        }
    }

    /// This build's step records written since the last call.
    pub fn done(&mut self) -> Vec<StepRecord> {
        let (records, offset) = step_records_from(&self.logs, self.offset);
        self.offset = offset;
        records
            .into_iter()
            .filter(|record| record.pid == self.pid && record.start_ns >= self.since_ns)
            .collect()
    }

    /// This build's tool runs that have not ended, oldest first.
    pub fn running(&mut self) -> Vec<(PathBuf, Running)> {
        let mut running = Vec::new();
        let Ok(steps) = fs::read_dir(&self.logs) else {
            return running;
        };
        for step in steps.flatten() {
            let Ok(files) = fs::read_dir(step.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                let Some(start_ns) = self.ours(&path) else {
                    continue;
                };
                if self.ended.contains(&path) {
                    continue;
                }
                let tail = tail(&path, 4096);
                if tail.lines().any(|line| line.starts_with("# end ")) {
                    self.ended.insert(path);
                    continue;
                }
                let Some((step, tool)) = self.head(&path) else {
                    continue;
                };
                let last = tail.lines().rev().find_map(|line| {
                    line.strip_prefix("out| ")
                        .or_else(|| line.strip_prefix("err| "))
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string)
                });
                running.push((
                    path,
                    Running {
                        step,
                        tool,
                        start_ns,
                        last,
                    },
                ));
            }
        }
        running.sort_by(|a, b| (a.1.start_ns, &a.0).cmp(&(b.1.start_ns, &b.0)));
        running
    }

    /// The newest run log of `step` from this build whose tool did not end
    /// with `exit 0`: it failed, or was stopped before it ended.
    pub fn failed_log(&self, step: &str) -> Option<PathBuf> {
        let dir = self.logs.join(step.replace(
            |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "_",
        ));
        fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|file| file.path())
            .filter_map(|path| Some((self.ours(&path)?, path)))
            .filter(|(_, path)| {
                let tail = tail(path, 4096);
                tail.lines()
                    .rev()
                    .find_map(|line| line.strip_prefix("# end "))
                    .is_none_or(|end| !end.ends_with(" exit 0"))
            })
            .max()
            .map(|(_, path)| path)
    }

    /// The start time of a log named `<start>-<key>-<pid>-<n>.log` written
    /// by this build.
    fn ours(&self, path: &Path) -> Option<u128> {
        let name = path.file_name()?.to_str()?.strip_suffix(".log")?;
        let mut words = name.split('-');
        let start: u128 = words.next()?.parse().ok()?;
        let _key = words.next()?;
        let pid: u32 = words.next()?.parse().ok()?;
        (pid == self.pid && start >= self.since_ns).then_some(start)
    }

    fn head(&mut self, path: &Path) -> Option<(String, String)> {
        if let Some(head) = self.heads.get(path) {
            return Some(head.clone());
        }
        let mut text = String::new();
        File::open(path)
            .ok()?
            .take(4096)
            .read_to_string(&mut text)
            .ok()?;
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::to_string)
        };
        let head = (field("# step ")?, field("# tool ").unwrap_or_default());
        self.heads.insert(path.to_path_buf(), head.clone());
        Some(head)
    }
}

/// The last `bytes` of `path`, from the first whole line in them.
fn tail(path: &Path, bytes: u64) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let from = length.saturating_sub(bytes);
    let mut buffer = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer).into_owned();
    if from == 0 {
        return text;
    }
    match text.split_once('\n') {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    }
}

/// What has been printed, and what to print next.
#[derive(Debug)]
pub struct Printer {
    options: Options,
    reader: Reader,
    done: usize,
    started: BTreeSet<PathBuf>,
    drawn: usize,
}

impl Printer {
    pub fn new(options: Options) -> Self {
        let reader = Reader::new(options.logs.clone(), options.pid, options.since_ns);
        Self {
            options,
            reader,
            done: 0,
            started: BTreeSet::new(),
            drawn: 0,
        }
    }

    /// Reads what changed and prints it to `out`.
    pub fn update(&mut self, out: &mut dyn Write) -> io::Result<()> {
        let records = self.reader.done();
        let running = self.reader.running();
        let mut text = String::new();
        if self.options.terminal && self.drawn > 0 {
            text.push_str(&format!("\x1b[{}A\r\x1b[J", self.drawn));
            self.drawn = 0;
        }
        if !self.options.terminal {
            for (path, run) in &running {
                if self.started.insert(path.clone()) {
                    text.push_str(&format!(
                        "start {}  {}\n",
                        run.step,
                        cut(
                            &run.tool,
                            self.options.width.saturating_sub(8 + run.step.len())
                        )
                    ));
                }
            }
        }
        for record in &records {
            self.done += 1;
            text.push_str(&format!(
                "{} {:5} {:>8} ms  {}\n",
                self.count(),
                if record.made { "made" } else { "hit" },
                record.end_ns.saturating_sub(record.start_ns) / 1_000_000,
                record.step
            ));
        }
        if self.options.terminal {
            let now = now_ns();
            let mut block = vec![format!(
                "{} {}s, {} running",
                self.count(),
                now.saturating_sub(self.options.since_ns) / 1_000_000_000,
                running.len()
            )];
            for (_, run) in &running {
                let line = format!(
                    "  {}  {}s  ",
                    run.step,
                    now.saturating_sub(run.start_ns) / 1_000_000_000
                );
                let rest = self.options.width.saturating_sub(line.chars().count());
                let last = run.last.as_deref().unwrap_or(&run.tool);
                block.push(format!("{line}{}", cut(last, rest)));
            }
            for line in &block {
                text.push_str(line);
                text.push('\n');
            }
            self.drawn = block.len();
        }
        out.write_all(text.as_bytes())?;
        out.flush()
    }

    /// Removes the redrawn block, so what follows starts on a clean line.
    pub fn clear(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.drawn > 0 {
            write!(out, "\x1b[{}A\r\x1b[J", self.drawn)?;
            self.drawn = 0;
        }
        out.flush()
    }

    /// `[done/total]`, the total never below what is done.
    fn count(&self) -> String {
        let total = self.options.total.max(self.done);
        let width = total.to_string().len();
        format!("[{:>width$}/{total}]", self.done)
    }
}

/// `text` cut to `width` characters, ending in `…` when cut.
fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A [`Printer`] on a thread of its own, printing to stderr until
/// [`Watch::finish`].
#[derive(Debug)]
pub struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Starts printing. A terminal redraws five times a second; plain lines
    /// are looked for twice a second.
    pub fn start(options: Options) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let every = if options.terminal {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(500)
        };
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("gaia-progress".into())
            .spawn(move || {
                let mut printer = Printer::new(options);
                let mut err = io::stderr();
                while !flag.load(Ordering::Acquire) {
                    let _ = printer.update(&mut err);
                    std::thread::park_timeout(every);
                }
                let _ = printer.update(&mut err);
                let _ = printer.clear(&mut err);
            })
            .ok();
        Self { stop, thread }
    }

    /// Prints what is left and stops.
    pub fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// How many stderr lines a failure shows.
pub const STDERR_LINES: usize = 20;

/// A failed step for people: where it failed, why, and the last
/// [`STDERR_LINES`] lines its tool printed on stderr, read from `log` (see
/// [`Reader::failed_log`]). Without a log, or with no stderr in it, the
/// reason is all there is.
pub fn failure(at: &str, reason: &str, log: Option<&Path>) -> String {
    let mut out = format!("failed: {at}\n");
    for line in reason.lines() {
        out.push_str(&format!("  {line}\n"));
    }
    let Some(log) = log else {
        return out;
    };
    out.push_str(&format!("  log: {}\n", log.display()));
    let stderr = stderr_tail(log);
    if !stderr.is_empty() {
        out.push_str(&format!("  stderr, last {} lines:\n", stderr.len()));
        for line in stderr {
            out.push_str(&format!("    {line}\n"));
        }
    }
    out
}

/// The last [`STDERR_LINES`] `err|` lines of a run log.
fn stderr_tail(log: &Path) -> Vec<String> {
    let text = tail(log, 64 * 1024);
    let lines: Vec<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("err| "))
        .map(str::to_string)
        .collect();
    let skip = lines.len().saturating_sub(STDERR_LINES);
    lines.into_iter().skip(skip).collect()
}

/// The step a step kit fault reason names: the reason is `<step>: <what>`.
pub fn reason_step(reason: &str) -> Option<&str> {
    reason
        .split_once(": ")
        .map(|(step, _)| step)
        .filter(|step| !step.is_empty() && !step.contains(char::is_whitespace))
}
