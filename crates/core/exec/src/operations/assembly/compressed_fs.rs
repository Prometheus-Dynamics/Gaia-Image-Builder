//! Read-only compressed images of a tree: `squashfs` (`mksquashfs`) and
//! `erofs` (`mkfs.erofs`). Both are reproducible: every entry is owned by
//! root with the epoch as its time, xattrs from the build host are left out
//! (they would carry its SELinux labels), and the erofs UUID is derived from
//! the filesystem id unless the spec sets one. The image is as large as its
//! content, so neither kind takes a `size`.

use super::filesystems::AssemblyFilesystemSummary;
use super::*;
use gaia_spec::{AssemblyFilesystemSpec, ErofsCompression, SquashfsCompression};

/// Prefix of the UUID derived from a filesystem id when none is set.
const EROFS_UUID_DOMAIN: &[u8] = b"gaia erofs uuid\n";

pub(super) fn execute_compressed_filesystem(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    filesystem: &AssemblyFilesystemSpec,
    source_tree: &Path,
    output: &Path,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<AssemblyFilesystemSummary, AssemblyError> {
    let squashfs = filesystem.kind == gaia_spec::AssemblyFilesystemKindSpec::Squashfs;
    let (tool, version_args) = if squashfs {
        (resolve_assembly_tool(roots, "mksquashfs")?, "-version")
    } else {
        (resolve_assembly_tool(roots, "mkfs.erofs")?, "--version")
    };
    tracing::Span::current().record("tool_path", tool.display.as_str());
    let temp = temporary_assembly_output_path(output);
    // mksquashfs appends to an existing image unless told not to, and
    // mkfs.erofs overwrites; start from no file either way.
    let _ = std_fs::remove_file(&temp);
    let mut command = Command::new(&tool.program);
    if squashfs {
        command.args(squashfs_args(filesystem, source_tree, &temp)?);
    } else {
        command.args(erofs_args(filesystem, source_tree, &temp)?);
    }
    let result = run_command_capture_tail(
        spec,
        &mut command,
        process_output_retention(spec),
        cancel_check,
    );
    let failure = match result {
        Ok(result) if result.status.success() => None,
        Ok(result) => Some(result.failure_context(&command)),
        Err(error) => {
            let _ = std_fs::remove_file(&temp);
            return Err(error);
        }
    };
    if let Some(context) = failure {
        let _ = std_fs::remove_file(&temp);
        return Err(format!(
            "{} image failed for '{}' from '{}' using '{}': {context}",
            filesystem.kind.as_str(),
            output.display(),
            source_tree.display(),
            tool.display,
        )
        .into());
    }
    publish_assembly_output(&temp, output)?;
    Ok(AssemblyFilesystemSummary {
        bytes: file_len(output)?,
        sha256: file_sha256(output)?,
        output: output.to_path_buf(),
        tool_path: Some(tool.display.clone()),
        tool_version: tool_version(&tool, [version_args]),
    })
}

/// `mksquashfs <tree> <output>` and its options.
pub(super) fn squashfs_args(
    filesystem: &AssemblyFilesystemSpec,
    source_tree: &Path,
    output: &Path,
) -> Result<Vec<std::ffi::OsString>, String> {
    let options = &filesystem.compressed;
    let compression = SquashfsCompression::parse(options.compression.as_deref())
        .ok_or_else(|| unknown_compression(filesystem, SquashfsCompression::NAMES))?;
    let mut args: Vec<std::ffi::OsString> = vec![
        source_tree.into(),
        output.into(),
        "-noappend".into(),
        "-no-progress".into(),
        "-comp".into(),
        compression.as_str().into(),
    ];
    if let Some(level) = filesystem.compression_level {
        args.extend(["-Xcompression-level".into(), level.to_string().into()]);
    }
    if let Some(block_size) = options.block_size {
        args.extend(["-b".into(), block_size.to_string().into()]);
    }
    for flag in [
        "-all-root",
        "-no-xattrs",
        "-mkfs-time",
        "0",
        "-all-time",
        "0",
        "-reproducible",
    ] {
        args.push(flag.into());
    }
    Ok(args)
}

/// `mkfs.erofs <options> <output> <tree>`.
pub(super) fn erofs_args(
    filesystem: &AssemblyFilesystemSpec,
    source_tree: &Path,
    output: &Path,
) -> Result<Vec<std::ffi::OsString>, String> {
    let options = &filesystem.compressed;
    let compression = ErofsCompression::parse(options.compression.as_deref())
        .ok_or_else(|| unknown_compression(filesystem, ErofsCompression::NAMES))?;
    let uuid = match &filesystem.uuid {
        Some(uuid) => uuid.to_ascii_lowercase(),
        None => derived_uuid(&[EROFS_UUID_DOMAIN, filesystem.id.as_str().as_bytes()]),
    };
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if compression != ErofsCompression::None {
        let mut value = format!("-z{}", compression.as_str());
        if let Some(level) = filesystem.compression_level {
            value.push_str(&format!(",{level}"));
        }
        args.push(value.into());
    }
    if let Some(block_size) = options.block_size {
        args.extend(["-b".into(), block_size.to_string().into()]);
    }
    if let Some(pcluster_size) = options.pcluster_size {
        args.push(format!("-C{pcluster_size}").into());
    }
    let extended: Vec<&str> = [
        (options.dedupe, "dedupe"),
        (options.fragments, "fragments"),
        (options.ztailpacking, "ztailpacking"),
    ]
    .into_iter()
    .filter_map(|(set, name)| set.then_some(name))
    .collect();
    if !extended.is_empty() {
        args.extend(["-E".into(), extended.join(",").into()]);
    }
    if let Some(label) = &filesystem.label {
        args.extend(["-L".into(), label.into()]);
    }
    // -T0 sets the build time and, with no --mkfs-time, every entry's time.
    // -x-1 stores no xattrs.
    for flag in ["-T0", "--all-root", "-x-1", "-U"] {
        args.push(flag.into());
    }
    args.push(uuid.into());
    args.push(output.into());
    args.push(source_tree.into());
    Ok(args)
}

fn unknown_compression(filesystem: &AssemblyFilesystemSpec, names: &str) -> String {
    format!(
        "assembly filesystem '{}' compression '{}' is not one of {names}",
        filesystem.id,
        filesystem
            .compressed
            .compression
            .as_deref()
            .unwrap_or_default()
    )
}
