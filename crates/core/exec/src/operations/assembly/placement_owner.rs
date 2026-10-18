//! The RAM work dirs of a shared tmpfs: the `gaia-<user>` dir must be the's
//! user's own, and the dirs of dead runs are removed before a run starts.

use super::*;
use std::time::SystemTime;

/// A RAM work dir whose owner is gone is removed once it is this old.
const RAM_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// The file in a `<hash>` dir that names the run owning it: `pid`,
/// `started` (unix seconds) and `build_dir`.
const RAM_OWNER_FILE: &str = "owner";

fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std_fs::metadata("/proc/self")
        .ok()
        .map(|metadata| metadata.uid())
}

/// Checks that `<base>/gaia-<user>` is a real directory this user owns, and
/// creates it private (0700) when it is missing. A symbolic link, a file or
/// a directory another user owns is an error: the RAM base is world-writable,
/// so anyone may have made the name first.
pub(super) fn check_ram_user_dir(base: &Path, user: &str) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let dir = base.join(format!("gaia-{user}"));
    let uid = current_uid().ok_or_else(|| {
        format!(
            "cannot read the current user id to check '{}'",
            dir.display()
        )
    })?;
    std_fs::create_dir_all(base).map_err(|error| {
        format!(
            "failed to create the RAM base '{}': {error}",
            base.display()
        )
    })?;
    match std_fs::symlink_metadata(&dir) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "'{}' is a symbolic link; refusing to build its RAM work dir there",
                    dir.display()
                ));
            }
            if !metadata.is_dir() {
                return Err(format!(
                    "'{}' is not a directory; refusing to build its RAM work dir there",
                    dir.display()
                ));
            }
            if metadata.uid() != uid {
                return Err(format!(
                    "'{}' is owned by uid {}, not by the current user (uid {uid}); refusing to build its RAM work dir there",
                    dir.display(),
                    metadata.uid()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std_fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|error| format!("failed to create '{}': {error}", dir.display())),
        Err(error) => Err(format!("failed to read '{}': {error}", dir.display())),
    }
}

/// Records which run owns a RAM work dir: its pid, its start and the build
/// dir the run belongs to.
pub(super) fn write_ram_owner(hash_dir: &Path, build_dir: &Path) -> Result<(), String> {
    let started = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    std_fs::create_dir_all(hash_dir).map_err(|error| {
        format!(
            "failed to create RAM work dir '{}': {error}",
            hash_dir.display()
        )
    })?;
    std_fs::write(
        hash_dir.join(RAM_OWNER_FILE),
        format!(
            "pid={}\nstarted={started}\nbuild_dir={}\n",
            std::process::id(),
            build_dir.display()
        ),
    )
    .map_err(|error| {
        format!(
            "failed to record the owner of '{}': {error}",
            hash_dir.display()
        )
    })
}

/// The owner a `<hash>` dir records, when its marker reads back.
struct RamOwner {
    pid: Option<u32>,
    started: Option<u64>,
    build_dir: Option<PathBuf>,
}

fn read_ram_owner(hash_dir: &Path) -> Option<RamOwner> {
    let text = std_fs::read_to_string(hash_dir.join(RAM_OWNER_FILE)).ok()?;
    let mut owner = RamOwner {
        pid: None,
        started: None,
        build_dir: None,
    };
    for line in text.lines() {
        match line.split_once('=') {
            Some(("pid", value)) => owner.pid = value.parse().ok(),
            Some(("started", value)) => owner.started = value.parse().ok(),
            Some(("build_dir", value)) => owner.build_dir = Some(PathBuf::from(value)),
            _ => {}
        }
    }
    Some(owner)
}

/// Removes the `<hash>` dirs under `user_dir` that a dead run left behind:
/// older than [`RAM_STALE_AFTER`], and either with no marker, with a pid that
/// is gone, or with a build dir that no longer exists. `keep` (this run's
/// `<hash>` dir) is never touched, and neither is any link.
pub(super) fn sweep_stale_ram_dirs(user_dir: &Path, keep: &Path) {
    let Ok(entries) = std_fs::read_dir(user_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir == keep {
            continue;
        }
        let Ok(metadata) = std_fs::symlink_metadata(&dir) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let owner = read_ram_owner(&dir);
        let started = owner
            .as_ref()
            .and_then(|owner| owner.started)
            .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .or_else(|| metadata.modified().ok());
        let old_enough = started
            .and_then(|started| now.duration_since(started).ok())
            .is_some_and(|age| age >= RAM_STALE_AFTER);
        let owner_gone = match &owner {
            None => true,
            Some(owner) => {
                let pid_alive = owner
                    .pid
                    .is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists());
                !pid_alive || owner.build_dir.as_ref().is_none_or(|build| !build.exists())
            }
        };
        if old_enough && owner_gone {
            let _ = std_fs::remove_dir_all(&dir);
        }
    }
}
