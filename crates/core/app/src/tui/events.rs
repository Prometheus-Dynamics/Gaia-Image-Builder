use std::collections::{HashMap, VecDeque};

use super::*;

/// Upper bound on retained log lines per operation. Buildroot can emit hundreds
/// of thousands of lines; the TUI only needs a readable tail.
const MAX_LOG_LINES_PER_OPERATION: usize = 5_000;

/// The events of the run this TUI started: the lifecycle events, in order,
/// and each operation's output, kept per operation so the operation popup
/// can show its tail.
#[derive(Default)]
pub(crate) struct EventLog {
    /// Lifecycle events (everything except logs), in arrival order.
    lifecycle: Vec<ExecutionEvent>,
    logs: HashMap<String, OperationLog>,
}

#[derive(Default)]
pub(crate) struct OperationLog {
    pub(crate) lines: VecDeque<String>,
    pub(crate) dropped: usize,
}

impl EventLog {
    pub(crate) fn from_events<'e>(events: impl IntoIterator<Item = &'e ExecutionEvent>) -> Self {
        let mut log = Self::default();
        for event in events {
            log.push(event.clone());
        }
        log
    }

    pub(crate) fn push(&mut self, event: ExecutionEvent) {
        if let ExecutionEvent::Log {
            operation_id,
            message,
        } = &event
        {
            let log = self
                .logs
                .entry(operation_id.as_str().to_string())
                .or_default();
            for line in sanitize_tui_lines(message) {
                if log.lines.len() == MAX_LOG_LINES_PER_OPERATION {
                    log.lines.pop_front();
                    log.dropped += 1;
                }
                log.lines.push_back(line);
            }
            return;
        }
        self.lifecycle.push(event);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lifecycle.is_empty() && self.logs.is_empty()
    }

    pub(crate) fn log(&self, operation_id: &str) -> Option<&OperationLog> {
        self.logs.get(operation_id)
    }

    pub(crate) fn first_failed(&self) -> Option<&str> {
        self.lifecycle.iter().find_map(|event| match event {
            ExecutionEvent::Failed { operation_id, .. } => Some(operation_id.as_str()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_plan::OperationId;

    fn id(value: &str) -> OperationId {
        OperationId::new(value)
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
}
