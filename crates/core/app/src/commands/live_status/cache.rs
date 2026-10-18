//! Reads of the snapshot files of many build dirs, each file read again only
//! when its modification time or size changed. The picker looks at every build
//! dir's live and last-run snapshots every couple of seconds, and most of them
//! are unchanged between looks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use super::{LAST_FILE, LastRun, LiveRun, LiveStatus, RunFiles, process_stopped, read_json};

/// What a file looked like when it was read: its modification time and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    len: u64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok()?,
        len: meta.len(),
    })
}

/// The values of JSON files, each read again only when its modification time
/// or size changed since the last read. The picker looks at every build dir's
/// snapshots every couple of seconds, and most of them are unchanged.
#[derive(Debug)]
pub(crate) struct FileCache<T> {
    entries: BTreeMap<PathBuf, (FileStamp, Option<T>)>,
}

impl<T> Default for FileCache<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T: Clone + for<'de> Deserialize<'de>> FileCache<T> {
    /// The value of the file at `path`, as [`read_json`] reads it.
    pub(crate) fn read(&mut self, path: &Path) -> Option<T> {
        let Some(stamp) = file_stamp(path) else {
            self.entries.remove(path);
            return None;
        };
        if let Some((cached, value)) = self.entries.get(path)
            && *cached == stamp
        {
            return value.clone();
        }
        let value = read_json::<T>(path);
        self.entries
            .insert(path.to_path_buf(), (stamp, value.clone()));
        value
    }
}

/// The snapshot reads of the picker, for many build dirs at once.
#[derive(Debug, Default)]
pub(crate) struct SnapshotCache {
    live: FileCache<LiveStatus>,
    last: FileCache<LastRun>,
}

impl SnapshotCache {
    /// The live run of `build_dir`, as [`live_run`] reports it; the status
    /// file is read only when it changed.
    pub(crate) fn live_run(&mut self, build_dir: &Path) -> Option<LiveRun> {
        #[cfg(unix)]
        {
            let files = RunFiles::in_build_dir(build_dir);
            let pid = u32::try_from(crate::commands::interrupt::running_gaia(&files.pid)?).ok()?;
            let status = self
                .live
                .read(&files.status)
                .filter(|status| status.pid == pid);
            Some(LiveRun {
                pid,
                status,
                stopped: process_stopped(pid),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (self, build_dir);
            None
        }
    }

    /// The final snapshot of the last run of `build_dir`, as [`read_last`]
    /// reads it; the file is read only when it changed.
    pub(crate) fn read_last(&mut self, build_dir: &Path) -> Option<LastRun> {
        self.last.read(&build_dir.join(LAST_FILE))
    }
}
