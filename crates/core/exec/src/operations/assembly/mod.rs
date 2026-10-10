use super::*;
use gaia_spec::KeyValueState;
use gaia_spec::{AssemblyRoots, ImageAssemblySpec};
use sha2::{Digest, Sha256};
use std::fs as std_fs;
#[cfg(test)]
use std::io::Read;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

use super::helpers::{process_output_retention, runtime_state_dir};

mod archive_log;
mod archives;
mod busybox;
mod disks;
mod files;
mod filesystems;
mod mbr;
mod placement;
mod state;
mod steps;
mod tar;
mod transforms;

use archive_log::*;
use archives::*;
use busybox::*;
use disks::*;
use files::*;
use filesystems::*;
use mbr::*;
use placement::*;
use state::AssemblyExecutionContext;
pub(crate) use state::{assembly_state_path, image_assembly_cleanup_paths};
use steps::{StepRun, ordered_assembly_steps, remove_stale_step_outputs};
use transforms::*;

const TOOL_VERSION_TIMEOUT_SECONDS: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssemblyStagingSummary {
    pub state: KeyValueState,
    pub messages: Vec<String>,
    pub cleanup_paths: Vec<PathBuf>,
    pub archive_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssemblyError {
    pub(crate) kind: ExecutionErrorKind,
    pub(crate) message: String,
}

impl AssemblyError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: ExecutionErrorKind::RuntimeState,
            message: message.into(),
        }
    }

    fn process(command: &Command, error: gaia_process::ProcessRunError) -> Self {
        let kind = match error.kind {
            gaia_process::ProcessRunErrorKind::ToolStart => ExecutionErrorKind::ToolStart,
            gaia_process::ProcessRunErrorKind::Timeout => ExecutionErrorKind::Timeout,
            gaia_process::ProcessRunErrorKind::Cancelled => ExecutionErrorKind::Cancelled,
            gaia_process::ProcessRunErrorKind::RuntimeState => ExecutionErrorKind::RuntimeState,
        };
        Self {
            kind,
            message: format!(
                "assembly command `{}` failed before completion: {}",
                command_display(command),
                error.message
            ),
        }
    }
}

impl From<String> for AssemblyError {
    fn from(message: String) -> Self {
        Self::runtime(message)
    }
}

impl From<&str> for AssemblyError {
    fn from(message: &str) -> Self {
        Self::runtime(message)
    }
}

pub(crate) fn stage_image_assembly(
    spec: &ResolvedBuildSpec,
    operation_id: &OperationId,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyStagingSummary, AssemblyError> {
    stage_image_assembly_with(spec, operation_id, cancel_check, PlacementEnv::system)
}

/// The paths a run's steps use. `roots` and `assembly` are the run's own
/// view (intermediates in RAM when `ram` is set); `disk_roots` and
/// `disk_assembly` are the spec's view, for cleanup and for what is published.
struct AssemblyView<'a> {
    assembly: &'a ImageAssemblySpec,
    roots: &'a AssemblyRoots,
    disk_assembly: &'a ImageAssemblySpec,
    disk_roots: &'a AssemblyRoots,
    ram: Option<&'a RamPlacement>,
}

/// Stages the assembly. `env` supplies the RAM and tmpfs facts; it is read
/// only when the work dir asks for RAM.
pub(crate) fn stage_image_assembly_with(
    spec: &ResolvedBuildSpec,
    operation_id: &OperationId,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    env: impl FnOnce() -> PlacementEnv,
) -> Result<AssemblyStagingSummary, AssemblyError> {
    let Some(assembly) = &spec.image.assembly else {
        return Ok(AssemblyStagingSummary {
            state: KeyValueState::new().with("kind", gaia_spec::IMAGE_ASSEMBLY_STATE_KIND),
            messages: vec!["image assembly has no configured actions".into()],
            cleanup_paths: Vec::new(),
            archive_path: None,
        });
    };
    let disk_roots = AssemblyRoots::new(spec, assembly)?;
    match decide_placement(spec, assembly, &disk_roots, env) {
        AssemblyPlacement::Disk { messages } => stage_steps(
            spec,
            operation_id,
            cancel_check,
            AssemblyView {
                assembly,
                roots: &disk_roots,
                disk_assembly: assembly,
                disk_roots: &disk_roots,
                ram: None,
            },
            messages,
        ),
        AssemblyPlacement::Ram(ram) => {
            // Copies left by an earlier run (or a crash) are re-created.
            discard_ram_root(&ram.root);
            for path in &ram.stale {
                let _ = gaia_process::discard(path);
            }
            let messages = ram.messages.clone();
            let result = stage_steps(
                spec,
                operation_id,
                cancel_check,
                AssemblyView {
                    assembly: &ram.assembly,
                    roots: &ram.roots,
                    disk_assembly: assembly,
                    disk_roots: &disk_roots,
                    ram: Some(&ram),
                },
                messages,
            );
            discard_ram_root(&ram.root);
            result
        }
    }
}

fn stage_steps(
    spec: &ResolvedBuildSpec,
    operation_id: &OperationId,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    view: AssemblyView<'_>,
    mut messages: Vec<String>,
) -> Result<AssemblyStagingSummary, AssemblyError> {
    let span = tracing::info_span!(
        "image_assembly_stage",
        operation_id = %operation_id.as_str(),
        tree_count = tracing::field::Empty,
        dir_count = tracing::field::Empty,
        symlink_count = tracing::field::Empty,
        file_entry_count = tracing::field::Empty,
        transform_count = tracing::field::Empty,
        filesystem_count = tracing::field::Empty,
        disk_count = tracing::field::Empty
    );
    let _stage_span_guard = span.enter();
    let assembly = view.assembly;
    let roots = view.roots;

    tracing::Span::current().record("tree_count", assembly.trees.len());
    tracing::Span::current().record("dir_count", assembly.dirs.len());
    tracing::Span::current().record("symlink_count", assembly.symlinks.len());
    tracing::Span::current().record("file_entry_count", assembly.files.len());
    tracing::Span::current().record("transform_count", assembly.transforms.len());
    tracing::Span::current().record("filesystem_count", assembly.filesystems.len());
    tracing::Span::current().record("disk_count", assembly.disks.len());

    let context = AssemblyExecutionContext::new(spec, view.disk_assembly, view.disk_roots);
    let mut state = KeyValueState::new()
        .with("kind", gaia_spec::IMAGE_ASSEMBLY_STATE_KIND)
        .with("tree_count", assembly.trees.len())
        .with("dir_count", assembly.dirs.len())
        .with("symlink_count", assembly.symlinks.len())
        .with("file_entry_count", assembly.files.len())
        .with("transform_count", assembly.transforms.len())
        .with("filesystem_count", assembly.filesystems.len())
        .with("disk_count", assembly.disks.len())
        .with("busybox_initramfs_count", assembly.busybox_initramfs.len())
        .with(
            "work_dir.placement",
            if view.ram.is_some() { "ram" } else { "disk" },
        );
    if let Some(ram) = view.ram {
        state.insert("work_dir.path", ram.root.display().to_string());
        state.insert("work_dir.expected_bytes", ram.expected_bytes);
    }

    for tree in &assembly.trees {
        let span = tracing::info_span!(
            "assembly_tree_prepare",
            operation_id = %operation_id.as_str(),
            tree_id = %tree.id,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let path = roots.tree_path(&tree.id)?;
        tracing::Span::current().record("output_path", path.display().to_string());
        if path.exists() {
            gaia_process::discard(path).map_err(|error| {
                format!(
                    "failed to clean assembly tree '{}' at '{}': {error}",
                    tree.id,
                    path.display()
                )
            })?;
        }
        std_fs::create_dir_all(path).map_err(|error| {
            format!(
                "failed to create assembly tree '{}' at '{}': {error}",
                tree.id,
                path.display()
            )
        })?;
        state.insert(format!("tree.{}.path", tree.id), path.display().to_string());
        messages.push(format!(
            "prepared assembly tree '{}' at '{}'",
            tree.id,
            path.display()
        ));
    }

    // Every step runs after the steps whose outputs it reads, so for
    // example a transform compressing a filesystem image sees this run's
    // image. Outputs from an earlier run are removed first: a dependency
    // Gaia cannot see fails loudly instead of reading a stale file.
    let (order, step_paths) = ordered_assembly_steps(spec, assembly, roots)?;
    remove_stale_step_outputs(&step_paths)?;
    let mut run = StepRun::new(
        spec,
        assembly,
        roots,
        operation_id,
        cancel_check.clone(),
        state,
        messages,
    );
    if let Some(ram) = view.ram {
        run.disk_publish = ram.disk_publish.clone();
    }
    for step in order {
        run.run(step)?;
    }
    run.finish();
    let StepRun {
        mut state,
        mut messages,
        disk_outputs,
        disk_published,
        ..
    } = run;

    let mut cleanup_paths = context.cleanup_paths();
    let mut archive_path = None;
    if let Some(summary) =
        archive_assembly_disk_output(spec, &disk_outputs, cancel_check.clone(), &mut messages)?
    {
        state.insert("archive.path", summary.output.display().to_string());
        state.insert("archive.source", summary.source.display().to_string());
        state.insert("archive.bytes", summary.bytes);
        state.insert("archive.sha256", summary.sha256);
        archive_path = Some(summary.output.clone());
        cleanup_paths.push(summary.output.clone());
        messages.push(format!(
            "published assembly disk '{}' as '{}'",
            summary.source.display(),
            summary.output.display()
        ));
    } else if let [disk] = disk_published.as_slice() {
        // Without a raw archive, the single assembled disk is still the
        // deliverable: report it as the primary image output rather than
        // letting an intermediate rootfs image stand in for it.
        archive_path = Some(disk.clone());
    }
    state.insert("cleanup_path_count", cleanup_paths.len());
    for (index, path) in cleanup_paths.iter().enumerate() {
        state.insert(
            format!("cleanup_path.{}", index + 1),
            path.display().to_string(),
        );
    }

    Ok(AssemblyStagingSummary {
        state,
        messages,
        cleanup_paths,
        archive_path,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssemblyArchiveSummary {
    output: PathBuf,
    source: PathBuf,
    bytes: u64,
    sha256: String,
}

fn archive_assembly_disk_output(
    spec: &ResolvedBuildSpec,
    disk_outputs: &[PathBuf],
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    messages: &mut Vec<String>,
) -> Result<Option<AssemblyArchiveSummary>, AssemblyError> {
    let Some(archive_path) = assembly_archive_path(spec) else {
        return Ok(None);
    };
    let Some(kind) = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(gaia_spec::raw_disk_archive)
    else {
        return Ok(None);
    };
    if disk_outputs.is_empty() {
        return Ok(None);
    }
    if disk_outputs.len() != 1 {
        return Err(AssemblyError::runtime(format!(
            "image.output.archive_name '{}' requests a raw disk image, but assembly produced {} disk outputs; configure a single assembly disk or use a tar archive",
            archive_path.display(),
            disk_outputs.len()
        )));
    }
    let source = &disk_outputs[0];
    let temp_archive = temporary_assembly_output_path(&archive_path);
    let Some((compressor, args)) = kind.compressor(0) else {
        // An uncompressed `.img`/`.raw` archive is the assembled disk itself.
        copy_sparse(source, &temp_archive).map_err(|error| {
            let _ = std_fs::remove_file(&temp_archive);
            AssemblyError::runtime(format!(
                "failed to copy assembly disk '{}' to '{}': {error}",
                source.display(),
                archive_path.display()
            ))
        })?;
        publish_assembly_output(&temp_archive, &archive_path)?;
        return Ok(Some(AssemblyArchiveSummary {
            output: archive_path.clone(),
            source: source.clone(),
            bytes: file_len(&archive_path)?,
            sha256: file_sha256(&archive_path)?,
        }));
    };
    // All cores; the output is the same for any thread count.
    let mut command = Command::new(compressor);
    command.args(args).arg(source);
    let clock = gaia_process::ActiveClock::start();
    let output = run_command_stdout_to_file(
        spec,
        &mut command,
        &temp_archive,
        process_output_retention(spec),
        cancel_check,
    )
    .inspect_err(|_| {
        let _ = std_fs::remove_file(&temp_archive);
    })?;
    if !output.status.success() {
        let _ = std_fs::remove_file(&temp_archive);
        return Err(AssemblyError::runtime(format!(
            "failed to compress assembly disk '{}' to '{}': {}",
            source.display(),
            archive_path.display(),
            output.stderr_tail()
        )));
    }
    publish_assembly_output(&temp_archive, &archive_path)?;
    messages.extend(archive_log_messages(
        &archive_path,
        compressor,
        std_fs::metadata(source).ok().map(|metadata| metadata.len()),
        clock.elapsed(),
    ));
    Ok(Some(AssemblyArchiveSummary {
        output: archive_path.clone(),
        source: source.clone(),
        bytes: file_len(&archive_path)?,
        sha256: file_sha256(&archive_path)?,
    }))
}

fn assembly_archive_path(spec: &ResolvedBuildSpec) -> Option<PathBuf> {
    let collect_dir = spec.image.output.collect_dir.as_ref()?;
    let archive_name = spec.image.output.archive_name.as_ref()?;
    Some(PathBuf::from(collect_dir).join(archive_name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssemblyStateKey<'a> {
    section: &'a str,
    index: usize,
}

impl<'a> AssemblyStateKey<'a> {
    fn new(section: &'a str, index: usize) -> Self {
        Self { section, index }
    }

    fn field(self, field: &str) -> String {
        format!("{}.{}.{}", self.section, self.index, field)
    }

    fn child_field(self, child: &str, child_index: usize, field: &str) -> String {
        format!(
            "{}.{}.{}.{}.{}",
            self.section, self.index, child, child_index, field
        )
    }
}

fn temporary_assembly_output_path(output: &Path) -> PathBuf {
    gaia_image_providers::temporary_publish_output_path(output, "assembly-output")
}

fn publish_assembly_output(temp: &Path, output: &Path) -> Result<(), String> {
    gaia_image_providers::publish_replace_output(
        temp,
        output,
        "assembly output",
        "assembly-output",
    )?;
    // Record the output's digest in its directory's content manifest so the
    // next plan does not read it again. Best effort: a missing entry only
    // costs one re-hash.
    if let (true, Some(parent)) = (output.is_file(), output.parent()) {
        let _ = gaia_image_providers::record_content_digests(parent, &[output.to_path_buf()]);
    }
    Ok(())
}

fn temporary_assembly_backup_path(output: &Path) -> PathBuf {
    gaia_image_providers::temporary_publish_backup_path(output, "assembly-output")
}

#[derive(Debug)]
struct CommandFileOutput {
    pub(super) status: ExitStatus,
    pub(super) stderr: Vec<u8>,
}

impl CommandFileOutput {
    fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }

    fn failure_context(&self, command: &Command) -> String {
        format!(
            "command `{}` exited with status {}; stderr tail: {}",
            command_display(command),
            self.status,
            self.stderr_tail()
        )
    }
}

#[derive(Debug)]
struct CommandCapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CommandCapturedOutput {
    fn stdout_tail(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }

    fn failure_context(&self, command: &Command) -> String {
        format!(
            "command `{}` exited with status {}; stdout tail: {}; stderr tail: {}",
            command_display(command),
            self.status,
            self.stdout_tail(),
            self.stderr_tail()
        )
    }
}

fn run_command_stdout_to_file(
    spec: &ResolvedBuildSpec,
    command: &mut Command,
    output: &Path,
    retention: gaia_process::ProcessOutputRetention,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<CommandFileOutput, AssemblyError> {
    let result = gaia_process::run_command_stdout_to_file_with_timeout_and_retention(
        command,
        output,
        assembly_command_timeout(spec),
        "assembly command",
        retention,
        None,
        cancel_check,
    )
    .map_err(|error| AssemblyError::process(command, error))?;
    Ok(CommandFileOutput {
        status: result.output.status,
        stderr: result.output.stderr,
    })
}

fn run_command_capture_tail(
    spec: &ResolvedBuildSpec,
    command: &mut Command,
    retention: gaia_process::ProcessOutputRetention,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<CommandCapturedOutput, AssemblyError> {
    let result = gaia_process::run_command_with_timeout_and_retention(
        command,
        assembly_command_timeout(spec),
        "assembly command",
        retention,
        None,
        cancel_check,
    )
    .map_err(|error| AssemblyError::process(command, error))?;
    Ok(CommandCapturedOutput {
        status: result.output.status,
        stdout: result.output.stdout,
        stderr: result.output.stderr,
    })
}

fn assembly_command_timeout(spec: &ResolvedBuildSpec) -> Duration {
    Duration::from_secs(
        spec.policy
            .providers
            .image_command_policy(spec.image.provider_kind())
            .timeout_seconds
            .max(1),
    )
}

fn command_display(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn read_tail_bytes(mut reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if limit == 0 {
            continue;
        }
        if read >= limit {
            retained.clear();
            retained.extend_from_slice(&buffer[read - limit..read]);
            continue;
        }
        let overflow = retained.len().saturating_add(read).saturating_sub(limit);
        if overflow > 0 {
            retained.drain(0..overflow);
        }
        retained.extend_from_slice(&buffer[..read]);
    }
    Ok(retained)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedTool {
    pub(super) program: PathBuf,
    pub(super) display: String,
}

fn resolve_assembly_tool(roots: &AssemblyRoots, name: &str) -> Result<ResolvedTool, String> {
    if let Some(provider_host) = &roots.provider_host {
        for relative in [
            PathBuf::from("bin").join(name),
            PathBuf::from("usr/bin").join(name),
            PathBuf::from(name),
        ] {
            let candidate = provider_host.join(relative);
            if candidate.is_file() {
                return Ok(ResolvedTool {
                    display: candidate.display().to_string(),
                    program: candidate,
                });
            }
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(ResolvedTool {
                    display: candidate.display().to_string(),
                    program: candidate,
                });
            }
        }
    }

    Err(format!(
        "assembly tool '{name}' was not found in provider host tools or host PATH"
    ))
}

fn tool_version<const N: usize>(tool: &ResolvedTool, args: [&str; N]) -> Option<String> {
    let mut command = Command::new(&tool.program);
    command.args(args);
    let result = gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        Duration::from_secs(TOOL_VERSION_TIMEOUT_SECONDS),
        "assembly tool version",
        gaia_process::ProcessOutputRetention {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            stdout_lines: 4,
            stderr_lines: 4,
        },
        None,
        None,
    )
    .ok()?;
    if !result.output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&result.output.stdout);
    let stderr = String::from_utf8_lossy(&result.output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .next()
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
