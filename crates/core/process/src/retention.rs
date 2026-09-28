//! Bounded retention of child stdout/stderr and draining of reader messages.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    MAX_RETAINED_STREAM_BYTES, MAX_RETAINED_STREAM_LINES, ProcessLogStream, ProcessOutputRetention,
    StreamMessage,
};

/// Bounded tail of one output stream. Retaining the newest output costs
/// O(new data), not O(retained data): lines live in a ring buffer, and bytes
/// are appended with memcpy behind a moving start offset that is compacted
/// at most once per `max_bytes` of input.
#[derive(Debug)]
pub(crate) struct RetainedStream {
    pub(crate) lines: VecDeque<String>,
    bytes: Vec<u8>,
    /// Retained bytes are `bytes[bytes_start..]`.
    bytes_start: usize,
    max_lines: usize,
    max_bytes: usize,
}

impl Default for RetainedStream {
    fn default() -> Self {
        Self::with_limits(MAX_RETAINED_STREAM_LINES, MAX_RETAINED_STREAM_BYTES)
    }
}

impl RetainedStream {
    pub(crate) fn with_limits(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: Vec::new(),
            bytes_start: 0,
            max_lines,
            max_bytes,
        }
    }

    pub(crate) fn push_line(&mut self, line: String) {
        if self.max_lines == 0 {
            return;
        }
        if self.lines.len() == self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        if self.max_bytes == 0 {
            return;
        }
        if bytes.len() >= self.max_bytes {
            self.bytes.clear();
            self.bytes_start = 0;
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - self.max_bytes..]);
            return;
        }
        self.bytes.extend_from_slice(bytes);
        let retained = self.bytes.len() - self.bytes_start;
        if retained > self.max_bytes {
            self.bytes_start += retained - self.max_bytes;
        }
        // Compact once the dead prefix is as large as the retained tail, so
        // the memmove cost is amortized over at least `max_bytes` of input.
        if self.bytes_start >= self.max_bytes {
            self.bytes.drain(..self.bytes_start);
            self.bytes_start = 0;
        }
    }

    pub(crate) fn take_lines(&mut self) -> Vec<String> {
        Vec::from(std::mem::take(&mut self.lines))
    }

    pub(crate) fn take_bytes(&mut self) -> Vec<u8> {
        let mut bytes = std::mem::take(&mut self.bytes);
        bytes.drain(..std::mem::take(&mut self.bytes_start));
        bytes
    }
}

#[derive(Debug, Default)]
pub(crate) struct StreamDrainState {
    pub(crate) stdout: RetainedStream,
    pub(crate) stderr: RetainedStream,
    pub(crate) stdout_done: bool,
    pub(crate) stderr_done: bool,
}

impl StreamDrainState {
    pub(crate) fn with_retention(retention: ProcessOutputRetention) -> Self {
        Self {
            stdout: RetainedStream::with_limits(retention.stdout_lines, retention.stdout_bytes),
            stderr: RetainedStream::with_limits(retention.stderr_lines, retention.stderr_bytes),
            stdout_done: false,
            stderr_done: false,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.stdout_done && self.stderr_done
    }
}

/// Waits up to `tick` for stream output instead of sleeping blindly, so a
/// chatty child never stalls on the bounded queue while the loop sleeps.
pub(crate) fn wait_for_stream_message(
    rx: &Receiver<StreamMessage>,
    state: &mut StreamDrainState,
    tick: Duration,
) {
    if state.is_done() {
        thread::sleep(tick);
        return;
    }
    match rx.recv_timeout(tick) {
        Ok(message) => apply_stream_message(message, state),
        Err(RecvTimeoutError::Timeout) => {}
        // Readers are gone without reporting completion; avoid spinning.
        Err(RecvTimeoutError::Disconnected) => thread::sleep(tick),
    }
}

pub(crate) fn drain_stream_messages(rx: &Receiver<StreamMessage>, state: &mut StreamDrainState) {
    while let Ok(message) = rx.try_recv() {
        apply_stream_message(message, state);
    }
}

pub(crate) fn drain_stream_messages_until_idle(
    rx: &Receiver<StreamMessage>,
    state: &mut StreamDrainState,
    idle_timeout: Duration,
) {
    let deadline = Instant::now() + idle_timeout;
    while !state.is_done() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match rx.recv_timeout(remaining) {
            Ok(message) => apply_stream_message(message, state),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    drain_stream_messages(rx, state);
}

fn apply_stream_message(message: StreamMessage, state: &mut StreamDrainState) {
    match message {
        StreamMessage::Bytes {
            stream: ProcessLogStream::Stdout,
            bytes,
        } => {
            state.stdout.push_bytes(&bytes);
        }
        StreamMessage::Bytes {
            stream: ProcessLogStream::Stderr,
            bytes,
        } => {
            state.stderr.push_bytes(&bytes);
        }
        StreamMessage::Line {
            stream: ProcessLogStream::Stdout,
            line,
        } => {
            state.stdout.push_line(line);
        }
        StreamMessage::Line {
            stream: ProcessLogStream::Stderr,
            line,
        } => {
            state.stderr.push_line(line);
        }
        StreamMessage::Done {
            stream: ProcessLogStream::Stdout,
        } => state.stdout_done = true,
        StreamMessage::Done {
            stream: ProcessLogStream::Stderr,
        } => state.stderr_done = true,
    }
}
