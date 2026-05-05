use std::io::Read;
use std::sync::mpsc::SyncSender;
use std::thread;

use crate::{
    MAX_RETAINED_LINE_BYTES, ProcessLogLine, ProcessLogSink, ProcessLogStream,
    STREAM_READ_BUFFER_BYTES, StreamMessage,
};

pub(crate) fn spawn_stream_reader(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
    tx: SyncSender<StreamMessage>,
) {
    thread::spawn(move || {
        read_stream(stream, which, sink, tx);
    });
}

fn read_stream(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
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
                if tx
                    .send(StreamMessage::Bytes {
                        stream: which,
                        bytes: bytes.to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
                if !process_stream_lines(bytes, which, sink.as_ref(), &tx, &mut line) {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    if line.has_pending() {
        let line = line.take_line();
        emit_stream_line(which, sink.as_ref(), &tx, line);
    }
    let _ = tx.send(StreamMessage::Done { stream: which });
}

#[derive(Debug, Default)]
struct BoundedLine {
    bytes: Vec<u8>,
    pending: bool,
}

impl BoundedLine {
    fn push(&mut self, byte: u8) {
        self.pending = true;
        if self.bytes.len() == MAX_RETAINED_LINE_BYTES {
            self.bytes.drain(..MAX_RETAINED_LINE_BYTES / 2);
        }
        self.bytes.push(byte);
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

fn process_stream_lines(
    bytes: &[u8],
    which: ProcessLogStream,
    sink: Option<&ProcessLogSink>,
    tx: &SyncSender<StreamMessage>,
    line: &mut BoundedLine,
) -> bool {
    for byte in bytes {
        if *byte == b'\n' {
            let line = line.take_line();
            if !emit_stream_line(which, sink, tx, line) {
                return false;
            }
        } else {
            line.push(*byte);
        }
    }
    true
}

fn emit_stream_line(
    which: ProcessLogStream,
    sink: Option<&ProcessLogSink>,
    tx: &SyncSender<StreamMessage>,
    line: String,
) -> bool {
    if let Some(sink) = sink
        && !line.is_empty()
    {
        sink(ProcessLogLine {
            stream: which,
            line: line.clone(),
        });
    }
    tx.send(StreamMessage::Line {
        stream: which,
        line,
    })
    .is_ok()
}
