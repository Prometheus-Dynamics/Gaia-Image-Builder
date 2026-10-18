use std::collections::{HashMap, VecDeque};

use super::*;

/// Upper bound on retained log lines per operation. Buildroot can emit hundreds
/// of thousands of lines; the TUI only needs a readable tail.
const MAX_LOG_LINES_PER_OPERATION: usize = 5_000;
/// Upper bound on the log text kept across all operations, in bytes. Past it
/// the oldest lines go: those of finished operations first, in the order they
/// finished, then those of the largest operation.
pub(crate) const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
/// Upper bound on the lifecycle events kept; the oldest go first.
const MAX_LIFECYCLE_EVENTS: usize = 10_000;

/// The events of the run this TUI started: the lifecycle events, in order,
/// and each operation's output, kept per operation so the operation popup
/// can show its tail. Both are bounded: see the constants above.
pub(crate) struct EventLog {
    /// Lifecycle events (everything except logs), in arrival order, the most
    /// recent [`MAX_LIFECYCLE_EVENTS`] of them.
    lifecycle: VecDeque<ExecutionEvent>,
    /// The first operation that failed. Kept apart from `lifecycle`, so the
    /// failure is still known once its event has been dropped.
    first_failed: Option<String>,
    logs: HashMap<String, OperationLog>,
    /// Bytes of log text kept across all operations.
    log_bytes: usize,
    /// Operations that ended, in the order they ended. Their output is the
    /// first to go when the log budget is spent.
    finished: VecDeque<String>,
    /// The log byte budget; [`MAX_LOG_BYTES`] outside tests.
    budget: usize,
}

#[derive(Default)]
pub(crate) struct OperationLog {
    pub(crate) lines: VecDeque<String>,
    pub(crate) dropped: usize,
    /// Bytes of the lines kept.
    bytes: usize,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::with_log_budget(MAX_LOG_BYTES)
    }
}

impl EventLog {
    pub(crate) fn from_events<'e>(events: impl IntoIterator<Item = &'e ExecutionEvent>) -> Self {
        let mut log = Self::default();
        for event in events {
            log.push(event.clone());
        }
        log
    }

    /// An empty log that keeps at most `budget` bytes of output.
    pub(crate) fn with_log_budget(budget: usize) -> Self {
        Self {
            lifecycle: VecDeque::new(),
            first_failed: None,
            logs: HashMap::new(),
            log_bytes: 0,
            finished: VecDeque::new(),
            budget,
        }
    }

    pub(crate) fn push(&mut self, event: ExecutionEvent) {
        if let ExecutionEvent::Log {
            operation_id,
            message,
        } = &event
        {
            self.push_log(operation_id.as_str(), message);
            return;
        }
        match &event {
            ExecutionEvent::Failed { operation_id, .. } => {
                if self.first_failed.is_none() {
                    self.first_failed = Some(operation_id.as_str().to_string());
                }
                self.finished.push_back(operation_id.as_str().to_string());
            }
            ExecutionEvent::Succeeded { operation_id }
            | ExecutionEvent::Reused { operation_id }
            | ExecutionEvent::Cancelled { operation_id }
            | ExecutionEvent::Skipped { operation_id, .. } => {
                self.finished.push_back(operation_id.as_str().to_string());
            }
            ExecutionEvent::Started { .. } | ExecutionEvent::Log { .. } => {}
        }
        if self.lifecycle.len() == MAX_LIFECYCLE_EVENTS {
            self.lifecycle.pop_front();
        }
        self.lifecycle.push_back(event);
    }

    fn push_log(&mut self, operation_id: &str, message: &str) {
        let log = self.logs.entry(operation_id.to_string()).or_default();
        for line in sanitize_tui_lines(message) {
            if log.lines.len() == MAX_LOG_LINES_PER_OPERATION {
                let old = log.lines.pop_front().unwrap_or_default();
                log.dropped += 1;
                log.bytes -= old.len();
                self.log_bytes -= old.len();
            }
            log.bytes += line.len();
            self.log_bytes += line.len();
            log.lines.push_back(line);
        }
        if self.log_bytes > self.budget {
            self.evict_log_bytes();
        }
    }

    /// Drops the oldest lines until the log is at 7/8 of its budget, so that
    /// eviction runs in batches rather than with every line.
    fn evict_log_bytes(&mut self) {
        let target = self.budget - self.budget / 8;
        let finished = std::mem::take(&mut self.finished);
        for operation_id in &finished {
            if self.log_bytes <= target {
                break;
            }
            if let Some(log) = self.logs.get_mut(operation_id) {
                evict_front(log, &mut self.log_bytes, target);
            }
        }
        self.finished = finished;
        while self.log_bytes > target {
            let largest = self
                .logs
                .iter()
                .max_by_key(|(_, log)| log.bytes)
                .map(|(operation_id, _)| operation_id.clone());
            let Some(log) = largest.and_then(|id| self.logs.get_mut(&id)) else {
                break;
            };
            if log.bytes == 0 {
                break;
            }
            evict_front(log, &mut self.log_bytes, target);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lifecycle.is_empty() && self.logs.is_empty()
    }

    pub(crate) fn log(&self, operation_id: &str) -> Option<&OperationLog> {
        self.logs.get(operation_id)
    }

    /// Bytes of log text kept across all operations.
    #[cfg(test)]
    pub(crate) fn log_bytes(&self) -> usize {
        self.log_bytes
    }

    pub(crate) fn first_failed(&self) -> Option<&str> {
        self.first_failed.as_deref()
    }
}

/// Drops the oldest lines of `log` until the total is at most `target`, or
/// the log is empty.
fn evict_front(log: &mut OperationLog, total: &mut usize, target: usize) {
    while *total > target {
        let Some(line) = log.lines.pop_front() else {
            break;
        };
        log.dropped += 1;
        log.bytes -= line.len();
        *total -= line.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_plan::OperationId;

    fn id(value: &str) -> OperationId {
        OperationId::new(value)
    }

    fn log_line(operation: &str, message: &str) -> ExecutionEvent {
        ExecutionEvent::Log {
            operation_id: id(operation),
            message: message.to_string(),
        }
    }

    #[test]
    fn finds_the_first_failed_operation() {
        let mut log = EventLog::default();
        log.push(ExecutionEvent::Started {
            operation_id: id("a"),
        });
        log.push(ExecutionEvent::Succeeded {
            operation_id: id("b"),
        });
        assert_eq!(log.first_failed(), None);
        log.push(ExecutionEvent::Failed {
            operation_id: id("a"),
            message: "boom".into(),
        });
        log.push(ExecutionEvent::Failed {
            operation_id: id("c"),
            message: "later".into(),
        });
        assert_eq!(log.first_failed(), Some("a"));
    }

    #[test]
    fn caps_and_sanitizes_logs_per_operation() {
        let mut log = EventLog::default();
        for index in 0..MAX_LOG_LINES_PER_OPERATION + 3 {
            log.push(ExecutionEvent::Log {
                operation_id: id("a"),
                message: format!("\u{1b}[32mline {index}\u{1b}[0m"),
            });
        }
        let entry = log.log("a").expect("log for a");
        assert_eq!(entry.lines.len(), MAX_LOG_LINES_PER_OPERATION);
        assert_eq!(entry.dropped, 3);
        assert_eq!(entry.lines.front().map(String::as_str), Some("line 3"));
        assert!(!log.is_empty());
    }

    #[test]
    fn lifecycle_events_are_capped_and_the_first_failure_survives() {
        let mut log = EventLog::default();
        log.push(ExecutionEvent::Failed {
            operation_id: id("first"),
            message: "boom".into(),
        });
        for index in 0..MAX_LIFECYCLE_EVENTS + 50 {
            log.push(ExecutionEvent::Started {
                operation_id: id(&format!("op-{index}")),
            });
        }
        assert_eq!(log.lifecycle.len(), MAX_LIFECYCLE_EVENTS);
        assert_eq!(log.first_failed(), Some("first"));
    }

    #[test]
    fn the_log_budget_evicts_finished_operations_first() {
        // Lines of 100 bytes; the budget of 1000 bytes is cut to 875 when it
        // is exceeded.
        let line = |tag: &str, index: usize| format!("{tag}{index:0>99}");
        let mut log = EventLog::with_log_budget(1000);
        for index in 0..6 {
            log.push(log_line("a", &line("a", index)));
        }
        log.push(ExecutionEvent::Succeeded {
            operation_id: id("a"),
        });
        for index in 0..6 {
            log.push(log_line("b", &line("b", index)));
        }

        assert!(log.log_bytes() <= 1000);
        let a = log.log("a").expect("log for a");
        assert_eq!(
            a.lines.len(),
            3,
            "the finished operation gave up its oldest lines"
        );
        assert_eq!(a.lines.back(), Some(&line("a", 5)));
        assert_eq!(a.dropped, 3);
        // The running operation keeps everything.
        assert_eq!(log.log("b").expect("log for b").lines.len(), 6);
    }

    #[test]
    fn the_log_budget_falls_back_to_the_largest_operation() {
        let line = |tag: &str, index: usize| format!("{tag}{index:0>99}");
        let mut log = EventLog::with_log_budget(1000);
        for index in 0..4 {
            log.push(log_line("small", &line("s", index)));
        }
        for index in 0..10 {
            log.push(log_line("big", &line("g", index)));
        }
        assert!(log.log_bytes() <= 1000);
        let big = log.log("big").expect("log for big");
        assert!(big.lines.len() < 10);
        assert_eq!(big.lines.back(), Some(&line("g", 9)));
        // The smaller, still running operation is only touched when the big
        // one is empty, and it is not here.
        assert_eq!(log.log("small").expect("log for small").lines.len(), 4);
    }
}
