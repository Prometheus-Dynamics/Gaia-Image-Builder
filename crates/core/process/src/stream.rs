use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::SyncSender;
use std::thread;

use crate::{
    MAX_RETAINED_LINE_BYTES, ProcessLogLine, ProcessLogSink, ProcessLogStream,
    STREAM_READ_BUFFER_BYTES, StreamMessage,
};

/// What a reader must hand to the drain: the newest `bytes` bytes and the
/// newest `lines` lines of its stream are retained, so older output is never
/// sent. A log sink needs every line, whatever is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamKeep {
    pub(crate) bytes: usize,
    pub(crate) lines: usize,
    pub(crate) sink: bool,
}

impl StreamKeep {
    /// Whether the reader must split the stream into lines at all.
    fn wants_lines(self) -> bool {
        self.lines > 0 || self.sink
    }
}

pub(crate) fn spawn_stream_reader(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
    keep: StreamKeep,
    tx: SyncSender<StreamMessage>,
) {
    thread::spawn(move || {
        read_stream(stream, which, sink, keep, tx);
    });
}

fn read_stream(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
    keep: StreamKeep,
    tx: SyncSender<StreamMessage>,
) {
    let Some(stream) = stream else {
        let _ = tx.send(StreamMessage::Done { stream: which });
        return;
    };
    let mut reader = stream;
    let mut buffer = [0; STREAM_READ_BUFFER_BYTES];
    let mut line = BoundedLine::default();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let bytes = &buffer[..read];
                // Only the tail of a chunk can survive the drain's byte
                // retention, so the rest is never copied or sent.
                if keep.bytes > 0 {
                    let tail = &bytes[bytes.len().saturating_sub(keep.bytes)..];
                    if tx
                        .send(StreamMessage::Bytes {
                            stream: which,
                            bytes: tail.to_vec(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                if keep.wants_lines() {
                    let mut batch = LineBatch::new(keep.lines);
                    process_stream_lines(bytes, which, sink.as_ref(), &mut line, &mut batch);
                    if !batch.is_empty() && !send_lines(which, &tx, batch) {
                        return;
                    }
                }
            }
            Err(_) => break,
        }
    }
    if keep.wants_lines() && line.has_pending() {
        let mut batch = LineBatch::new(keep.lines);
        emit_line(which, sink.as_ref(), &mut batch, line.take_line());
        if !batch.is_empty() {
            let _ = send_lines(which, &tx, batch);
        }
    }
    let _ = tx.send(StreamMessage::Done { stream: which });
}

/// The lines of one chunk that the drain may retain: at most the newest
/// `capacity`, in order. Sending them as one message keeps the channel off
/// the per-line path.
#[derive(Debug)]
struct LineBatch {
    capacity: usize,
    lines: VecDeque<String>,
}

impl LineBatch {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            lines: VecDeque::new(),
        }
    }

    fn push(&mut self, line: String) {
        if self.capacity == 0 {
            return;
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

fn send_lines(which: ProcessLogStream, tx: &SyncSender<StreamMessage>, batch: LineBatch) -> bool {
    tx.send(StreamMessage::Lines {
        stream: which,
        lines: Vec::from(batch.lines),
    })
    .is_ok()
}

#[derive(Debug, Default)]
struct BoundedLine {
    bytes: Vec<u8>,
    pending: bool,
}

impl BoundedLine {
    /// Appends `bytes` as [`push`](Self::push) would, one byte at a time: the
    /// line keeps its newest `MAX_RETAINED_LINE_BYTES` bytes, dropping the
    /// oldest half when it is full.
    fn push_slice(&mut self, mut bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.pending = true;
        while !bytes.is_empty() {
            if self.bytes.len() == MAX_RETAINED_LINE_BYTES {
                self.bytes.drain(..MAX_RETAINED_LINE_BYTES / 2);
            }
            let take = (MAX_RETAINED_LINE_BYTES - self.bytes.len()).min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
    }

    fn has_pending(&self) -> bool {
        self.pending
    }

    fn take_line(&mut self) -> String {
        if self.bytes.last() == Some(&b'\r') {
            self.bytes.pop();
        }
        let line = String::from_utf8_lossy(&self.bytes).to_string();
        self.bytes.clear();
        self.pending = false;
        line
    }
}

/// Splits `bytes` into lines, continuing a line left open by the previous
/// chunk. Every line goes to the sink; the batch keeps what may be retained.
fn process_stream_lines(
    bytes: &[u8],
    which: ProcessLogStream,
    sink: Option<&ProcessLogSink>,
    line: &mut BoundedLine,
    batch: &mut LineBatch,
) {
    let mut rest = bytes;
    while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
        line.push_slice(&rest[..end]);
        emit_line(which, sink, batch, line.take_line());
        rest = &rest[end + 1..];
    }
    line.push_slice(rest);
}

fn emit_line(
    which: ProcessLogStream,
    sink: Option<&ProcessLogSink>,
    batch: &mut LineBatch,
    line: String,
) {
    if let Some(sink) = sink
        && !line.is_empty()
    {
        sink(ProcessLogLine {
            stream: which,
            line: line.clone(),
        });
    }
    batch.push(line);
}
