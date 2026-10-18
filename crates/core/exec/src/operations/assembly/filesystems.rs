use super::cpio::{plan_newc, run_compressor_from_plan, write_newc_file};
use super::*;

/// The zstd level of a `cpio-zstd` image when the spec sets none.
const CPIO_ZSTD_DEFAULT_LEVEL: u32 = 19;

/// The `[[image.assembly.devices]]` entries whose tree is `filesystem`'s
/// source tree: the device nodes its cpio archive adds.
fn tree_devices<'a>(
    spec: &'a ResolvedBuildSpec,
    filesystem: &gaia_spec::AssemblyFilesystemSpec,
) -> Vec<&'a gaia_spec::AssemblyDeviceSpec> {
    spec.image
        .assembly
        .as_ref()
        .map(|assembly| {
            assembly
                .devices
                .iter()
                .filter(|device| device.tree.as_str() == filesystem.source_tree.as_str())
                .collect()
        })
        .unwrap_or_default()
}
/// The size of a vfat image when the spec sets none.
const VFAT_DEFAULT_BYTES: u64 = 32 * 1024 * 1024;
/// The cluster size `size = "auto"` assumes for every file and directory:
/// the last cluster of each is counted whole.
const VFAT_AUTO_CLUSTER_BYTES: u64 = 4 * 1024;
/// Room for the boot sector, the FAT tables and the root directory.
const VFAT_AUTO_FIXED_OVERHEAD_BYTES: u64 = 1024 * 1024;
/// The least free space `size = "auto"` leaves, or 5% of the content when
/// that is more.
const VFAT_AUTO_MIN_MARGIN_BYTES: u64 = 1024 * 1024;
pub(super) const MIB: u64 = 1024 * 1024;

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
            let plan = plan_newc(source_tree, &tree_devices(spec, filesystem))?;
            write_newc_file(&plan, &temp)?;
            let sha256 = publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256,
                output,
                tool_path: None,
                tool_version: None,
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::CpioGzip => {
            let tool = resolve_assembly_tool(roots, "gzip")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&output);
            // The archive goes into gzip's stdin: no full cpio copy on disk.
            let plan = plan_newc(source_tree, &tree_devices(spec, filesystem))?;
            let mut command = Command::new(&tool.program);
            command.arg("-n").arg("-c");
            let gzip_output =
                run_compressor_from_plan(spec, &mut command, &plan, &temp, cancel_check)
                    .inspect_err(|_| {
                        let _ = std_fs::remove_file(&temp);
                    })?;
            if !gzip_output.status.success() {
                let _ = std_fs::remove_file(&temp);
                return Err(format!(
                    "cpio-gzip compressor failed for '{}' using '{}': {}",
                    source_tree.display(),
                    tool.display,
                    gzip_output.failure_context(&command)
                )
                .into());
            }
            let sha256 = publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256,
                output,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::CpioXz => {
            let tool = resolve_assembly_tool(roots, "xz")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&output);
            let temp_cpio = temp.with_extension("cpio.tmp");
            // xz keeps its input as a file: its block headers (and so the
            // proven-bootable bytes) are the ones a file input gives.
            let plan = plan_newc(source_tree, &tree_devices(spec, filesystem))?;
            write_newc_file(&plan, &temp_cpio)?;
            // The kernel's XZ decoder reads CRC32 (or no) checks only, and it
            // allocates the whole dictionary up front: an initramfs with the
            // 64 MiB dictionary of `-9` fails to unpack before /init runs.
            // The stream must also carry its sizes in the block header, which
            // xz writes only in threaded mode (`-T1` leaves them out, and that
            // initramfs did not boot either). xz's own threaded block size
            // (3 x the dictionary) is kept: with `--block-size` set to the cpio
            // size xz wrote a 16-byte block header the slot kernel did not
            // boot, where plain `-T2` writes the 20-byte header that boots.
            // A fixed thread count keeps the output the same on every host.
            let input_bytes = file_len(&temp_cpio).inspect_err(|_| {
                let _ = std_fs::remove_file(&temp_cpio);
            })?;
            let filter = cpio_xz_filter(filesystem.compression_level, input_bytes);
            let mut command = Command::new(&tool.program);
            command
                .args(["-T2", "--check=crc32", filter.as_str(), "-c"])
                .arg(&temp_cpio);
            let xz_output = run_command_stdout_to_file(
                spec,
                &mut command,
                &temp,
                process_output_retention(spec),
                cancel_check,
            )
            .inspect_err(|_| {
                let _ = std_fs::remove_file(&temp);
            })?;
            let _ = std_fs::remove_file(&temp_cpio);
            if !xz_output.status.success() {
                let _ = std_fs::remove_file(&temp);
                return Err(format!(
                    "cpio-xz compressor failed for '{}' using '{}': {}",
                    source_tree.display(),
                    tool.display,
                    xz_output.failure_context(&command)
                )
                .into());
            }
            let sha256 = publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256,
                output,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::CpioZstd => {
            let tool = resolve_assembly_tool(roots, "zstd")?;
            tracing::Span::current().record("tool_path", tool.display.as_str());
            let temp = temporary_assembly_output_path(&output);
            // The archive goes into zstd's stdin. `--stream-size` gives the
            // size the plan computed, so the frame records it as a file input
            // does and the bytes match.
            let plan = plan_newc(source_tree, &tree_devices(spec, filesystem))?;
            // Same flags as the zstd transform; the level is fixed by the
            // spec, so the output is the same for any thread count.
            let level = filesystem
                .compression_level
                .unwrap_or(CPIO_ZSTD_DEFAULT_LEVEL);
            let mut command = Command::new(&tool.program);
            command
                .args(["-q", "-c", "--no-progress", "-T0"])
                .arg(format!("-{level}"))
                .arg(format!("--stream-size={}", plan.bytes()));
            let zstd_output =
                run_compressor_from_plan(spec, &mut command, &plan, &temp, cancel_check)
                    .inspect_err(|_| {
                        let _ = std_fs::remove_file(&temp);
                    })?;
            if !zstd_output.status.success() {
                let _ = std_fs::remove_file(&temp);
                return Err(format!(
                    "cpio-zstd compressor failed for '{}' using '{}': {}",
                    source_tree.display(),
                    tool.display,
                    zstd_output.failure_context(&command)
                )
                .into());
            }
            let sha256 = publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256,
                output,
                tool_path: Some(tool.display.clone()),
                tool_version: tool_version(&tool, ["--version"]),
            })
        }
        gaia_spec::AssemblyFilesystemKindSpec::Ext4 => {
            execute_ext4_filesystem(spec, roots, filesystem, source_tree, &output, cancel_check)
        }
        gaia_spec::AssemblyFilesystemKindSpec::Squashfs
        | gaia_spec::AssemblyFilesystemKindSpec::Erofs => execute_compressed_filesystem(
            spec,
            roots,
            filesystem,
            source_tree,
            &output,
            cancel_check,
        ),
        gaia_spec::AssemblyFilesystemKindSpec::Vfat => {
            let mformat = resolve_assembly_tool(roots, "mformat")?;
            let mcopy = resolve_assembly_tool(roots, "mcopy")?;
            let tool_path = format!("mformat={};mcopy={}", mformat.display, mcopy.display);
            tracing::Span::current().record("tool_path", tool_path.as_str());
            let bytes = if filesystem.size.as_deref() == Some("auto") {
                vfat_auto_bytes(source_tree, auto_free_bytes(filesystem)?)?
            } else {
                filesystem
                    .parsed_size()
                    .map_err(|error| error.to_string())?
                    .map_or(VFAT_DEFAULT_BYTES, |size| size.bytes())
            };
            let temp = temporary_assembly_output_path(&output);
            write_vfat_filesystem(VfatWriteContext {
                spec,
                source_tree,
                output: &temp,
                bytes,
                fat_size: filesystem.fat_size,
                deterministic: filesystem.deterministic,
                volume_id: filesystem.id.as_str(),
                mformat: &mformat,
                mcopy: &mcopy,
                retention: process_output_retention(spec),
                cancel_check,
            })?;
            let sha256 = publish_assembly_output(&temp, &output)?;
            Ok(AssemblyFilesystemSummary {
                bytes: file_len(&output)?,
                sha256,
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
    fat_size: Option<u32>,
    /// A deterministic image gets a fixed volume serial and keeps the
    /// entries' modification times.
    deterministic: bool,
    /// The filesystem id: the serial of a deterministic image derives from it.
    volume_id: &'a str,
    mformat: &'a ResolvedTool,
    mcopy: &'a ResolvedTool,
    retention: gaia_process::ProcessOutputRetention,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
}

/// Images from this size up get FAT16 or FAT32 from `mformat`'s own choice.
const VFAT_FAT16_DEFAULT_FROM_BYTES: u64 = 16 * MIB;
/// The smallest image where one sector per cluster still gives FAT16's
/// minimum of 4085 clusters, with room for the FATs and root directory.
const VFAT_FAT16_MIN_BYTES: u64 = 3 * MIB;

/// The `mformat` options for an image of `bytes` with the filesystem's
/// `fat_size`. Below 16 MiB `mformat` picks FAT12, which some boot firmwares
/// (e.g. Raspberry Pi's) do not boot from, so with no `fat_size` and with
/// `fat_size = 16` one sector per cluster makes those images FAT16. Images too
/// small for FAT16, and images of 16 MiB or more, keep `mformat`'s choice;
/// `fat_size = 12` always does, and `fat_size = 32` asks for FAT32.
pub(super) fn vfat_mformat_args(bytes: u64, fat_size: Option<u32>) -> Vec<&'static str> {
    match fat_size {
        Some(12) => Vec::new(),
        Some(32) => vec!["-F"],
        _ if (VFAT_FAT16_MIN_BYTES..VFAT_FAT16_DEFAULT_FROM_BYTES).contains(&bytes) => {
            vec!["-c", "1"]
        }
        _ => Vec::new(),
    }
}

/// The FAT type `mformat` wrote, from the boot sector's type string: 12 or
/// 16 at offset 54, 32 at offset 82. `None` when neither names one.
pub(super) fn vfat_fat_size(boot_sector: &[u8]) -> Option<u32> {
    let at = |offset: usize| boot_sector.get(offset..offset + 8);
    match (at(54), at(82)) {
        (Some(b"FAT12   "), _) => Some(12),
        (Some(b"FAT16   "), _) => Some(16),
        (_, Some(b"FAT32   ")) => Some(32),
        _ => None,
    }
}

/// With a `fat_size` set, fails when `mformat` wrote another FAT type, as it
/// does when the image is too small or too large for the asked type.
fn check_vfat_fat_size(output: &Path, bytes: u64, fat_size: Option<u32>) -> Result<(), String> {
    let Some(wanted) = fat_size else {
        return Ok(());
    };
    let mut boot_sector = [0u8; 90];
    std_fs::File::open(output)
        .and_then(|mut image| std::io::Read::read_exact(&mut image, &mut boot_sector))
        .map_err(|error| {
            format!(
                "failed to read the boot sector of vfat image '{}': {error}",
                output.display()
            )
        })?;
    match vfat_fat_size(&boot_sector) {
        Some(made) if made == wanted => Ok(()),
        made => Err(format!(
            "vfat image '{}' of {bytes} bytes asks for fat_size = {wanted}, but mformat made {}; \
             FAT12 needs less than 16 MiB, FAT16 at least 3 MiB",
            output.display(),
            made.map_or_else(
                || "an unknown FAT type".to_string(),
                |made| format!("FAT{made}")
            )
        )),
    }
}

/// The margin `size = "auto"` leaves: 5% of the content or the minimum,
/// whichever is more, and at least the filesystem's `free`.
pub(super) fn auto_margin(content: u64, min: u64, free: u64) -> u64 {
    (content / 20).max(min).max(free)
}

/// The `free` of a filesystem with `size = "auto"`, in bytes (0 when unset).
pub(super) fn auto_free_bytes(
    filesystem: &gaia_spec::AssemblyFilesystemSpec,
) -> Result<u64, String> {
    Ok(filesystem
        .parsed_free()
        .map_err(|error| format!("assembly filesystem '{}' free: {error}", filesystem.id))?
        .map_or(0, |free| free.bytes()))
}

/// The image size for `size = "auto"`: the content (each file rounded up to
/// a cluster, each directory costing one) plus the fixed FAT overhead plus a
/// margin, rounded up to 1 MiB. It depends only on the tree's content, so the
/// same tree gives the same size. A tree that overruns the estimate fails
/// when `mcopy` runs out of space, rather than silently truncating.
pub(super) fn vfat_auto_bytes(source_tree: &Path, free: u64) -> Result<u64, String> {
    let content = vfat_content_bytes(source_tree)?;
    let margin = auto_margin(content, VFAT_AUTO_MIN_MARGIN_BYTES, free);
    let total = content + VFAT_AUTO_FIXED_OVERHEAD_BYTES + margin;
    Ok(total.div_ceil(MIB) * MIB)
}

fn vfat_content_bytes(dir: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    let mut children = std_fs::read_dir(dir)
        .map_err(|error| {
            format!(
                "failed to read vfat source tree '{}': {error}",
                dir.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "failed to read vfat source tree '{}': {error}",
                dir.display()
            )
        })?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let path = child.path();
        let metadata = std_fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to read vfat source '{}': {error}", path.display()))?;
        // mcopy follows symlinks, so a link is sized by its target.
        let followed = if metadata.file_type().is_symlink() {
            std_fs::metadata(&path).ok()
        } else {
            Some(metadata)
        };
        match followed {
            Some(target) if target.is_file() => {
                total += target.len().div_ceil(VFAT_AUTO_CLUSTER_BYTES) * VFAT_AUTO_CLUSTER_BYTES;
            }
            Some(target) if target.is_dir() => {
                total += VFAT_AUTO_CLUSTER_BYTES;
                // A link to a directory is not followed again, so a loop
                // of links cannot recurse forever.
                if !path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    total += vfat_content_bytes(&path)?;
                }
            }
            _ => total += VFAT_AUTO_CLUSTER_BYTES,
        }
    }
    Ok(total)
}

/// The volume serial a deterministic vfat image gets: four bytes of the
/// sha256 of the filesystem's id, so one id always gives the same serial.
pub(super) fn vfat_volume_serial(volume_id: &str) -> String {
    let digest = Sha256::digest(format!("gaia vfat volume serial\n{volume_id}").as_bytes());
    format!(
        "{:02X}{:02X}{:02X}{:02X}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

fn write_vfat_filesystem(context: VfatWriteContext<'_>) -> Result<(), AssemblyError> {
    let VfatWriteContext {
        spec,
        source_tree,
        output,
        bytes,
        fat_size,
        deterministic,
        volume_id,
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
    format_command.args(vfat_mformat_args(bytes, fat_size));
    if deterministic {
        format_command
            .env("TZ", "UTC")
            .arg("-N")
            .arg(vfat_volume_serial(volume_id));
    }
    format_command.arg("-i").arg(output).arg("::");
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
    check_vfat_fat_size(output, bytes, fat_size)?;

    // A deterministic image is copied from a staging copy of the tree whose
    // entries carry one fixed modification time: `mcopy -m` keeps it, so the
    // timestamps do not depend on when the tree was built.
    let staging = deterministic.then(|| {
        output.with_file_name(format!(
            "{}.tree",
            output.file_name().unwrap_or_default().to_string_lossy()
        ))
    });
    let copied = (|| -> Result<(), AssemblyError> {
        let from = match &staging {
            Some(staging) => {
                stage_vfat_tree(source_tree, staging)?;
                staging.as_path()
            }
            None => source_tree,
        };
        let mut children = std_fs::read_dir(from)
            .map_err(|error| {
                format!(
                    "failed to read vfat source tree '{}': {error}",
                    from.display()
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!(
                    "failed to read vfat source tree '{}': {error}",
                    from.display()
                )
            })?;
        children.sort_by_key(|entry| entry.path());
        for child in children {
            let mut copy_command = Command::new(&mcopy.program);
            copy_command.arg("-i").arg(output);
            if deterministic {
                // mtools stores local times: UTC keeps the image the same on
                // every host.
                copy_command.env("TZ", "UTC").arg("-m");
            }
            copy_command.arg("-s").arg(child.path()).arg("::");
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
    })();
    if let Some(staging) = &staging {
        let _ = std_fs::remove_dir_all(staging);
    }
    copied
}

/// The modification time every entry of a deterministic vfat image gets: the
/// FAT epoch, 1980-01-01 00:00:00 UTC (FAT cannot store earlier times).
const VFAT_FIXED_MTIME_SECS: u64 = 315_532_800;

/// Copies the entries of `source` (files followed through their links, as
/// `mcopy` reads them) into `staging`, each with the fixed modification time.
fn stage_vfat_tree(source: &Path, staging: &Path) -> Result<(), AssemblyError> {
    let _ = std_fs::remove_dir_all(staging);
    std_fs::create_dir_all(staging).map_err(|error| {
        format!(
            "failed to create vfat staging dir '{}': {error}",
            staging.display()
        )
    })?;
    let fixed = std::time::UNIX_EPOCH + Duration::from_secs(VFAT_FIXED_MTIME_SECS);
    let entries = collect_archive_entries(source)?;
    for (name, path) in &entries {
        let target = staging.join(name);
        let metadata = match std_fs::metadata(path) {
            Ok(metadata) => metadata,
            // A dangling link has nothing to copy.
            Err(_) => continue,
        };
        if metadata.is_dir() {
            std_fs::create_dir_all(&target).map_err(|error| {
                format!(
                    "failed to create vfat staging dir '{}': {error}",
                    target.display()
                )
            })?;
        } else if metadata.is_file() {
            std_fs::copy(path, &target).map_err(|error| {
                format!(
                    "failed to stage vfat file '{}' to '{}': {error}",
                    path.display(),
                    target.display()
                )
            })?;
        } else {
            continue;
        }
        let file = std_fs::File::open(&target).map_err(|error| {
            format!(
                "failed to open vfat staging entry '{}': {error}",
                target.display()
            )
        })?;
        file.set_times(std::fs::FileTimes::new().set_modified(fixed))
            .map_err(|error| {
                format!(
                    "failed to set the time of vfat staging entry '{}': {error}",
                    target.display()
                )
            })?;
    }
    // Directory times last: copying into a directory changes its time.
    let mut dirs: Vec<PathBuf> = entries
        .iter()
        .filter(|(_, path)| std_fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .map(|(name, _)| staging.join(name))
        .collect();
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        let file = std_fs::File::open(&dir).map_err(|error| {
            format!(
                "failed to open vfat staging dir '{}': {error}",
                dir.display()
            )
        })?;
        file.set_times(std::fs::FileTimes::new().set_modified(fixed))
            .map_err(|error| {
                format!(
                    "failed to set the time of vfat staging dir '{}': {error}",
                    dir.display()
                )
            })?;
    }
    Ok(())
}

pub(super) fn collect_archive_entries(
    source_tree: &Path,
) -> Result<Vec<(String, PathBuf)>, String> {
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

/// The largest LZMA2 dictionary a `cpio-xz` initramfs gets: the kernel's
/// decoder allocates the whole dictionary (Documentation/staging/xz.rst).
pub(super) const CPIO_XZ_MAX_DICT: u64 = 1024 * 1024;

/// The `--lzma2` filter of a `cpio-xz` image: the level's preset (6 when
/// unset) with its dictionary capped at 1 MiB, or the input size rounded up
/// to 4 KiB (xz's smallest dictionary) when that is smaller.
pub(super) fn cpio_xz_filter(level: Option<u32>, input_bytes: u64) -> String {
    let dict = input_bytes
        .div_ceil(4096)
        .saturating_mul(4096)
        .clamp(4096, CPIO_XZ_MAX_DICT);
    format!("--lzma2=preset={},dict={dict}", level.unwrap_or(6))
}
