//! `gaia status [run]`: what the `gaia run`s on this system are doing right
//! now, read from the registry (see `run_registry`) and the live status files
//! (see `live_status`).
//!
//! With no argument it lists every registered run, live ones first and runs
//! that ended in the last day after them. A number from that list, a build
//! name or id, or a build config path selects one run. A build config that no
//! registered run uses is resolved and its build dir is read directly, as
//! before. `--follow` refreshes every second until the run ends (for the list:
//! until no run is live). The TUI monitor shows the same lines.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_plan::format_duration_short;

use super::CommandOutcome;
use super::live_status::{
    BuildProgressStatus, LastRun, LiveRun, LiveStatus, live_run, read_last, unix_now,
};
use super::progress::{build_progress_summary, format_progress_elapsed};
use super::run_registry::{ListedRun, RegisteredRun, inspect_run, list_runs, select_run};

/// Log lines `gaia status` prints under the summary.
const RECENT_LOG_LINES: usize = 10;

/// `build` is the build config or run named on the command line, `None` when
/// there is none.
#[cfg(unix)]
pub fn status_command(
    build: Option<&str>,
    options: &ResolveOptions,
    follow: bool,
) -> CommandOutcome {
    let Some(selector) = build else {
        return status_listing(follow);
    };
    match select_run(&list_runs(), selector) {
        Ok(Some(run)) => status_of_run(run.run.clone(), follow),
        Ok(None) => status_of_build(selector, options, follow),
        Err(message) => CommandOutcome::Failed { message },
    }
}

#[cfg(not(unix))]
pub fn status_command(
    _build: Option<&str>,
    _options: &ResolveOptions,
    _follow: bool,
) -> CommandOutcome {
    CommandOutcome::Failed {
        message: "gaia status needs a Unix system".into(),
    }
}

/// `gaia status` with no run named: every registered run, one line each.
#[cfg(unix)]
fn status_listing(follow: bool) -> CommandOutcome {
    if !follow {
        return CommandOutcome::Text {
            text: listing_text(&list_runs(), unix_now()),
        };
    }
    follow_frames(|| {
        let runs = list_runs();
        let live = runs.iter().any(ListedRun::is_live);
        (listing_text(&runs, unix_now()), live)
    })
}

/// `gaia status <run>`: the status of one registered run.
#[cfg(unix)]
fn status_of_run(run: RegisteredRun, follow: bool) -> CommandOutcome {
    if !follow {
        let (text, _) = run_status_text(&inspect_run(run), unix_now());
        return CommandOutcome::Text { text };
    }
    follow_frames(|| {
        // The registry entry is re-read each frame, so a run that ends is
        // shown with its outcome.
        run_status_text(&inspect_run(run.clone()), unix_now())
    })
}

/// `gaia status <build config>` for a build no registered run uses: its build
/// dir is read directly.
#[cfg(unix)]
fn status_of_build(build: &str, options: &ResolveOptions, follow: bool) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let name = spec.identity.display_name.clone();
    if !follow {
        let (text, _) = build_dir_status_text(&name, &build_dir);
        return CommandOutcome::Text { text };
    }
    follow_frames(|| build_dir_status_text(&name, &build_dir))
}

/// Prints each new frame that `frame` produces, until a frame says no run is
/// live. On a terminal the screen is cleared between frames.
#[cfg(unix)]
fn follow_frames(mut frame: impl FnMut() -> (String, bool)) -> CommandOutcome {
    let terminal = io::stdout().is_terminal();
    let mut previous: Option<String> = None;
    loop {
        let (text, live) = frame();
        if previous.as_deref() != Some(text.as_str()) {
            let mut out = io::stdout().lock();
            // Clear the screen between frames on a terminal; elsewhere print
            // each new frame as its own block.
            let written = if terminal {
                writeln!(out, "\x1b[2J\x1b[H{text}")
            } else {
                writeln!(out, "{text}\n")
            };
            if written.and_then(|()| out.flush()).is_err() {
                return CommandOutcome::Failed {
                    message: "gaia status: stdout closed".into(),
                };
            }
            previous = Some(text);
        }
        if !live {
            // The final frame already said how the run ended.
            return CommandOutcome::Text {
                text: String::new(),
            };
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// The status text of a registered run now, and whether it is live.
pub(crate) fn run_status_text(run: &ListedRun, now: u64) -> (String, bool) {
    status_text(
        &run.run.display_name,
        run.live.as_ref(),
        run.ended.as_ref(),
        now,
    )
}

/// The status text for `build_dir` now, and whether its run is live.
#[cfg(unix)]
fn build_dir_status_text(display_name: &str, build_dir: &Path) -> (String, bool) {
    let live = live_run(build_dir);
    let last = read_last(build_dir);
    status_text(display_name, live.as_ref(), last.as_ref(), unix_now())
}

/// The `gaia status` text: the live run, or that none runs and how the last
/// one ended. The flag says whether a run is live.
pub(crate) fn status_text(
    display_name: &str,
    live: Option<&LiveRun>,
    last: Option<&LastRun>,
    now: u64,
) -> (String, bool) {
    if let Some(run) = live {
        let mut lines = live_lines(run, now);
        if let Some(status) = &run.status {
            lines.push("recent output:".into());
            let logs = log_lines(status);
            let skip = logs.len().saturating_sub(RECENT_LOG_LINES);
            lines.extend(logs.into_iter().skip(skip));
        }
        return (lines.join("\n"), true);
    }
    let mut lines = vec![format!("no run of '{display_name}' is running")];
    if let Some(last) = last {
        lines.extend(last_run_lines(last, now));
    }
    (lines.join("\n"), false)
}

/// The listing of registered runs, one numbered line each (the numbers are
/// what `gaia status <n>` takes).
pub(crate) fn listing_text(runs: &[ListedRun], now: u64) -> String {
    if runs.is_empty() {
        return "no gaia run is running, and none ended in the last 24 hours".into();
    }
    numbered_lines(runs, now, false)
}

/// The numbered listing lines; `only_live` leaves out the ended runs but keeps
/// their numbers.
pub(crate) fn numbered_lines(runs: &[ListedRun], now: u64, only_live: bool) -> String {
    runs.iter()
        .enumerate()
        .filter(|(_, run)| !only_live || run.is_live())
        .map(|(index, run)| format!("{:>2}  {}", index + 1, run_summary(run, now)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One registered run on one line: name, pid, elapsed time, paused, operations
/// finished out of the total, Buildroot packages with the estimate, and the
/// current operation. An ended run gives its outcome and how long ago it ended.
pub(crate) fn run_summary(run: &ListedRun, now: u64) -> String {
    let name = run.run.display_name.as_str();
    let pid = run.run.pid;
    if let Some(live) = &run.live {
        let state = if live.paused() { "PAUSED" } else { "running" };
        let elapsed = short_elapsed(now.saturating_sub(run.run.started_at));
        let mut line = format!("{name}  pid {pid}  elapsed {elapsed}  {state}");
        if let Some(status) = &live.status {
            line.push_str(&format!("  ops {}/{}", status.ops_done, status.ops_total));
            if let Some(progress) = status.running.iter().find_map(|op| op.build.as_ref()) {
                line.push_str(&format!("  packages {}/{}", progress.done, progress.total));
                if let Some(eta) = progress.eta_secs {
                    let eta = format_duration_short(Duration::from_secs(eta));
                    line.push_str(&format!(" eta {eta}"));
                }
            }
            if let Some(op) = status.running.first() {
                line.push_str(&format!("  current {}", op.id));
            }
        }
        return line;
    }
    match &run.ended {
        Some(last) => {
            let ago = format_duration_short(Duration::from_secs(now.saturating_sub(last.ended_at)));
            format!(
                "{name}  pid {pid}  ended ({}) {ago} ago  ops {}/{}",
                last.outcome, last.status.ops_done, last.status.ops_total
            )
        }
        None => format!("{name}  pid {pid}  state unknown"),
    }
}

/// The summary of a live run: build, elapsed time, paused, operations,
/// Buildroot packages and the running operations with their last output.
pub(crate) fn live_lines(run: &LiveRun, now: u64) -> Vec<String> {
    let Some(status) = &run.status else {
        return vec![format!(
            "gaia run {} is running; it has not written a status yet",
            run.pid
        )];
    };
    let elapsed =
        format_progress_elapsed(Duration::from_secs(now.saturating_sub(status.started_at)));
    let state = if run.paused() { "PAUSED" } else { "running" };
    let counts = &status.finished;
    let mut lines = vec![
        format!(
            "build: {}  pid {}  elapsed {}  {}",
            status.display_name, status.pid, elapsed, state
        ),
        format!(
            "ops: {}/{}  done {} reused {} failed {} cancelled {} skipped {}  running {}",
            status.ops_done,
            status.ops_total,
            counts.done,
            counts.reused,
            counts.failed,
            counts.cancelled,
            counts.skipped,
            status.running.len()
        ),
    ];
    if let Some(progress) = status.running.iter().find_map(|op| op.build.as_ref()) {
        lines.push(format!(
            "packages: {}",
            build_progress_summary(&process_progress(progress))
        ));
    }
    if !status.running.is_empty() {
        lines.push("running:".into());
    }
    for op in &status.running {
        let elapsed =
            format_progress_elapsed(Duration::from_secs(now.saturating_sub(op.started_at)));
        let mut line = format!("  {}  {}", op.id, elapsed);
        if let Some(last) = &op.last_log {
            line.push_str(&format!("  last: {last}"));
        }
        lines.push(line);
    }
    lines
}

/// How a run that is no longer live ended.
pub(crate) fn last_run_lines(last: &LastRun, now: u64) -> Vec<String> {
    let status = &last.status;
    let ago = format_duration_short(Duration::from_secs(now.saturating_sub(last.ended_at)));
    let counts = &status.finished;
    let mut lines = vec![
        format!(
            "last run: {}  pid {}  ended {} ago",
            last.outcome, status.pid, ago
        ),
        format!(
            "ops: {}/{}  done {} reused {} failed {} cancelled {} skipped {}",
            status.ops_done,
            status.ops_total,
            counts.done,
            counts.reused,
            counts.failed,
            counts.cancelled,
            counts.skipped
        ),
    ];
    if !status.failed.is_empty() {
        lines.push(format!("failed: {}", status.failed.join(", ")));
    }
    for error in &last.errors {
        lines.push(format!(
            "error {} ({}): {}",
            error.operation_id,
            error.code,
            error.message.lines().collect::<Vec<_>>().join(" / ")
        ));
    }
    lines
}

/// Every retained output line, as `operation  line`.
pub(crate) fn log_lines(status: &LiveStatus) -> Vec<String> {
    status
        .logs
        .iter()
        .map(|log| format!("{}  {}", log.op, log.line))
        .collect()
}

#[cfg_attr(not(feature = "tui"), allow(dead_code))]
/// The picker's note for a build with a live run:
/// `● running 12m, 83% packages`.
pub(crate) fn live_badge(run: &LiveRun, now: u64) -> String {
    let Some(status) = &run.status else {
        return "● running".into();
    };
    let state = if run.paused() { "paused" } else { "running" };
    let elapsed = short_elapsed(now.saturating_sub(status.started_at));
    let detail = match status.running.iter().find_map(|op| op.build.as_ref()) {
        Some(progress) => {
            let percent = progress
                .done
                .saturating_mul(100)
                .checked_div(progress.total)
                .unwrap_or(100);
            format!("{percent}% packages")
        }
        None => format!("ops {}/{}", status.ops_done, status.ops_total),
    };
    format!("● {state} {elapsed}, {detail}")
}

/// `45s`, `12m`, `1h05m`.
pub(crate) fn short_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn process_progress(progress: &BuildProgressStatus) -> gaia_process::BuildProgress {
    gaia_process::BuildProgress {
        done: progress.done,
        total: progress.total,
        active: progress.active.clone(),
        eta: progress.eta_secs.map(Duration::from_secs),
    }
}

#[cfg(test)]
mod tests {
    use super::super::live_status::fixture_status;
    use super::super::run_registry::fixture_registered_run;
    use super::*;

    fn live_fixture() -> LiveRun {
        LiveRun {
            pid: 4242,
            status: Some(fixture_status()),
            stopped: false,
        }
    }

    fn ended_fixture() -> LastRun {
        let mut status = fixture_status();
        status.running.clear();
        status.failed = vec!["artifact:kernel".into()];
        status.finished.failed = 1;
        LastRun {
            outcome: "failed".into(),
            ended_at: 1_500,
            status,
            errors: Vec::new(),
        }
    }

    #[test]
    fn live_status_lists_build_ops_packages_and_running_operations() {
        // Started at 1000, now 1100: 100 seconds in.
        let (text, live) = status_text("Demo build", Some(&live_fixture()), None, 1_100);
        assert!(live);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "build: Demo build  pid 4242  elapsed 01:40  running"
        );
        assert_eq!(
            lines[1],
            "ops: 3/10  done 2 reused 1 failed 0 cancelled 0 skipped 0  running 1"
        );
        assert!(lines[2].starts_with("packages: [") && lines[2].contains("120/145 packages"));
        assert!(lines[2].contains("building=linux,mesa3d"));
        assert_eq!(lines[3], "running:");
        assert_eq!(lines[4], "  image:buildroot  00:50  last: building linux");
        assert_eq!(lines[5], "recent output:");
        assert_eq!(lines[6], "artifact:cli  cargo build finished");
    }

    #[test]
    fn paused_live_run_says_so() {
        let mut run = live_fixture();
        run.stopped = true;
        let (text, _) = status_text("Demo build", Some(&run), None, 1_100);
        assert!(text.contains("elapsed 01:40  PAUSED"));
    }

    #[test]
    fn ended_run_reports_its_outcome_when_nothing_is_live() {
        let last = ended_fixture();
        let (text, live) = status_text("Demo build", None, Some(&last), 1_560);
        assert!(!live);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "no run of 'Demo build' is running");
        assert_eq!(lines[1], "last run: failed  pid 4242  ended 1m00s ago");
        assert_eq!(
            lines[2],
            "ops: 3/10  done 2 reused 1 failed 1 cancelled 0 skipped 0"
        );
        assert_eq!(lines[3], "failed: artifact:kernel");
    }

    #[test]
    fn nothing_recorded_says_no_run_is_running() {
        let (text, live) = status_text("Demo build", None, None, 0);
        assert!(!live);
        assert_eq!(text, "no run of 'Demo build' is running");
    }

    #[test]
    fn live_run_without_status_yet_says_so() {
        let run = LiveRun {
            pid: 7,
            status: None,
            stopped: false,
        };
        let (text, live) = status_text("Demo build", Some(&run), None, 0);
        assert!(live);
        assert_eq!(
            text,
            "gaia run 7 is running; it has not written a status yet"
        );
    }

    #[test]
    fn badge_shows_package_percentage_and_elapsed_time() {
        let run = live_fixture();
        // 12 minutes after the start, 120 of 145 packages: 82%.
        assert_eq!(
            live_badge(&run, 1_000 + 12 * 60),
            "● running 12m, 82% packages"
        );
        let mut no_packages = fixture_status();
        no_packages.running.clear();
        let run = LiveRun {
            pid: 1,
            status: Some(no_packages),
            stopped: true,
        };
        assert_eq!(live_badge(&run, 1_000 + 90), "● paused 1m, ops 3/10");
    }

    #[test]
    fn short_elapsed_picks_a_compact_unit() {
        assert_eq!(short_elapsed(45), "45s");
        assert_eq!(short_elapsed(12 * 60 + 3), "12m");
        assert_eq!(short_elapsed(3_600 + 5 * 60), "1h05m");
    }

    fn live_listed(name: &str, pid: u32) -> ListedRun {
        let mut status = fixture_status();
        status.pid = pid;
        status.display_name = name.into();
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: Some(LiveRun {
                pid,
                status: Some(status),
                stopped: false,
            }),
            ended: None,
        }
    }

    fn ended_listed(name: &str, pid: u32) -> ListedRun {
        let mut last = ended_fixture();
        last.status.pid = pid;
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: None,
            ended: Some(last),
        }
    }

    #[test]
    fn listing_has_one_numbered_line_per_run() {
        let runs = vec![live_listed("Cm5 image", 4242), ended_listed("Pi zero", 99)];
        // Started at 1000, now 1560: 560 seconds in; the ended run ended at 1500.
        let text = listing_text(&runs, 1_560);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(
            lines[0],
            " 1  Cm5 image  pid 4242  elapsed 9m  running  ops 3/10  packages 120/145 eta 10m00s  current image:buildroot"
        );
        assert_eq!(
            lines[1],
            " 2  Pi zero  pid 99  ended (failed) 1m00s ago  ops 3/10"
        );
    }

    #[test]
    fn listing_marks_paused_runs_and_packages_without_eta() {
        let mut run = live_listed("Demo", 4242);
        run.live.as_mut().expect("live").stopped = true;
        if let Some(status) = run.live.as_mut().and_then(|live| live.status.as_mut()) {
            status.running[0].build = None;
        }
        let line = run_summary(&run, 1_100);
        assert_eq!(
            line,
            "Demo  pid 4242  elapsed 1m  PAUSED  ops 3/10  current image:buildroot"
        );
    }

    #[test]
    fn listing_without_runs_says_so() {
        assert_eq!(
            listing_text(&[], 0),
            "no gaia run is running, and none ended in the last 24 hours"
        );
    }

    #[test]
    fn only_live_lines_keep_the_listing_numbers() {
        let runs = vec![ended_listed("Old", 1), live_listed("New", 4242)];
        let text = numbered_lines(&runs, 1_100, true);
        assert!(text.starts_with(" 2  New"), "{text}");
        assert!(!text.contains("Old"));
    }

    #[test]
    fn ended_run_lists_the_error_message_of_each_failed_operation() {
        use crate::commands::live_status::RunError;

        let mut last = ended_fixture();
        last.errors = vec![RunError {
            operation_id: "image:assembly".into(),
            code: "assembly_execution_failed".into(),
            message: "assembly source 'out/flash-id' does not exist\nsecond line".into(),
        }];
        let lines = last_run_lines(&last, 1_600);
        assert_eq!(
            lines.last().map(String::as_str),
            Some(
                "error image:assembly (assembly_execution_failed): assembly source 'out/flash-id' does not exist / second line"
            )
        );
    }
}
