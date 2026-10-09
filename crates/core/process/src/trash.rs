//! Large deletes that do not hold up the build.
//!
//! Removing a Buildroot output tree (hundreds of thousands of files) can take
//! hours on a busy disk. [`discard`] instead renames the path into a
//! `.gaia-trash` directory next to it, which is instant on the same
//! filesystem, and deletes the trash in a detached background process at idle
//! IO and CPU priority (`ionice -c3 nice -n19`). Trash left by an interrupted
//! delete is picked up again by the next [`discard`] or [`purge_trash`] in the
//! same directory.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The directory discarded paths are moved into, next to them.
pub const TRASH_DIR: &str = ".gaia-trash";

/// Removes `path` (a file, symlink or directory; missing is fine): moves it
/// into `<parent>/.gaia-trash` and deletes it in the background, or deletes
/// it in place when it cannot be moved there.
pub fn discard(path: &Path) -> std::io::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    let Some(trash) = path.parent().map(|parent| parent.join(TRASH_DIR)) else {
        return fs::remove_dir_all(path);
    };
    if fs::create_dir_all(&trash).is_err()
        || fs::rename(path, trash.join(trash_name(path))).is_err()
    {
        return fs::remove_dir_all(path);
    }
    purge_trash(&trash);
    Ok(())
}

/// Starts deleting what `trash` (a `.gaia-trash` directory) holds in the
/// background, and the directory itself once empty.
pub fn purge_trash(trash: &Path) {
    let entries = fs::read_dir(trash)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if entries.is_empty() {
        let _ = fs::remove_dir(trash);
        return;
    }
    // `rm -rf` the entries, then `rmdir` the trash if nothing else was put
    // there meanwhile.
    let script = r#"trash="$1"; shift; rm -rf -- "$@"; rmdir -- "$trash" 2>/dev/null; exit 0"#;
    let launchers: [&[&str]; 3] = [&["ionice", "-c3", "nice", "-n19"], &["nice", "-n19"], &[]];
    for launcher in launchers {
        let mut command = match launcher.split_first() {
            Some((program, args)) => {
                let mut command = Command::new(program);
                command.args(args).arg("sh");
                command
            }
            None => Command::new("sh"),
        };
        command
            .arg("-c")
            .arg(script)
            .arg("gaia-trash")
            .arg(trash)
            .args(&entries)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Its own process group, so Ctrl-C of the build does not stop it.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        if let Ok(child) = command.spawn() {
            // Reap it without waiting: a detached thread owns the handle.
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
            return;
        }
    }
    // No shell at all: delete on a low-priority thread instead (stopped if
    // Gaia exits; the rest is resumed next time).
    std::thread::spawn(move || {
        for entry in entries {
            let _ = fs::remove_dir_all(&entry).or_else(|_| fs::remove_file(&entry));
        }
    });
}

fn trash_name(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "path".to_string());
    PathBuf::from(format!(
        "{name}-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn discarded_trees_vanish_at_once_and_are_deleted_in_the_background() {
        let root = std::env::temp_dir().join(format!("gaia-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let tree = root.join("output/build");
        fs::create_dir_all(tree.join("pkg/sub")).expect("tree");
        fs::write(tree.join("pkg/sub/file"), b"x").expect("file");
        fs::write(root.join("output/keep"), b"keep").expect("keep");

        discard(&tree).expect("discard");
        assert!(!tree.exists());
        assert!(root.join("output/keep").is_file());
        let trash = root.join("output").join(TRASH_DIR);
        let deadline = Instant::now() + Duration::from_secs(20);
        while trash.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!trash.exists(), "trash was not purged");

        // Files and missing paths.
        discard(&root.join("output/keep")).expect("file");
        assert!(!root.join("output/keep").exists());
        discard(&root.join("output/missing")).expect("missing");
        let _ = fs::remove_dir_all(root);
    }
}
