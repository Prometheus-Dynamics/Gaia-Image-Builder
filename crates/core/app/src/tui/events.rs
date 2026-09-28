use std::collections::{HashMap, VecDeque};

use super::*;

/// Upper bound on retained log lines per operation. Buildroot can emit hundreds
/// of thousands of lines; the TUI only needs a readable tail.
const MAX_LOG_LINES_PER_OPERATION: usize = 5_000;

/// Execution events indexed by operation as they arrive, so rendering never
/// rescans the full event stream.
#[derive(Default)]
pub(crate) struct EventLog {
    /// Lifecycle events (everything except logs), in arrival order.
    lifecycle: Vec<ExecutionEvent>,
    statuses: HashMap<String, OperationStatus>,
    logs: HashMap<String, OperationLog>,
    /// Operations started but not yet finished, oldest first.
    running: Vec<String>,
    completed: usize,
}

#[derive(Default)]
pub(crate) struct OperationLog {
    pub(crate) lines: VecDeque<String>,
    pub(crate) dropped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationStatus {
    Running,
    Succeeded,
    Reused,
    Cancelled,
    Failed,
}

impl OperationStatus {
    pub(crate) fn badge(self) -> (&'static str, Color) {
        match self {
            Self::Running => ("RUN", Color::LightCyan),
            Self::Succeeded => ("OK", Color::Green),
            Self::Reused => ("REUSE", Color::LightBlue),
            Self::Cancelled => ("CANCEL", Color::LightYellow),
            Self::Failed => ("FAIL", Color::Red),
        }
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

    pub(crate) fn push(&mut self, event: ExecutionEvent) {
        let (operation_id, status) = match &event {
            ExecutionEvent::Log {
                operation_id,
                message,
            } => {
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
            ExecutionEvent::Started { operation_id } => (operation_id, OperationStatus::Running),
            ExecutionEvent::Succeeded { operation_id } => {
                (operation_id, OperationStatus::Succeeded)
            }
            ExecutionEvent::Reused { operation_id } => (operation_id, OperationStatus::Reused),
            ExecutionEvent::Cancelled { operation_id } => {
                (operation_id, OperationStatus::Cancelled)
            }
            ExecutionEvent::Failed { operation_id, .. } => (operation_id, OperationStatus::Failed),
        };
        let id = operation_id.as_str().to_string();
        self.running.retain(|running| running != &id);
        match status {
            OperationStatus::Running => self.running.push(id.clone()),
            OperationStatus::Succeeded | OperationStatus::Reused => self.completed += 1,
            OperationStatus::Cancelled | OperationStatus::Failed => {}
        }
        self.statuses.insert(id, status);
        self.lifecycle.push(event);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lifecycle.is_empty() && self.logs.is_empty()
    }

    pub(crate) fn lifecycle(&self) -> &[ExecutionEvent] {
        &self.lifecycle
    }

    pub(crate) fn status(&self, operation_id: &str) -> Option<OperationStatus> {
        self.statuses.get(operation_id).copied()
    }

    pub(crate) fn log(&self, operation_id: &str) -> Option<&OperationLog> {
        self.logs.get(operation_id)
    }

    pub(crate) fn running(&self) -> &[String] {
        &self.running
    }

    /// The most recently started operation that is still running.
    pub(crate) fn newest_running(&self) -> Option<&str> {
        self.running.last().map(String::as_str)
    }

    pub(crate) fn completed_count(&self) -> usize {
        self.completed
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
    fn tracks_status_running_set_and_completion() {
        let mut log = EventLog::default();
        log.push(ExecutionEvent::Started {
            operation_id: id("a"),
        });
        log.push(ExecutionEvent::Started {
            operation_id: id("b"),
        });
        assert_eq!(log.running(), ["a".to_string(), "b".to_string()]);
        assert_eq!(log.newest_running(), Some("b"));

        log.push(ExecutionEvent::Succeeded {
            operation_id: id("b"),
        });
        log.push(ExecutionEvent::Failed {
            operation_id: id("a"),
            message: "boom".into(),
        });
        assert!(log.running().is_empty());
        assert_eq!(log.completed_count(), 1);
        assert_eq!(log.status("a"), Some(OperationStatus::Failed));
        assert_eq!(log.status("b"), Some(OperationStatus::Succeeded));
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
        assert!(log.lifecycle().is_empty());
    }
}
