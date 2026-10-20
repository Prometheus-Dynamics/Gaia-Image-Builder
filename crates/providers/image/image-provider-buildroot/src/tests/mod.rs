use super::*;
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec,
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootImageSpec,
    ImageAssemblySpec, ImageDefinition, ImageOutputSpec, ImageSpec,
};
use gaia_testkit::TempDir;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

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

mod archive_outputs;
mod artifact_target;
mod buildroot_command;
mod buildroot_preview;
mod config_digest;
mod config_inputs_skip;
mod config_settings;
mod erofs_lzma;
mod erofs_workers;
mod feed_archive;
mod feed_archive_expected_images;
mod feed_archive_overlay;
mod finalize_marker;
mod make_retry;
mod offline_prefetch;
mod override_check;
mod provider_squashfs_fs;
mod single_pass_shared;
mod stage_owned;
mod step_times;
