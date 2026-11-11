//! Run logs: every tool run's whole output, in a file of its own.
//!
//! Logs live in a `logs` directory next to the store's root, one directory
//! per step name, one file per tool run, named so they sort by start time:
//!
//! ```text
//! <store parent>/logs/<step>/<start ns>-<key, 16 hex>-<pid>-<n>.log
//! ```
//!
//! A log is not a store entry and is not in the step's scratch, so a failed
//! step keeps it. Nothing deletes logs automatically.
//!
//! A log reads:
//!
//! ```text
//! # step test.tool
//! # key sha256:...
//! # tool /bin/sh -c ...
//! # start 1762800000000000000
//! out| a line the tool printed on stdout
//! err| a line on stderr
//! # end 1762800000150000000 exit 3
//! ```

use gaia_store::{StepKey, Store};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The run log directory of `store`: `logs` next to its root.
pub fn logs_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("logs")
}

/// Nanoseconds since the Unix epoch.
pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

#[derive(Debug)]
pub(crate) struct RunLog {
    pub(crate) path: PathBuf,
    file: Mutex<File>,
}

impl RunLog {
    pub(crate) fn create(
        logs: &Path,
        step: &str,
        key: &StepKey,
        command: &str,
    ) -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = logs.join(file_name(step));
        fs::create_dir_all(&dir)?;
        let start = now_ns();
        let path = dir.join(format!(
            "{start}-{}-{}-{}.log",
            &key.0.hex()[..16],
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::create(&path)?;
        writeln!(
            file,
            "# step {step}\n# key {}\n# tool {command}\n# start {start}",
            key.0
        )?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    pub(crate) fn line(&self, prefix: &str, text: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{prefix}| {text}");
        }
    }

    pub(crate) fn end(&self, status: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "# end {} {status}", now_ns());
            let _ = file.flush();
        }
    }
}

/// A step name as a directory name: anything but letters, digits, `.`, `-`
/// and `_` becomes `_`.
fn file_name(step: &str) -> String {
    step.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
