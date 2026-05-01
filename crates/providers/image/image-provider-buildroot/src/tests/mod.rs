use super::*;
use gaia_spec::{
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootImageSpec,
    ImageDefinition, ImageOutputSpec, ImageSpec,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}"))
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
mod provider_squashfs_fs;
