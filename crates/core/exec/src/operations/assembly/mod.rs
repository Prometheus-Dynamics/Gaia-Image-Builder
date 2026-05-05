use super::*;
use gaia_spec::AssemblyRoots;
use gaia_spec::KeyValueState;
use sha2::{Digest, Sha256};
use std::fs as std_fs;
#[cfg(test)]
use std::io::Read;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

use super::helpers::{process_output_retention, runtime_state_dir};

mod busybox;
mod disks;
mod files;
mod filesystems;
mod state;
mod transforms;

use busybox::*;
use disks::*;
use files::*;
use filesystems::*;
use state::AssemblyExecutionContext;
pub(crate) use state::{assembly_state_path, image_assembly_cleanup_paths};
use transforms::*;

const TOOL_VERSION_TIMEOUT_SECONDS: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssemblyStagingSummary {
    pub state: KeyValueState,
    pub messages: Vec<String>,
    pub cleanup_paths: Vec<PathBuf>,
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
    let Some(assembly) = &spec.image.assembly else {
        return Ok(AssemblyStagingSummary {
            state: KeyValueState::new().with("kind", gaia_spec::IMAGE_ASSEMBLY_STATE_KIND),
            messages: vec!["image assembly has no configured actions".into()],
            cleanup_paths: Vec::new(),
        });
    };

    tracing::Span::current().record("tree_count", assembly.trees.len());
    tracing::Span::current().record("dir_count", assembly.dirs.len());
    tracing::Span::current().record("symlink_count", assembly.symlinks.len());
    tracing::Span::current().record("file_entry_count", assembly.files.len());
    tracing::Span::current().record("transform_count", assembly.transforms.len());
    tracing::Span::current().record("filesystem_count", assembly.filesystems.len());
    tracing::Span::current().record("disk_count", assembly.disks.len());

    let roots = AssemblyRoots::new(spec, assembly)?;
    let context = AssemblyExecutionContext::new(spec, assembly, &roots);
    let mut state = KeyValueState::new()
        .with("kind", gaia_spec::IMAGE_ASSEMBLY_STATE_KIND)
        .with("tree_count", assembly.trees.len())
        .with("dir_count", assembly.dirs.len())
        .with("symlink_count", assembly.symlinks.len())
        .with("file_entry_count", assembly.files.len())
        .with("transform_count", assembly.transforms.len())
        .with("filesystem_count", assembly.filesystems.len())
        .with("disk_count", assembly.disks.len())
        .with("busybox_initramfs_count", assembly.busybox_initramfs.len());
    let mut messages = Vec::new();

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
            std_fs::remove_dir_all(path).map_err(|error| {
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

    let mut dir_count = 0usize;
    for dir in &assembly.dirs {
        let tree_path = roots.tree_path(&dir.tree)?;
        let dest = create_assembly_dir(tree_path, dir)?;
        dir_count += 1;
        state.insert(format!("dir.{dir_count}.tree"), dir.tree.as_str());
        state.insert(format!("dir.{dir_count}.path"), dest.display().to_string());
        if let Some(mode) = &dir.mode {
            state.insert(format!("dir.{dir_count}.mode"), mode);
        }
    }
    state.insert("created_dir_count", dir_count);
    if dir_count > 0 {
        messages.push(format!("created {dir_count} assembly dir(s)"));
    }

    let mut symlink_count = 0usize;
    for symlink in &assembly.symlinks {
        let tree_path = roots.tree_path(&symlink.tree)?;
        let dest = create_assembly_symlink(tree_path, symlink)?;
        symlink_count += 1;
        state.insert(
            format!("symlink.{symlink_count}.tree"),
            symlink.tree.as_str(),
        );
        state.insert(
            format!("symlink.{symlink_count}.path"),
            dest.display().to_string(),
        );
        state.insert(format!("symlink.{symlink_count}.target"), &symlink.target);
    }
    state.insert("created_symlink_count", symlink_count);
    if symlink_count > 0 {
        messages.push(format!("created {symlink_count} assembly symlink(s)"));
    }

    let mut staged_count = 0usize;
    let mut skipped_count = 0usize;
    for (entry_index, file) in assembly.files.iter().enumerate() {
        let span = tracing::info_span!(
            "assembly_file_stage",
            operation_id = %operation_id.as_str(),
            entry_index,
            tree_id = %file.tree,
            dest = %file.dest,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let tree_path = roots.tree_path(&file.tree)?;
        let sources = assembly_file_sources(spec, &roots, file)?;
        if sources.is_empty() && file.optional {
            skipped_count += 1;
            state.insert(format!("file.{entry_index}.skipped"), "true");
            state.insert(format!("file.{entry_index}.tree"), &file.tree);
            continue;
        }
        if sources.is_empty() {
            return Err(format!(
                "assembly file entry for tree '{}' matched no sources",
                file.tree
            )
            .into());
        }

        for source in sources {
            if !source.exists() {
                if file.optional {
                    skipped_count += 1;
                    state.insert(format!("file.{entry_index}.skipped"), source.display());
                    continue;
                }
                return Err(
                    format!("assembly source '{}' does not exist", source.display()).into(),
                );
            }
            let dest = assembly_file_dest(tree_path, &source, &file.dest)?;
            tracing::Span::current().record("output_path", dest.display().to_string());
            copy_assembly_file(&source, &dest, file)?;
            staged_count += 1;
            state.insert(
                format!("file.{staged_count}.src"),
                source.display().to_string(),
            );
            state.insert(
                format!("file.{staged_count}.dest"),
                dest.display().to_string(),
            );
            state.insert(format!("file.{staged_count}.bytes"), file_len(&dest)?);
            state.insert(format!("file.{staged_count}.sha256"), file_sha256(&dest)?);
            if let Some(mode) = &file.mode {
                state.insert(format!("file.{staged_count}.mode"), mode);
            }
        }
    }
    state.insert("staged_file_count", staged_count);
    state.insert("skipped_file_count", skipped_count);
    messages.push(format!(
        "staged {staged_count} assembly file(s), skipped {skipped_count}"
    ));

    let mut busybox_count = 0usize;
    for initramfs in &assembly.busybox_initramfs {
        let span = tracing::info_span!(
            "assembly_busybox_initramfs",
            operation_id = %operation_id.as_str(),
            tree_id = %initramfs.tree,
            busybox = %initramfs.busybox,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_busybox_initramfs(spec, &roots, initramfs, cancel_check.clone())?;
        tracing::Span::current().record("output_path", summary.dest.display().to_string());
        busybox_count += 1;
        state.insert(
            format!("busybox.{busybox_count}.tree"),
            initramfs.tree.as_str(),
        );
        state.insert(
            format!("busybox.{busybox_count}.src"),
            summary.src.display().to_string(),
        );
        state.insert(
            format!("busybox.{busybox_count}.dest"),
            summary.dest.display().to_string(),
        );
        state.insert(format!("busybox.{busybox_count}.bytes"), summary.bytes);
        state.insert(format!("busybox.{busybox_count}.sha256"), summary.sha256);
        state.insert(
            format!("busybox.{busybox_count}.applet_count"),
            summary.applets.len(),
        );
        for (applet_index, applet) in summary.applets.iter().enumerate() {
            let index = applet_index + 1;
            state.insert(format!("busybox.{busybox_count}.applet.{index}"), applet);
        }
        state.insert(
            format!("busybox.{busybox_count}.runtime_linkage"),
            summary.runtime_linkage.as_str(),
        );
        state.insert(
            format!("busybox.{busybox_count}.runtime_library_count"),
            summary.runtime_libraries.len(),
        );
        for (library_index, library) in summary.runtime_libraries.iter().enumerate() {
            let index = library_index + 1;
            state.insert(
                format!("busybox.{busybox_count}.runtime_library.{index}"),
                library.display().to_string(),
            );
        }
        messages.push(format!(
            "prepared busybox initramfs tree '{}' with {} applet(s)",
            initramfs.tree,
            summary.applets.len()
        ));
    }
    state.insert("completed_busybox_initramfs_count", busybox_count);

    let mut transform_count = 0usize;
    for transform in &assembly.transforms {
        let span = tracing::info_span!(
            "assembly_transform",
            operation_id = %operation_id.as_str(),
            kind = transform.kind.as_str(),
            dest = %transform.dest,
            output_path = tracing::field::Empty,
            tool_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_assembly_transform(spec, &roots, transform, cancel_check.clone())?;
        tracing::Span::current().record("output_path", summary.dest.display().to_string());
        transform_count += 1;
        let transform_state = AssemblyStateKey::new("transform", transform_count);
        state.insert(transform_state.field("kind"), transform.kind.as_str());
        state.insert(
            transform_state.field("src"),
            summary.src.display().to_string(),
        );
        state.insert(
            transform_state.field("dest"),
            summary.dest.display().to_string(),
        );
        state.insert(
            transform_state.field("deterministic"),
            transform.deterministic,
        );
        state.insert(transform_state.field("bytes"), summary.bytes);
        state.insert(transform_state.field("sha256"), summary.sha256);
        if let Some(tool) = summary.tool_path {
            state.insert(transform_state.field("tool"), tool);
        }
        if let Some(tool_version) = summary.tool_version {
            state.insert(transform_state.field("tool_version"), tool_version);
        }
        messages.push(format!(
            "ran assembly transform '{}' to '{}'",
            transform.kind.as_str(),
            summary.dest.display()
        ));
    }
    state.insert("completed_transform_count", transform_count);

    let mut filesystem_count = 0usize;
    for filesystem in &assembly.filesystems {
        let span = tracing::info_span!(
            "assembly_filesystem",
            operation_id = %operation_id.as_str(),
            filesystem_id = %filesystem.id,
            kind = filesystem.kind.as_str(),
            source_tree = %filesystem.source_tree,
            output = %filesystem.output,
            output_path = tracing::field::Empty,
            tool_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_assembly_filesystem(spec, &roots, filesystem, cancel_check.clone())?;
        tracing::Span::current().record("output_path", summary.output.display().to_string());
        filesystem_count += 1;
        let filesystem_state = AssemblyStateKey::new("filesystem", filesystem_count);
        state.insert(filesystem_state.field("id"), filesystem.id.as_str());
        state.insert(filesystem_state.field("kind"), filesystem.kind.as_str());
        state.insert(
            filesystem_state.field("source_tree"),
            filesystem.source_tree.as_str(),
        );
        state.insert(
            filesystem_state.field("output"),
            summary.output.display().to_string(),
        );
        state.insert(
            filesystem_state.field("deterministic"),
            filesystem.deterministic,
        );
        state.insert(filesystem_state.field("bytes"), summary.bytes);
        state.insert(filesystem_state.field("sha256"), summary.sha256);
        if let Some(tool) = summary.tool_path {
            state.insert(filesystem_state.field("tool"), tool);
        }
        if let Some(tool_version) = summary.tool_version {
            state.insert(filesystem_state.field("tool_version"), tool_version);
        }
        messages.push(format!(
            "built assembly filesystem '{}' at '{}'",
            filesystem.id,
            summary.output.display()
        ));
    }
    state.insert("completed_filesystem_count", filesystem_count);

    let mut disk_count = 0usize;
    let mut disk_outputs = Vec::new();
    for disk in &assembly.disks {
        let span = tracing::info_span!(
            "assembly_disk",
            operation_id = %operation_id.as_str(),
            disk_id = %disk.id,
            partition_table = disk.partition_table.as_str(),
            output = %disk.output,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_assembly_disk(spec, &roots, disk)?;
        tracing::Span::current().record("output_path", summary.output.display().to_string());
        disk_count += 1;
        disk_outputs.push(summary.output.clone());
        let disk_state = AssemblyStateKey::new("disk", disk_count);
        state.insert(disk_state.field("id"), disk.id.as_str());
        state.insert(
            disk_state.field("partition_table"),
            disk.partition_table.as_str(),
        );
        state.insert(
            disk_state.field("output"),
            summary.output.display().to_string(),
        );
        state.insert(disk_state.field("bytes"), summary.bytes);
        state.insert(disk_state.field("sha256"), summary.sha256);
        state.insert(
            disk_state.field("partition_count"),
            summary.partitions.len(),
        );
        for (partition_index, partition) in summary.partitions.iter().enumerate() {
            let index = partition_index + 1;
            state.insert(
                disk_state.child_field("partition", index, "name"),
                &partition.name,
            );
            state.insert(
                disk_state.child_field("partition", index, "type"),
                format!("0x{:02X}", partition.partition_type),
            );
            state.insert(
                disk_state.child_field("partition", index, "image"),
                partition.image.display().to_string(),
            );
            state.insert(
                disk_state.child_field("partition", index, "start_lba"),
                partition.start_lba,
            );
            state.insert(
                disk_state.child_field("partition", index, "sector_count"),
                partition.sector_count,
            );
            state.insert(
                disk_state.child_field("partition", index, "bytes"),
                partition.bytes,
            );
        }
        messages.push(format!(
            "built assembly disk '{}' at '{}'",
            disk.id,
            summary.output.display()
        ));
    }
    state.insert("completed_disk_count", disk_count);

    let mut cleanup_paths = context.cleanup_paths();
    if let Some(summary) = archive_assembly_disk_output(spec, &disk_outputs, cancel_check.clone())?
    {
        state.insert("archive.path", summary.output.display().to_string());
        state.insert("archive.source", summary.source.display().to_string());
        state.insert("archive.bytes", summary.bytes);
        state.insert("archive.sha256", summary.sha256);
        cleanup_paths.push(summary.output.clone());
        messages.push(format!(
            "compressed assembly disk '{}' to '{}'",
            summary.source.display(),
            summary.output.display()
        ));
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
) -> Result<Option<AssemblyArchiveSummary>, AssemblyError> {
    let Some(archive_path) = assembly_archive_path(spec) else {
        return Ok(None);
    };
    if !raw_xz_archive_path(&archive_path) {
        return Ok(None);
    }
    if disk_outputs.is_empty() {
        return Ok(None);
    }
    if disk_outputs.len() != 1 {
        return Err(AssemblyError::runtime(format!(
            "image.output.archive_name '{}' requests a raw compressed image, but assembly produced {} disk outputs; configure a single assembly disk or use a tar archive",
            archive_path.display(),
            disk_outputs.len()
        )));
    }
    let source = &disk_outputs[0];
    let temp_archive = temporary_assembly_output_path(&archive_path);
    let mut command = Command::new("xz");
    command.arg("-T1").arg("-c").arg(source);
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

fn raw_xz_archive_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".img.xz") || name.ends_with(".raw.xz"))
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
    gaia_image_providers::publish_replace_output(temp, output, "assembly output", "assembly-output")
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
