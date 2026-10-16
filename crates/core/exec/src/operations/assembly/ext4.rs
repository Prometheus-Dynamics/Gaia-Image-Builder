//! `ext4` filesystem images of a tree, built with `mkfs.ext4 -d` and then
//! normalized: `mke2fs -d` copies the builder's uids, gids and timestamps
//! into the image, so `debugfs` sets every entry to root:root with fixed
//! times afterwards. The fixed UUID, hash seed and fake time make two runs
//! over the same tree byte-identical.

use super::filesystems::{AssemblyFilesystemSummary, MIB, collect_archive_entries};
use super::*;

const EXT4_BLOCK_BYTES: u64 = 4096;
/// The inode size passed to `mkfs.ext4 -I`.
const EXT4_INODE_BYTES: u64 = 256;
/// Inodes for each tree entry, plus room for the reserved ones.
const EXT4_INODES_PER_ENTRY: u64 = 2;
const EXT4_INODES_FIXED: u64 = 64;
/// Room for group descriptors, bitmaps and reserved blocks.
const EXT4_AUTO_FIXED_OVERHEAD_BYTES: u64 = 8 * MIB;
/// The least free space `size = "auto"` leaves, or 5% of the content when
/// that is more.
const EXT4_AUTO_MIN_MARGIN_BYTES: u64 = 4 * MIB;
/// The journal is 1/64 of the image, between 4 and 128 MiB.
const EXT4_JOURNAL_DIVISOR: u64 = 64;
const EXT4_JOURNAL_MIN_BYTES: u64 = 4 * MIB;
const EXT4_JOURNAL_MAX_BYTES: u64 = 128 * MIB;
/// The time mke2fs and debugfs use for every timestamp they write.
/// `E2FSPROGS_FAKE_TIME` treats 0 as unset, so the epoch plus one second
/// stands in for it; the entries are then set to 0 by `debugfs`.
const EXT4_FAKE_TIME: &str = "1";
/// The timestamp every entry gets: the epoch, as the cpio archives use.
const EXT4_ENTRY_TIME: &str = "0";
/// Prefix of the UUID derived from a filesystem id when none is set.
const EXT4_UUID_DOMAIN: &[u8] = b"gaia ext4 uuid\n";

/// The image size for `size = "auto"`: the content (each file and directory
/// rounded up to a block), the inode tables, the fixed overhead and the
/// journal, plus a margin, rounded up to 1 MiB. It depends only on the
/// tree's content.
pub(super) fn ext4_auto_bytes(source_tree: &Path, free: u64) -> Result<u64, String> {
    let entries = collect_archive_entries(source_tree)?;
    let mut content = EXT4_BLOCK_BYTES;
    for (_, path) in &entries {
        let metadata = std_fs::symlink_metadata(path)
            .map_err(|error| format!("failed to read ext4 source '{}': {error}", path.display()))?;
        content += if metadata.is_file() {
            metadata.len().div_ceil(EXT4_BLOCK_BYTES) * EXT4_BLOCK_BYTES
        } else {
            EXT4_BLOCK_BYTES
        };
    }
    let inode_table = ext4_inode_count(entries.len()) * EXT4_INODE_BYTES;
    let margin = auto_margin(content, EXT4_AUTO_MIN_MARGIN_BYTES, free);
    let base = content + inode_table + EXT4_AUTO_FIXED_OVERHEAD_BYTES + margin;
    let total = base + ext4_journal_bytes(base);
    Ok(total.div_ceil(MIB) * MIB)
}

/// Inodes for a tree of `entries` entries (the root directory not counted).
fn ext4_inode_count(entries: usize) -> u64 {
    entries as u64 * EXT4_INODES_PER_ENTRY + EXT4_INODES_FIXED
}

/// The journal size for an image of `bytes`.
fn ext4_journal_bytes(bytes: u64) -> u64 {
    (bytes / EXT4_JOURNAL_DIVISOR).clamp(EXT4_JOURNAL_MIN_BYTES, EXT4_JOURNAL_MAX_BYTES)
}

pub(super) fn execute_ext4_filesystem(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    filesystem: &gaia_spec::AssemblyFilesystemSpec,
    source_tree: &Path,
    output: &Path,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyFilesystemSummary, AssemblyError> {
    let mkfs = resolve_assembly_tool(roots, "mkfs.ext4")?;
    let debugfs = resolve_assembly_tool(roots, "debugfs")?;
    let tool_path = format!("mkfs.ext4={};debugfs={}", mkfs.display, debugfs.display);
    tracing::Span::current().record("tool_path", tool_path.as_str());
    let bytes = if filesystem.size.as_deref() == Some("auto") {
        ext4_auto_bytes(source_tree, auto_free_bytes(filesystem)?)?
    } else {
        filesystem
            .parsed_size()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("assembly filesystem '{}' requires size", filesystem.id))?
            .bytes()
    };
    if bytes == 0 || !bytes.is_multiple_of(EXT4_BLOCK_BYTES) {
        return Err(format!(
            "assembly filesystem '{}' size {bytes} must be a multiple of {EXT4_BLOCK_BYTES} bytes",
            filesystem.id
        )
        .into());
    }
    let uuid = match &filesystem.uuid {
        Some(uuid) => uuid.to_ascii_lowercase(),
        None => derived_uuid(&[EXT4_UUID_DOMAIN, filesystem.id.as_str().as_bytes()]),
    };
    let temp = temporary_assembly_output_path(output);
    let script = temp.with_file_name(format!(
        "{}.debugfs",
        temp.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = write_ext4_image(
        spec,
        Ext4WriteContext {
            filesystem,
            source_tree,
            output: &temp,
            script: &script,
            bytes,
            uuid: &uuid,
            mkfs: &mkfs,
            debugfs: &debugfs,
            retention: process_output_retention(spec),
            cancel_check,
        },
    );
    let _ = std_fs::remove_file(&script);
    if let Err(error) = result {
        let _ = std_fs::remove_file(&temp);
        return Err(error);
    }
    publish_assembly_output(&temp, output)?;
    Ok(AssemblyFilesystemSummary {
        bytes: file_len(output)?,
        sha256: file_sha256(output)?,
        output: output.to_path_buf(),
        tool_path: Some(tool_path),
        tool_version: tool_version(&mkfs, ["-V"]),
    })
}

struct Ext4WriteContext<'a> {
    filesystem: &'a gaia_spec::AssemblyFilesystemSpec,
    source_tree: &'a Path,
    output: &'a Path,
    script: &'a Path,
    bytes: u64,
    uuid: &'a str,
    mkfs: &'a ResolvedTool,
    debugfs: &'a ResolvedTool,
    retention: gaia_process::ProcessOutputRetention,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
}

fn write_ext4_image(
    spec: &ResolvedBuildSpec,
    context: Ext4WriteContext<'_>,
) -> Result<(), AssemblyError> {
    let Ext4WriteContext {
        filesystem,
        source_tree,
        output,
        script,
        bytes,
        uuid,
        mkfs,
        debugfs,
        retention,
        cancel_check,
    } = context;
    let image = std_fs::File::create(output).map_err(|error| {
        format!(
            "failed to create ext4 image '{}' before formatting: {error}",
            output.display()
        )
    })?;
    image.set_len(bytes).map_err(|error| {
        format!(
            "failed to size ext4 image '{}' to {bytes} bytes: {error}",
            output.display()
        )
    })?;
    drop(image);

    let entries = collect_archive_entries(source_tree)?.len();
    let journal_mib = ext4_journal_bytes(bytes) / MIB;
    let mut mkfs_command = Command::new(&mkfs.program);
    mkfs_command
        .env("E2FSPROGS_FAKE_TIME", EXT4_FAKE_TIME)
        .args(["-q", "-F", "-t", "ext4", "-b", "4096", "-I"])
        .arg(EXT4_INODE_BYTES.to_string())
        .arg("-N")
        .arg(ext4_inode_count(entries).to_string())
        .arg("-U")
        .arg(uuid)
        .arg("-E")
        .arg(format!("hash_seed={uuid},root_owner=0:0"));
    if filesystem.journal_enabled() {
        mkfs_command.arg("-J").arg(format!("size={journal_mib}"));
    } else {
        mkfs_command.arg("-O").arg("^has_journal");
    }
    if let Some(label) = &filesystem.label {
        mkfs_command.arg("-L").arg(label);
    }
    mkfs_command
        .arg("-d")
        .arg(source_tree)
        .arg(output)
        .arg((bytes / EXT4_BLOCK_BYTES).to_string());
    let format_output =
        run_command_capture_tail(spec, &mut mkfs_command, retention, cancel_check.clone())?;
    if !format_output.status.success() {
        return Err(format!(
            "ext4 formatter failed for '{}' from '{}' using '{}': {}",
            output.display(),
            source_tree.display(),
            mkfs.display,
            format_output.failure_context(&mkfs_command)
        )
        .into());
    }

    let commands = ext4_normalize_script(source_tree)?;
    std_fs::write(script, commands).map_err(|error| {
        format!(
            "failed to write ext4 normalization script '{}': {error}",
            script.display()
        )
    })?;
    let log = script.with_extension("log");
    let mut normalize = Command::new(&debugfs.program);
    normalize
        .env("E2FSPROGS_FAKE_TIME", EXT4_FAKE_TIME)
        .arg("-w")
        .arg("-f")
        .arg(script)
        .arg(output);
    let normalize_output =
        run_command_stdout_to_file(spec, &mut normalize, &log, retention, cancel_check);
    let _ = std_fs::remove_file(&log);
    let normalize_output = normalize_output?;
    // debugfs echoes each command on stdout and reports a failed command on
    // stderr, exiting 0 either way; only its banner is expected there.
    let errors: Vec<String> = normalize_output
        .stderr_tail()
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with("debugfs 1."))
        .map(str::to_string)
        .collect();
    if !normalize_output.status.success() || !errors.is_empty() {
        return Err(format!(
            "ext4 ownership normalization failed for '{}' using '{}': {}",
            output.display(),
            debugfs.display,
            if errors.is_empty() {
                normalize_output.failure_context(&normalize)
            } else {
                errors.join("; ")
            }
        )
        .into());
    }
    Ok(())
}

/// The `debugfs` commands that set every entry of the image to root:root
/// with fixed times: the root directory, `lost+found` (made by `mke2fs`),
/// then every entry of the tree.
pub(super) fn ext4_normalize_script(source_tree: &Path) -> Result<String, String> {
    let mut paths = vec!["/".to_string(), "/lost+found".to_string()];
    for (name, _) in collect_archive_entries(source_tree)? {
        if name.contains(['"', '\\', '\n', '\r', '\0']) {
            return Err(format!(
                "ext4 entry '{name}' has a quote, backslash or control character in its name, which debugfs cannot set"
            ));
        }
        paths.push(format!("/{name}"));
    }
    let mut script = String::new();
    for path in paths {
        for (field, value) in [
            ("uid", "0"),
            ("gid", "0"),
            ("atime", EXT4_ENTRY_TIME),
            ("mtime", EXT4_ENTRY_TIME),
            ("ctime", EXT4_ENTRY_TIME),
            ("crtime", EXT4_ENTRY_TIME),
        ] {
            script.push_str(&format!("set_inode_field \"{path}\" {field} {value}\n"));
        }
    }
    Ok(script)
}
