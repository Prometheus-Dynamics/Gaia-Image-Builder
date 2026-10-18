//! CI output of `gaia run` (`--ci`, or `CI` set): no live progress lines;
//! each operation's output is kept until it finishes and then printed as one
//! GitHub Actions log group (`::group::` ... `::endgroup::`), so parallel
//! operations never interleave and the log folds per operation.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static CI_OUTPUT: AtomicBool = AtomicBool::new(false);

/// Lines kept per operation; earlier ones are dropped and counted.
const KEPT_LINES: usize = 5_000;

/// Turns CI output on or off for the runs this process starts.
pub(crate) fn set_ci_output(enabled: bool) {
    CI_OUTPUT.store(enabled, Ordering::Relaxed);
}

pub(super) fn ci_output() -> bool {
    CI_OUTPUT.load(Ordering::Relaxed)
}

#[derive(Default)]
struct OperationLog {
    started_at: Option<Instant>,
    lines: VecDeque<String>,
    dropped: usize,
}

/// The per-operation logs of a run in CI mode.
#[derive(Default)]
pub(super) struct CiGroups {
    operations: BTreeMap<String, OperationLog>,
}

impl CiGroups {
    pub(super) fn started(&mut self, operation_id: &str) {
        self.operations
            .entry(operation_id.to_string())
            .or_default()
            .started_at = Some(Instant::now());
    }

    pub(super) fn log(&mut self, operation_id: &str, line: &str) {
        let log = self.operations.entry(operation_id.to_string()).or_default();
        if log.lines.len() >= KEPT_LINES {
            log.lines.pop_front();
            log.dropped += 1;
        }
        log.lines.push_back(line.trim_end().to_string());
    }

    /// The lines to print for an operation that finished with `status`
    /// (`done`, `reused`, `failed: ...`): its group, or a single line when it
    /// wrote nothing. A failure is repeated after the group as an `::error::`
    /// annotation.
    pub(super) fn finished(&mut self, operation_id: &str, status: &str) -> Vec<String> {
        let log = self.operations.remove(operation_id).unwrap_or_default();
        let elapsed = log
            .started_at
            .map(|started| format!(" in {}", short(started.elapsed())))
            .unwrap_or_default();
        let title = format!("{operation_id}: {status}{elapsed}");
        let mut lines = Vec::new();
        if log.lines.is_empty() {
            lines.push(title);
        } else {
            lines.push(format!("::group::{title}"));
            if log.dropped > 0 {
                lines.push(format!("({} earlier line(s) not kept)", log.dropped));
            }
            // Output that looks like a workflow command must not run as one.
            let guard = log
                .lines
                .iter()
                .any(|line| line.trim_start().starts_with("::"))
                .then(stop_token);
            if let Some(token) = &guard {
                lines.push(format!("::stop-commands::{token}"));
            }
            lines.extend(log.lines);
            if let Some(token) = guard {
                lines.push(format!("::{token}::"));
            }
            lines.push("::endgroup::".into());
        }
        if let Some(message) = status.strip_prefix("failed: ") {
            lines.push(format!(
                "::error title=gaia {operation_id} failed::{}",
                escape_data(message)
            ));
        }
        lines
    }
}

fn short(duration: Duration) -> String {
    gaia_plan::format_duration_short(duration)
}

/// A token for `::stop-commands::` that build output cannot guess.
fn stop_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("gaia-{:x}-{nanos:x}", std::process::id())
}

/// Escapes a workflow command's data (`%`, CR and LF).
fn escape_data(message: &str) -> String {
    message
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_operations_print_one_group_with_their_output() {
        let mut groups = CiGroups::default();
        groups.started("artifact:app");
        groups.started("artifact:lib");
        groups.log("artifact:app", "compiling app");
        groups.log("artifact:lib", "compiling lib");
        groups.log("artifact:app", "finished app\n");
        let lines = groups.finished("artifact:app", "done");
        assert!(lines[0].starts_with("::group::artifact:app: done in "));
        assert_eq!(
            lines[1..],
            ["compiling app", "finished app", "::endgroup::"]
        );
        // Quiet operations get one line, no group.
        assert_eq!(groups.finished("source:a", "reused"), ["source:a: reused"]);
    }

    #[test]
    fn failures_are_annotated_and_commands_in_output_are_disabled() {
        let mut groups = CiGroups::default();
        groups.log("image:build", "::add-mask::nope");
        let lines = groups.finished("image:build", "failed: make exited 2\nsee log");
        let token = lines[1]
            .strip_prefix("::stop-commands::")
            .expect("stop-commands");
        assert_eq!(lines[2], "::add-mask::nope");
        assert_eq!(lines[3], format!("::{token}::"));
        assert_eq!(lines[4], "::endgroup::");
        assert_eq!(
            lines[5],
            "::error title=gaia image:build failed::make exited 2%0Asee log"
        );
    }

    #[test]
    fn long_output_keeps_the_last_lines() {
        let mut groups = CiGroups::default();
        for index in 0..KEPT_LINES + 2 {
            groups.log("image:build", &format!("line {index}"));
        }
        let lines = groups.finished("image:build", "done");
        assert_eq!(lines[1], "(2 earlier line(s) not kept)");
        assert_eq!(lines[2], "line 2");
    }
}
