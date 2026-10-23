//! The erofs image and dm-verity tree of one layer: `mkfs.erofs --tar` over
//! the layer tar, and `veritysetup format` appending the tree, with both
//! tools taken from Buildroot's host dir first.

use super::layers::{VERITY_BLOCK, file_len, runtime_error};
use super::*;
use std::io::Write;

/// A UUID derived from a hex digest, so the same layer gets the same UUID.
pub(crate) fn uuid_from_hex(hex: &str) -> String {
    let digits: String = hex.chars().take(32).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// `mkfs.erofs` over the layer tar. The tar's owners are kept (no
/// `--all-root`), and times and xattrs are fixed, so the image is the same
/// for the same tar.
pub(crate) fn run_mkfs_erofs(
    host_dir: &Path,
    out: &Path,
    tar: &Path,
    uuid: &str,
) -> Result<(), ImageProviderError> {
    let mut command = Command::new(host_tool(host_dir, "mkfs.erofs"));
    command
        .args(["-T0", "-x-1", "-U", uuid, "--tar=f"])
        .arg(out)
        .arg(tar);
    let _ = fs::remove_file(out);
    let output = command.output().map_err(|error| {
        runtime_error(format!(
            "image.layers erofs needs mkfs.erofs in Buildroot's host dir or on PATH (set [image.layers] erofs = false to skip it): {error}"
        ))
    })?;
    if !output.status.success() {
        let _ = fs::remove_file(out);
        return Err(runtime_error(format!(
            "mkfs.erofs failed for layer tar '{}': {}",
            tar.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// A tool from Buildroot's host dir (`bin`, `sbin`, `usr/bin`, `usr/sbin`),
/// as the assembly resolves its tools, else the bare name from PATH.
pub(crate) fn host_tool(host_dir: &Path, name: &str) -> PathBuf {
    ["bin", "sbin", "usr/bin", "usr/sbin"]
        .iter()
        .map(|dir| host_dir.join(dir).join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// What `veritysetup format` reports for an erofs image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerityRecord {
    pub(crate) root_hash: String,
    pub(crate) hash_offset: u64,
    pub(crate) data_blocks: u64,
    pub(crate) salt: String,
}

/// A dm-verity tree appended to the erofs image at `path`, as the verity
/// transform does: the image is padded to whole data blocks, and the tree
/// starts at the next hash-block boundary.
pub(crate) fn verity_format(
    host_dir: &Path,
    path: &Path,
    name: &str,
) -> Result<VerityRecord, ImageProviderError> {
    let len = file_len(path)?;
    let padded = len.div_ceil(VERITY_BLOCK) * VERITY_BLOCK;
    let data_blocks = padded / VERITY_BLOCK;
    let hash_offset = padded;
    let salt = gaia_process::sha256_file(path)
        .map_err(|error| runtime_error(format!("failed to hash '{}': {error}", path.display())))?;
    let uuid = uuid_from_hex(&salt);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| runtime_error(format!("failed to open '{}': {error}", path.display())))?;
    let pad = vec![0u8; (padded - len) as usize];
    file.write_all(&pad)
        .map_err(|error| runtime_error(format!("failed to pad '{}': {error}", path.display())))?;
    drop(file);
    let output = Command::new(host_tool(host_dir, "veritysetup"))
        .arg("format")
        .arg(path)
        .arg(path)
        .arg("--hash=sha256")
        .arg(format!("--data-block-size={VERITY_BLOCK}"))
        .arg(format!("--hash-block-size={VERITY_BLOCK}"))
        .arg(format!("--salt={salt}"))
        .arg(format!("--uuid={uuid}"))
        .arg(format!("--hash-offset={hash_offset}"))
        .output()
        .map_err(|error| {
            runtime_error(format!(
                "image.layers verity needs veritysetup in Buildroot's host dir or on PATH (or set verity = false): {error}"
            ))
        })?;
    if !output.status.success() {
        return Err(runtime_error(format!(
            "veritysetup failed for layer '{name}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    let root_hash = report_value(&report, "Root hash").ok_or_else(|| {
        runtime_error(format!(
            "veritysetup reported no root hash for layer '{name}'"
        ))
    })?;
    let reported_blocks = report_value(&report, "Data blocks").unwrap_or_default();
    if reported_blocks != data_blocks.to_string() {
        return Err(runtime_error(format!(
            "veritysetup covered {reported_blocks} data blocks for layer '{name}', expected {data_blocks}"
        )));
    }
    Ok(VerityRecord {
        root_hash,
        hash_offset,
        data_blocks,
        salt,
    })
}

pub(crate) fn report_value(report: &str, label: &str) -> Option<String> {
    report.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == label).then(|| value.trim().to_string())
    })
}
