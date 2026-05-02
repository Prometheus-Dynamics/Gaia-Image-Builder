use super::*;

pub(crate) struct BuildrootArchiveRequest<'a> {
    pub(crate) image: &'a ImageSpec,
    pub(crate) collect_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) matched_expected_images: &'a [String],
    pub(crate) archive_path: &'a Path,
    pub(crate) reuse_details: &'a mut Vec<String>,
    pub(crate) command: ImageCommandContext<'a>,
}

pub(crate) fn archive_buildroot_output(
    request: BuildrootArchiveRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootArchiveRequest {
        image,
        collect_dir,
        output_dir,
        matched_expected_images,
        archive_path,
        reuse_details,
        command: command_context,
    } = request;
    let entries = archive_entries_for_buildroot_archive(image, matched_expected_images);
    if let Some(tar_mode) = tar_archive_mode(archive_path) {
        if archive_signature_is_current(collect_dir, &entries, archive_path, tar_mode) {
            reuse_details.push("image-archive".to_string());
            return Ok(vec![format!(
                "reused image archive '{}' for unchanged entries: {}",
                archive_path.display(),
                entries.join(",")
            )]);
        }
        return archive_files(ArchiveFilesRequest {
            source_dir: collect_dir,
            entries: &entries,
            archive_path,
            mode: tar_mode,
            label: "buildroot expected image archive",
            command: command_context,
        });
    }
    if entries.len() == 1 {
        let source_path = collect_dir.join(&entries[0]);
        if source_path.is_file() {
            if raw_xz_archive_path(archive_path) {
                return compress_primary_image(
                    &source_path,
                    archive_path,
                    command_context.execution,
                    command_context.policy,
                    command_context.log_sink,
                    command_context.cancel_check,
                );
            }
            if let Some(parent) = archive_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    ImageProviderError::backend_command(format!(
                        "failed to create archive dir '{}': {error}",
                        parent.display()
                    ))
                })?;
            }
            fs::copy(&source_path, archive_path).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to copy primary buildroot image '{}' to '{}': {error}",
                        source_path.display(),
                        archive_path.display()
                    ),
                )
            })?;
            return Ok(vec![format!(
                "copied primary buildroot image '{}' to '{}'",
                source_path.display(),
                archive_path.display()
            )]);
        }
    }
    archive_directory(
        output_dir,
        archive_path,
        "buildroot archive",
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )
}

#[derive(Clone, Copy)]
pub(crate) enum TarArchiveMode {
    Plain,
    Xz,
}

impl TarArchiveMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "tar",
            Self::Xz => "tar.xz",
        }
    }
}

impl TarArchiveMode {
    fn create_arg(self) -> &'static str {
        match self {
            Self::Plain => "-cf",
            Self::Xz => "-cJf",
        }
    }
}

pub(crate) fn tar_archive_mode(path: &Path) -> Option<TarArchiveMode> {
    let name = path.file_name().and_then(|name| name.to_str())?;
    if name.ends_with(".tar") {
        Some(TarArchiveMode::Plain)
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Some(TarArchiveMode::Xz)
    } else {
        None
    }
}

pub(crate) fn archive_entries_for_buildroot_archive(
    image: &ImageSpec,
    matched_expected_images: &[String],
) -> Vec<String> {
    let ImageDefinition::Buildroot(buildroot) = &image.definition else {
        return matched_expected_images.to_vec();
    };
    let raw_expected = buildroot
        .expected_images
        .iter()
        .filter(|expected| expected.format == BuildrootExpectedImageFormatSpec::Raw)
        .map(|expected| expected.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let raw_matches = matched_expected_images
        .iter()
        .filter(|matched| raw_expected.contains(matched.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if raw_matches.is_empty() {
        matched_expected_images.to_vec()
    } else {
        raw_matches
    }
}

pub(crate) fn archive_signature_path(archive_path: &Path) -> PathBuf {
    let signature_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!(".{name}.gaia-archive-state.txt"))
        .unwrap_or_else(|| ".gaia-archive-state.txt".to_string());
    archive_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(signature_name)
}

pub(crate) fn archive_signature(
    source_dir: &Path,
    entries: &[String],
    mode: TarArchiveMode,
) -> String {
    let mut signature = format!("gaia-archive-v1\nmode={}\n", mode.as_str());
    for entry in entries {
        let path = source_dir.join(entry);
        signature.push_str(&format!("{entry}={}\n", archive_entry_digest(&path)));
    }
    signature
}

pub(crate) fn archive_entry_digest(path: &Path) -> String {
    if path.is_file() {
        file_sha256_or_placeholder(path)
    } else {
        dir_digest(path)
    }
}

pub(crate) fn archive_signature_is_current(
    source_dir: &Path,
    entries: &[String],
    archive_path: &Path,
    mode: TarArchiveMode,
) -> bool {
    archive_path.is_file()
        && fs::read_to_string(archive_signature_path(archive_path))
            .is_ok_and(|current| current == archive_signature(source_dir, entries, mode))
}

pub(crate) fn write_archive_signature(
    source_dir: &Path,
    entries: &[String],
    archive_path: &Path,
    mode: TarArchiveMode,
) -> Result<(), ImageProviderError> {
    fs::write(
        archive_signature_path(archive_path),
        archive_signature(source_dir, entries, mode),
    )
    .map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write image archive state for '{}': {error}",
                archive_path.display()
            ),
        )
    })
}

pub(crate) struct ArchiveFilesRequest<'a> {
    pub(crate) source_dir: &'a Path,
    pub(crate) entries: &'a [String],
    pub(crate) archive_path: &'a Path,
    pub(crate) mode: TarArchiveMode,
    pub(crate) label: &'static str,
    pub(crate) command: ImageCommandContext<'a>,
}

pub(crate) fn archive_files(
    request: ArchiveFilesRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let ArchiveFilesRequest {
        source_dir,
        entries,
        archive_path,
        mode,
        label,
        command: command_context,
    } = request;
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create archive dir '{}': {error}",
                parent.display()
            ))
        })?;
    }
    let mut command = Command::new("tar");
    command
        .arg(mode.create_arg())
        .arg(archive_path)
        .arg("-C")
        .arg(source_dir);
    for entry in entries {
        command.arg(entry);
    }
    let messages = run_command(
        command,
        label,
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )?;
    write_archive_signature(source_dir, entries, archive_path, mode)?;
    Ok(messages)
}

pub(crate) fn archive_directory(
    source_dir: &Path,
    archive_path: &Path,
    label: &str,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create archive dir '{}': {error}",
                parent.display()
            ))
        })?;
    }
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(archive_path)
        .arg("-C")
        .arg(source_dir)
        .arg(".");
    run_command(command, label, execution, policy, log_sink, cancel_check)
}

fn raw_xz_archive_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".img.xz") || name.ends_with(".raw.xz"))
}

fn compress_primary_image(
    source_path: &Path,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    _policy: &ImageExecutionPolicy,
    _log_sink: Option<ProcessLogSink>,
    _cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create archive dir '{}': {error}",
                parent.display()
            ))
        })?;
    }
    let mut command = Command::new("xz");
    command.arg("-T0").arg("-c").arg(source_path);
    let output_file = fs::File::create(archive_path).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create compressed primary buildroot image '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut command = command_for_execution(&command, execution)?;
    command.stdout(std::process::Stdio::from(output_file));
    let output = command.output().map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::ToolStart,
            format!("failed to start xz compression: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(ImageProviderError::backend_command(format!(
            "failed to compress primary buildroot image '{}': {}",
            source_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(vec![format!(
        "compressed primary buildroot image '{}' to '{}'",
        source_path.display(),
        archive_path.display()
    )])
}
