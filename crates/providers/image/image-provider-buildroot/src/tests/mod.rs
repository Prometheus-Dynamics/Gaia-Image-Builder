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
mod feed_archive;
mod feed_archive_expected_images;
mod feed_archive_overlay;
mod provider_squashfs_fs;
