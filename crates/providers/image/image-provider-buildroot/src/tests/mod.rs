use super::*;
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec,
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootImageSpec,
    ImageAssemblySpec, ImageDefinition, ImageOutputSpec, ImageSpec,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static TEMP_PATH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let counter = TEMP_PATH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{}-{counter}-{nonce}", std::process::id()))
}

/// Writes an executable fake tool without ever holding a write descriptor to
/// it in this process. Tests run in parallel threads; a child forked by
/// another thread inherits any open write descriptor until it execs, and
/// executing a file that still has a writer fails with ETXTBSY ("Text file
/// busy"). The body is written to a side file and `install` (a separate
/// process whose descriptors no test thread can inherit) creates the
/// executable.
fn write_executable(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("script dir");
    }
    let staged = PathBuf::from(format!("{}.gaia-script-body", path.display()));
    fs::write(&staged, body).expect("script body");
    let status = Command::new("install")
        .arg("-m")
        .arg("0755")
        .arg(&staged)
        .arg(path)
        .status()
        .expect("run install for fake script");
    assert!(status.success(), "install fake script '{}'", path.display());
    let _ = fs::remove_file(staged);
}

fn test_execution() -> ImageExecutionContext {
    ImageExecutionContext {
        workspace_root: std::env::temp_dir(),
        docker_image: None,
    }
}

fn test_command_context<'a>(
    execution: &'a ImageExecutionContext,
    policy: &'a ImageExecutionPolicy,
) -> ImageCommandContext<'a> {
    ImageCommandContext {
        execution,
        policy,
        log_sink: None,
        cancel_check: None,
    }
}

mod buildroot_command;
mod config_digest;
mod feed_archive;
mod feed_archive_expected_images;
mod feed_archive_overlay;
mod provider_squashfs_fs;
mod single_pass_shared;
