use super::*;

pub(super) struct AssemblyFilesystemSummary {
    pub(super) output: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) tool_path: Option<String>,
    pub(super) tool_version: Option<String>,
}

pub(super) fn execute_assembly_filesystem(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    filesystem: &gaia_spec::AssemblyFilesystemSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyFilesystemSummary, AssemblyError> {
    let source_tree = roots.tree_path(&filesystem.source_tree)?;
    if !source_tree.is_dir() {
        return Err(format!(
            "assembly filesystem '{}' source tree '{}' does not exist",
            filesystem.id,
            source_tree.display()
        )
        .into());
    }
    let output = roots.resolve_path(spec, &filesystem.output)?;
    if let Some(parent) = output.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly filesystem output dir '{}': {error}",
                parent.display()
            )
        })?;
    }

    match filesystem.kind {
        gaia_spec::AssemblyFilesystemKindSpec::Cpio => {
            let temp = temporary_assembly_output_path(&output);
            write_newc_archive(source_tree, &temp)?;
            publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256: file_sha256(&output)?,
                output,
                tool_path: None,
                tool_version: None,
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::CpioGzip => {
            let tool = resolve_assembly_tool(roots, "gzip")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&output);
            let temp_cpio = temp.with_extension("cpio.tmp");
            write_newc_archive(source_tree, &temp_cpio)?;
            let mut command = Command::new(&tool.program);
            command.arg("-n").arg("-c").arg(&temp_cpio);
            let gzip_output = run_command_stdout_to_file(
                spec,
                &mut command,
                &temp,
                process_output_retention(spec),
                cancel_check,
            )?;
            let _ = std_fs::remove_file(&temp_cpio);
            if !gzip_output.status.success() {
                return Err(format!(
                    "cpio-gzip compressor failed for '{}' using '{}': {}",
                    source_tree.display(),
                    tool.display,
                    gzip_output.failure_context(&command)
                )
                .into());
            }
            publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256: file_sha256(&output)?,
                output,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::Vfat => {
            let mformat = resolve_assembly_tool(roots, "mformat")?;
            let mcopy = resolve_assembly_tool(roots, "mcopy")?;
            let tool_path = format!("mformat={};mcopy={}", mformat.display, mcopy.display);
            tracing::Span::current().record("tool_path", tool_path.as_str());
            let bytes = filesystem
                .parsed_size()
                .map_err(|error| error.to_string())?
                .unwrap_or(gaia_spec::ByteSize::from_bytes(32 * 1024 * 1024))
                .bytes();
            let temp = temporary_assembly_output_path(&output);
            write_vfat_filesystem(VfatWriteContext {
                spec,
                source_tree,
                output: &temp,
                bytes,
                mformat: &mformat,
                mcopy: &mcopy,
                retention: process_output_retention(spec),
                cancel_check,
            })?;
            publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256: file_sha256(&output)?,
                output,
                tool_path: Some(tool_path),
                tool_version: tool_version(&mformat, ["--version"]),
            })
        }
    }
}

struct VfatWriteContext<'a> {
    spec: &'a ResolvedBuildSpec,
    source_tree: &'a Path,
    output: &'a Path,
    bytes: u64,
    mformat: &'a ResolvedTool,
    mcopy: &'a ResolvedTool,
    retention: gaia_process::ProcessOutputRetention,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
}

fn write_vfat_filesystem(context: VfatWriteContext<'_>) -> Result<(), AssemblyError> {
    let VfatWriteContext {
        spec,
        source_tree,
        output,
        bytes,
        mformat,
        mcopy,
        retention,
        cancel_check,
    } = context;
    let image = std_fs::File::create(output).map_err(|error| {
        format!(
            "failed to create vfat image '{}' before formatting: {error}",
            output.display()
        )
    })?;
    image.set_len(bytes).map_err(|error| {
        format!(
            "failed to size vfat image '{}' to {bytes} bytes: {error}",
            output.display()
        )
    })?;
    drop(image);

    let mut format_command = Command::new(&mformat.program);
    format_command.arg("-i").arg(output).arg("-F").arg("::");
    let format_output =
        run_command_capture_tail(spec, &mut format_command, retention, cancel_check.clone())?;
    if !format_output.status.success() {
        return Err(format!(
            "vfat formatter failed for '{}' using '{}': {}",
            output.display(),
            mformat.display,
            format_output.failure_context(&format_command)
        )
        .into());
    }

    let mut children = std_fs::read_dir(source_tree)
        .map_err(|error| {
            format!(
                "failed to read vfat source tree '{}': {error}",
                source_tree.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "failed to read vfat source tree '{}': {error}",
                source_tree.display()
            )
        })?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let mut copy_command = Command::new(&mcopy.program);
        copy_command
            .arg("-i")
            .arg(output)
            .arg("-s")
            .arg(child.path())
            .arg("::");
        let copy_output =
            run_command_capture_tail(spec, &mut copy_command, retention, cancel_check.clone())?;
        if !copy_output.status.success() {
            return Err(format!(
                "vfat copy failed for '{}' into '{}' using '{}': {}",
                child.path().display(),
                output.display(),
                mcopy.display,
                copy_output.failure_context(&copy_command)
            )
            .into());
        }
    }
    Ok(())
}

fn write_newc_archive(source_tree: &Path, output: &Path) -> Result<(), String> {
    let mut entries = collect_archive_entries(source_tree)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut file = std_fs::File::create(output).map_err(|error| {
        format!(
            "failed to create cpio archive '{}': {error}",
            output.display()
        )
    })?;
    let mut ino = 1u32;
    for (name, path) in entries {
        write_newc_entry(&mut file, source_tree, &name, &path, ino)?;
        ino = ino.saturating_add(1);
    }
    write_newc_record_bytes(&mut file, "TRAILER!!!", 0, 0o100644, &[])?;
    Ok(())
}

fn collect_archive_entries(source_tree: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut entries = Vec::new();
    collect_archive_entries_inner(source_tree, source_tree, &mut entries)?;
    Ok(entries)
}

fn collect_archive_entries_inner(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let mut children = std_fs::read_dir(dir)
        .map_err(|error| format!("failed to read assembly tree '{}': {error}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read assembly tree '{}': {error}", dir.display()))?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let path = child.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            format!(
                "failed to derive archive path for '{}' under '{}': {error}",
                path.display(),
                root.display()
            )
        })?;
        let name = relative.to_string_lossy().replace('\\', "/");
        entries.push((name, path.clone()));
        if child
            .file_type()
            .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?
            .is_dir()
        {
            collect_archive_entries_inner(root, &path, entries)?;
        }
    }
    Ok(())
}

fn write_newc_entry(
    file: &mut std_fs::File,
    source_tree: &Path,
    name: &str,
    path: &Path,
    ino: u32,
) -> Result<(), String> {
    let metadata = std_fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect assembly archive entry '{}': {error}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let permissions = 0o644;
    if metadata.file_type().is_dir() {
        return write_newc_record_bytes(file, name, ino, 0o040000 | permissions.max(0o755), &[]);
    }
    if metadata.file_type().is_symlink() {
        let target = std_fs::read_link(path).map_err(|error| {
            format!(
                "failed to read assembly archive symlink '{}': {error}",
                path.display()
            )
        })?;
        return write_newc_record_bytes(
            file,
            name,
            ino,
            0o120000 | 0o777,
            target.to_string_lossy().as_bytes(),
        );
    }
    if metadata.is_file() {
        let file_size = u32::try_from(metadata.len()).map_err(|_| {
            format!(
                "assembly archive file '{}' exceeds the cpio newc size limit",
                path.display()
            )
        })?;
        let mode = 0o100000 | permissions.max(0o644);
        write_newc_record_header(file, name, ino, file_size, mode)?;
        let mut source = std_fs::File::open(path).map_err(|error| {
            format!(
                "failed to open assembly archive file '{}': {error}",
                path.display()
            )
        })?;
        let copied = std::io::copy(&mut source, file).map_err(|error| {
            format!(
                "failed to stream assembly archive file '{}': {error}",
                path.display()
            )
        })?;
        if copied != metadata.len() {
            return Err(format!(
                "assembly archive file '{}' changed while it was being archived",
                path.display()
            ));
        }
        return pad_newc(file, file_size as usize);
    }
    Err(format!(
        "assembly archive entry '{}' under '{}' is not supported",
        path.display(),
        source_tree.display()
    ))
}

fn write_newc_record_bytes(
    file: &mut std_fs::File,
    name: &str,
    ino: u32,
    mode: u32,
    data: &[u8],
) -> Result<(), String> {
    let file_size = u32::try_from(data.len())
        .map_err(|_| format!("assembly archive entry '{name}' exceeds the cpio newc size limit"))?;
    write_newc_record_header(file, name, ino, file_size, mode)?;
    file.write_all(data)
        .map_err(|error| format!("failed to write cpio data: {error}"))?;
    pad_newc(file, data.len())
}

fn write_newc_record_header(
    file: &mut std_fs::File,
    name: &str,
    ino: u32,
    file_size: u32,
    mode: u32,
) -> Result<(), String> {
    let name_size = name.len() + 1;
    write!(
        file,
        "070701{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{file_size:08x}{dev_major:08x}{dev_minor:08x}{rdev_major:08x}{rdev_minor:08x}{name_size:08x}{check:08x}",
        uid = 0,
        gid = 0,
        nlink = 1,
        mtime = 0,
        dev_major = 0,
        dev_minor = 0,
        rdev_major = 0,
        rdev_minor = 0,
        check = 0,
    )
    .map_err(|error| format!("failed to write cpio header: {error}"))?;
    file.write_all(name.as_bytes())
        .map_err(|error| format!("failed to write cpio name: {error}"))?;
    file.write_all(&[0])
        .map_err(|error| format!("failed to write cpio name terminator: {error}"))?;
    pad_newc(file, 110 + name_size)
}

fn pad_newc(file: &mut std_fs::File, written: usize) -> Result<(), String> {
    const PADDING: [u8; 3] = [0; 3];
    let padding = (4 - (written % 4)) % 4;
    if padding > 0 {
        file.write_all(&PADDING[..padding])
            .map_err(|error| format!("failed to write cpio padding: {error}"))?;
    }
    Ok(())
}
