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
            if let Some(kind) = compressed_raw_archive(archive_path) {
                return compress_primary_image(
                    kind,
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
            let clock = gaia_process::ActiveClock::start();
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
            return Ok(vec![
                format!(
                    "copied primary buildroot image '{}' to '{}'",
                    source_path.display(),
                    archive_path.display()
                ),
                gaia_process::step_time_message("copy primary image", clock.elapsed()),
            ]);
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
    /// The compression named in the step-time line (`archive <name> (<label>)`).
    fn log_label(self) -> &'static str {
        match self {
            Self::Plain => "tar",
            Self::Xz => "xz",
        }
    }

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
        signature.push_str(&format!(
            "{entry}={}\n",
            archive_entry_digest(source_dir, &path)
        ));
    }
    signature
}

/// The digest of an archive entry. A file in the source dir (the collect
/// dir) whose digest its content manifest records is not read again.
pub(crate) fn archive_entry_digest(base: &Path, path: &Path) -> String {
    if path.is_file() {
        gaia_image_providers::recorded_sha256(base, path)
            .unwrap_or_else(|| file_sha256_or_placeholder(path))
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
    let input_bytes = entries
        .iter()
        .map(|entry| tree_bytes(&source_dir.join(entry)))
        .sum();
    let mut command = Command::new("tar");
    if matches!(mode, TarArchiveMode::Xz) {
        command.env("XZ_OPT", "-T0 --block-size=24MiB");
    }
    command
        .arg(mode.create_arg())
        .arg(archive_path)
        .arg("-C")
        .arg(source_dir);
    for entry in entries {
        command.arg(entry);
    }
    let clock = gaia_process::ActiveClock::start();
    let mut messages = run_command(
        command,
        label,
        command_context.execution,
        command_context.policy,
        command_context.log_sink,
        command_context.cancel_check,
    )?;
    write_archive_signature(source_dir, entries, archive_path, mode)?;
    messages.extend(archive_log_messages(
        archive_path,
        mode.log_label(),
        Some(input_bytes),
        clock.elapsed(),
    ));
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
    // The input is the whole source tree, which is not worth walking for a
    // size line, so only the output size is reported.
    let clock = gaia_process::ActiveClock::start();
    let mut messages = run_command(command, label, execution, policy, log_sink, cancel_check)?;
    messages.extend(archive_log_messages(
        archive_path,
        TarArchiveMode::Plain.log_label(),
        None,
        clock.elapsed(),
    ));
    Ok(messages)
}

/// The step time and size lines of a finished archive: `archive <name>
/// (<label>)` with its wall time, and `archived <name>: <in> -> <out> in
/// <secs>s` (just the output size when the input is not known).
pub(crate) fn archive_log_messages(
    archive_path: &Path,
    label: &str,
    input_bytes: Option<u64>,
    elapsed: Duration,
) -> Vec<String> {
    let name = archive_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| archive_path.display().to_string());
    let output_bytes = fs::metadata(archive_path).map_or(0, |metadata| metadata.len());
    let sizes = match input_bytes {
        Some(input) => format!(
            "{} -> {}",
            format_archive_bytes(input),
            format_archive_bytes(output_bytes)
        ),
        None => format_archive_bytes(output_bytes),
    };
    vec![
        gaia_process::step_time_message(&format!("archive {name} ({label})"), elapsed),
        format!("archived {name}: {sizes} in {}s", elapsed.as_secs()),
    ]
}

/// Total size of a file, or of every file under a directory (symlinks count
/// as their own size).
fn tree_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| tree_bytes(&entry.path()))
            .sum()
    })
}

/// `1.4 GiB`, `142 MiB`, `812 B`.
fn format_archive_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value >= 100.0 => format!("{value:.0} {}", UNITS[unit]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

/// The compression of a compressed raw disk archive name (`.img.xz`,
/// `.img.zst`, ...).
pub(crate) fn compressed_raw_archive(path: &Path) -> Option<gaia_spec::RawDiskArchive> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(gaia_spec::raw_disk_archive)
        .filter(|kind| *kind != gaia_spec::RawDiskArchive::Plain)
}

fn compress_primary_image(
    kind: gaia_spec::RawDiskArchive,
    source_path: &Path,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    compress_primary_image_with_program(
        (kind, None),
        source_path,
        archive_path,
        execution,
        policy,
        log_sink,
        cancel_check,
    )
}

/// Compresses a disk image with `kind`'s compressor, or the given program
/// in its place (tests).
pub(crate) fn compress_primary_image_with_program(
    (kind, program): (gaia_spec::RawDiskArchive, Option<&Path>),
    source_path: &Path,
    archive_path: &Path,
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
    let temp_archive = temporary_archive_output_path(archive_path);
    // `local_jobs = 0` means all cores.
    let (compressor, args) = kind.compressor(policy.local_jobs).ok_or_else(|| {
        ImageProviderError::backend_command("an uncompressed disk archive is not compressed")
    })?;
    let mut command = Command::new(program.unwrap_or(Path::new(compressor)));
    command.args(args).arg(source_path);
    let clock = gaia_process::ActiveClock::start();
    let output = command_stdout_to_file_with_timeout(CommandStdoutToFileRequest {
        command: &mut command,
        output_path: &temp_archive,
        execution,
        timeout: Duration::from_secs(policy.timeout_seconds.max(1)),
        label: "buildroot raw image compression",
        retention: policy.output_retention,
        log_sink,
        cancel_check,
    })
    .inspect_err(|_| {
        let _ = fs::remove_file(&temp_archive);
    })?;
    if !output.status.success() {
        let _ = fs::remove_file(&temp_archive);
        return Err(ImageProviderError::backend_command(format!(
            "failed to compress primary buildroot image '{}': {}",
            source_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    publish_archive_output(&temp_archive, archive_path)?;
    let mut messages = vec![format!(
        "compressed primary buildroot image '{}' to '{}'",
        source_path.display(),
        archive_path.display()
    )];
    messages.extend(archive_log_messages(
        archive_path,
        compressor,
        fs::metadata(source_path)
            .ok()
            .map(|metadata| metadata.len()),
        clock.elapsed(),
    ));
    Ok(messages)
}

pub(crate) fn temporary_archive_output_path(output: &Path) -> PathBuf {
    gaia_image_providers::temporary_publish_output_path(output, "image-archive")
}

fn publish_archive_output(temp: &Path, output: &Path) -> Result<(), ImageProviderError> {
    gaia_image_providers::publish_replace_output(temp, output, "image archive", "image-archive")
        .map_err(|message| ImageProviderError::new(ImageProviderErrorKind::RuntimeState, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_installed(tool: &str) -> bool {
        Command::new(tool).arg("--version").output().is_ok()
    }

    #[test]
    fn compressed_raw_archives_log_a_step_time_and_sizes() {
        for (kind, tool, extension) in [
            (gaia_spec::RawDiskArchive::Xz, "xz", "xz"),
            (gaia_spec::RawDiskArchive::Zstd, "zstd", "zst"),
        ] {
            if !tool_installed(tool) {
                eprintln!("skipping archive step-time test: '{tool}' is not installed");
                continue;
            }
            let dir = std::env::temp_dir().join(format!(
                "gaia-archive-step-time-{tool}-{}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("test dir");
            let source = dir.join("helios.img");
            fs::write(&source, "raw image ".repeat(4096)).expect("raw image");
            let archive = dir.join(format!("helios.img.{extension}"));
            let execution = ImageExecutionContext {
                workspace_root: dir.clone(),
                docker_image: None,
            };
            let policy = ImageExecutionPolicy::default();

            let messages = compress_primary_image_with_program(
                (kind, None),
                &source,
                &archive,
                &execution,
                &policy,
                None,
                None,
            )
            .expect("compression should succeed");

            let step = format!("archive helios.img.{extension} ({tool})");
            assert!(
                messages.iter().any(|message| {
                    gaia_process::parse_step_time(message).is_some_and(|(name, _)| name == step)
                }),
                "missing step time '{step}' in {messages:?}"
            );
            assert!(
                messages.iter().any(|message| {
                    message.starts_with(&format!("archived helios.img.{extension}: "))
                        && message.contains(" -> ")
                }),
                "missing size line in {messages:?}"
            );
            fs::remove_dir_all(&dir).expect("cleanup");
        }
    }

    #[test]
    fn format_archive_bytes_uses_binary_units() {
        assert_eq!(format_archive_bytes(812), "812 B");
        assert_eq!(format_archive_bytes(142 * 1024 * 1024), "142 MiB");
        assert_eq!(format_archive_bytes(1_503_238_553), "1.4 GiB");
    }
}
