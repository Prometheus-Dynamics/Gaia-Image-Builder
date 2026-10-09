//! The live progress lines of `gaia run`: completed operations, the running
//! ones and their latest output, and for a Buildroot `make` its packages.

use gaia_exec::ExecutionEvent;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

pub(super) fn console_progress_disabled() -> bool {
    std::env::var("GAIA_RUN_PROGRESS")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "quiet" | "none"
            )
        })
        .unwrap_or(false)
}

pub(super) struct ConsoleProgress {
    total: usize,
    rx: mpsc::Receiver<ExecutionEvent>,
    started_at: Instant,
    running: BTreeMap<String, Instant>,
    terminal: BTreeSet<String>,
    last_log: BTreeMap<String, String>,
    /// Inner progress (Buildroot packages) of running operations.
    build_progress: BTreeMap<String, gaia_process::BuildProgress>,
    last_status_at: Instant,
}

impl ConsoleProgress {
    pub(super) fn new(total: usize, rx: mpsc::Receiver<ExecutionEvent>) -> Self {
        Self {
            total,
            rx,
            started_at: Instant::now(),
            running: BTreeMap::new(),
            terminal: BTreeSet::new(),
            last_log: BTreeMap::new(),
            build_progress: BTreeMap::new(),
            last_status_at: Instant::now() - Duration::from_secs(30),
        }
    }

    pub(super) fn run(mut self) {
        self.print_line("run", "starting execution plan");
        loop {
            match self.rx.recv_timeout(Duration::from_secs(10)) {
                Ok(event) => self.handle_event(event),
                Err(RecvTimeoutError::Timeout) => self.print_heartbeat(),
                Err(RecvTimeoutError::Disconnected) => {
                    if !self.running.is_empty() {
                        self.print_heartbeat();
                    }
                    break;
                }
            }
        }
    }

    fn handle_event(&mut self, event: ExecutionEvent) {
        match event {
            ExecutionEvent::Started { operation_id } => {
                let id = operation_id.as_str().to_string();
                self.running.insert(id.clone(), Instant::now());
                self.print_line(&id, "started");
            }
            ExecutionEvent::Log {
                operation_id,
                message,
            } => {
                let id = operation_id.as_str().to_string();
                if let Some(progress) = gaia_process::parse_build_progress(&message) {
                    if self.running.contains_key(&id) {
                        self.build_progress.insert(id, progress);
                    }
                } else if self.running.contains_key(&id) {
                    self.last_log.insert(id, compact_log_line(&message));
                    if self.last_status_at.elapsed() >= Duration::from_secs(12) {
                        self.print_heartbeat();
                    }
                }
            }
            ExecutionEvent::Succeeded { operation_id } => {
                self.finish_operation(operation_id.as_str(), "done");
            }
            ExecutionEvent::Reused { operation_id } => {
                self.finish_operation(operation_id.as_str(), "reused");
            }
            ExecutionEvent::Cancelled { operation_id } => {
                self.finish_operation(operation_id.as_str(), "cancelled");
            }
            ExecutionEvent::Failed {
                operation_id,
                message,
            } => {
                let message = compact_log_line(&message);
                self.finish_operation(operation_id.as_str(), &format!("failed: {message}"));
            }
            ExecutionEvent::Skipped {
                operation_id,
                reason,
            } => {
                self.finish_operation(operation_id.as_str(), &reason);
            }
        }
    }

    fn finish_operation(&mut self, operation_id: &str, status: &str) {
        let status = match self.running.get(operation_id) {
            Some(started_at) if status == "done" => format!(
                "{status} in {}",
                gaia_plan::format_duration_short(started_at.elapsed())
            ),
            _ => status.to_string(),
        };
        let status = status.as_str();
        self.running.remove(operation_id);
        self.last_log.remove(operation_id);
        self.build_progress.remove(operation_id);
        self.terminal.insert(operation_id.to_string());
        self.print_line(operation_id, status);
    }

    fn print_heartbeat(&mut self) {
        let Some((operation_id, started_at)) = self
            .running
            .iter()
            .max_by_key(|(_, started_at)| started_at.elapsed())
        else {
            return;
        };
        let elapsed = format_progress_elapsed(started_at.elapsed());
        let detail = self
            .last_log
            .get(operation_id)
            .map(|line| format!("running {elapsed}; last: {line}"))
            .unwrap_or_else(|| format!("running {elapsed}"));
        let operation_id = operation_id.clone();
        self.print_line(&operation_id, &detail);
    }

    fn print_line(&mut self, operation_id: &str, detail: &str) {
        self.last_status_at = Instant::now();
        let done = self.terminal.len();
        let percent = done
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(100);
        // A Buildroot make reports its packages: show those first, the
        // operations second, so a long make does not look stalled.
        if let Some(progress) = self
            .running
            .keys()
            .find_map(|id| self.build_progress.get(id))
        {
            eprintln!(
                "run {} | ops {}/{} running={} elapsed={} op={} {}",
                build_progress_summary(progress),
                done,
                self.total,
                self.running.len(),
                format_progress_elapsed(self.started_at.elapsed()),
                operation_id,
                detail
            );
            return;
        }
        eprintln!(
            "run {} {:>3}% {}/{} running={} elapsed={} op={} {}",
            progress_bar(done, self.total),
            percent,
            done,
            self.total,
            self.running.len(),
            format_progress_elapsed(self.started_at.elapsed()),
            operation_id,
            detail
        );
    }
}

/// `[####----]  41% 120/290 packages eta ~35m00s building=linux,mesa3d +2`.
fn build_progress_summary(progress: &gaia_process::BuildProgress) -> String {
    const SHOWN: usize = 3;
    let percent = progress
        .done
        .saturating_mul(100)
        .checked_div(progress.total)
        .unwrap_or(100);
    let mut summary = format!(
        "{} {:>3}% {}/{} packages",
        progress_bar(progress.done, progress.total),
        percent,
        progress.done,
        progress.total
    );
    if let Some(eta) = progress.eta {
        summary.push_str(&format!(" eta ~{}", gaia_plan::format_duration_short(eta)));
    }
    if !progress.active.is_empty() {
        summary.push_str(" building=");
        summary.push_str(
            &progress
                .active
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
        if progress.active.len() > SHOWN {
            summary.push_str(&format!(" +{}", progress.active.len() - SHOWN));
        }
    }
    summary
}

fn progress_bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 16;
    let filled = done
        .saturating_mul(WIDTH)
        .checked_div(total)
        .unwrap_or(WIDTH);
    format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(WIDTH.saturating_sub(filled))
    )
}

fn format_progress_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

fn compact_log_line(line: &str) -> String {
    let mut cleaned = strip_ansi(line)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    const MAX_LEN: usize = 140;
    if cleaned.len() > MAX_LEN {
        let truncate_at = cleaned
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MAX_LEN.saturating_sub(3))
            .last()
            .unwrap_or(0);
        cleaned.truncate(truncate_at);
        cleaned.push_str("...");
    }
    cleaned
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_progress_leads_with_packages_eta_and_active_ones() {
        let progress = gaia_process::BuildProgress {
            done: 120,
            total: 290,
            active: vec![
                "linux".into(),
                "mesa3d".into(),
                "openjdk".into(),
                "qt6base".into(),
            ],
            eta: Some(Duration::from_secs(35 * 60)),
        };
        assert_eq!(
            build_progress_summary(&progress),
            "[######----------]  41% 120/290 packages eta ~35m00s \
             building=linux,mesa3d,openjdk +1"
        );
        let starting = gaia_process::BuildProgress {
            done: 0,
            total: 10,
            active: Vec::new(),
            eta: None,
        };
        assert_eq!(
            build_progress_summary(&starting),
            "[----------------]   0% 0/10 packages"
        );
    }

    #[test]
    fn progress_bar_renders_completed_fraction() {
        assert_eq!(progress_bar(0, 4), "[----------------]");
        assert_eq!(progress_bar(2, 4), "[########--------]");
        assert_eq!(progress_bar(4, 4), "[################]");
    }

    #[test]
    fn compact_log_line_strips_ansi_and_bounds_output() {
        let line = format!("\u{1b}[31mERROR\u{1b}[0m {}", "x ".repeat(200));
        let compact = compact_log_line(&line);

        assert!(compact.starts_with("ERROR"));
        assert!(compact.len() <= 140);
        assert!(!compact.contains('\u{1b}'));
    }
}
