//! `verity` transforms: a dm-verity hash tree over a data image, written by
//! `veritysetup format`. The tree is appended to the data (the default
//! layout: the superblock sits at the hash offset, the first block past the
//! padded data) or written to its own file. Everything an opener needs is
//! recorded: the root hash, salt, UUID, hash offset and block sizes.
//!
//! Reproducible: the salt and UUID default to values derived from the data
//! image's sha256, so the same data gives the same tree and root hash.

use super::*;
use gaia_spec::AssemblyTransformSpec;
use std::collections::BTreeMap;

/// Length of a sha256 root hash in hex.
const ROOT_HASH_HEX_LEN: usize = 64;

/// What one verity transform wrote, and the values to open it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerityOutput {
    /// The data image the tree is built over.
    pub(super) src: PathBuf,
    /// The data image, the transform's `dest`, with the tree when it is
    /// appended.
    pub(super) data: PathBuf,
    /// Where the hash tree is: `dest` when appended, `hash_dest` otherwise.
    pub(super) hash: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) root_hash: String,
    pub(super) salt: String,
    pub(super) uuid: String,
    /// Byte offset of the verity superblock in `hash`: past the padded data
    /// when appended, 0 for a separate file.
    pub(super) hash_offset: u64,
    /// The data blocks the tree covers (the padded data over the block size).
    pub(super) data_blocks: u64,
    pub(super) data_block_size: u32,
    pub(super) hash_block_size: u32,
    pub(super) tool_path: String,
    pub(super) tool_version: Option<String>,
}

/// The verity outputs of a run, keyed by their resolved `dest` path, so an
/// archive entry's `${assembly.verity:<dest>:<field>}` finds them.
pub(super) type VerityOutputs = BTreeMap<PathBuf, VerityOutput>;

impl VerityOutput {
    /// The value of one field, as an archive entry writes it.
    pub(super) fn field(&self, field: gaia_spec::AssemblyVerityField) -> String {
        use gaia_spec::AssemblyVerityField as Field;
        match field {
            Field::RootHash => self.root_hash.clone(),
            Field::Salt => self.salt.clone(),
            Field::HashOffset => self.hash_offset.to_string(),
            Field::DataBlocks => self.data_blocks.to_string(),
            Field::DataBlockSize => self.data_block_size.to_string(),
            Field::HashBlockSize => self.hash_block_size.to_string(),
        }
    }
}

/// The data length rounded up to whole data blocks; `veritysetup` covers
/// exactly these bytes, so the image is zero-padded to them.
pub(super) fn verity_padded_len(len: u64, data_block_size: u64) -> u64 {
    len.div_ceil(data_block_size) * data_block_size
}

/// Where the appended tree's superblock goes: the first hash-block boundary
/// at or after the padded data.
pub(super) fn verity_hash_offset(padded_len: u64, hash_block_size: u64) -> u64 {
    padded_len.div_ceil(hash_block_size) * hash_block_size
}

pub(super) fn execute_assembly_verity(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    transform: &AssemblyTransformSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<VerityOutput, AssemblyError> {
    let src = transform
        .src
        .as_ref()
        .ok_or_else(|| "assembly verity transform requires src".to_string())
        .and_then(|src| roots.resolve_path(spec, src))?;
    if !src.is_file() {
        return Err(format!(
            "assembly verity source '{}' does not exist or is not a file",
            src.display()
        )
        .into());
    }
    let dest = roots.resolve_path(spec, &transform.dest)?;
    let hash_dest = transform
        .hash_dest
        .as_ref()
        .map(|template| roots.resolve_path(spec, template))
        .transpose()?;
    let tool = resolve_assembly_tool(roots, "veritysetup")?;
    tracing::Span::current().record("tool_path", tool.display.as_str());

    let data_block_size = transform.verity_data_block_size();
    let hash_block_size = transform.verity_hash_block_size();
    let data_len = file_len(&src)?;
    if data_len == 0 {
        return Err(format!("assembly verity source '{}' is empty", src.display()).into());
    }
    let source_sha256 = file_sha256(&src)?;
    let salt = match &transform.salt {
        Some(salt) => salt.to_ascii_lowercase(),
        None => source_sha256.clone(),
    };
    let uuid = match &transform.uuid {
        Some(uuid) => uuid.to_ascii_lowercase(),
        None => derived_uuid(&[b"gaia dm-verity uuid\n", source_sha256.as_bytes()]),
    };
    let padded_len = verity_padded_len(data_len, u64::from(data_block_size));
    let data_blocks = padded_len / u64::from(data_block_size);
    let hash_offset = match hash_dest {
        Some(_) => 0,
        None => verity_hash_offset(padded_len, u64::from(hash_block_size)),
    };

    for parent in [dest.parent(), hash_dest.as_deref().and_then(Path::parent)]
        .into_iter()
        .flatten()
    {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly verity output dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temp_data = temporary_assembly_output_path(&dest);
    let temp_hash = match &hash_dest {
        Some(hash_dest) => temporary_assembly_output_path(hash_dest),
        None => temp_data.clone(),
    };
    let result = (|| -> Result<String, AssemblyError> {
        copy_padded(&src, &temp_data, padded_len)?;
        if hash_dest.is_some() {
            std_fs::File::create(&temp_hash).map_err(|error| {
                format!(
                    "failed to create verity hash file '{}': {error}",
                    temp_hash.display()
                )
            })?;
        }
        let mut command = Command::new(&tool.program);
        command
            .arg("format")
            .arg(&temp_data)
            .arg(&temp_hash)
            .arg("--hash=sha256")
            .arg(format!("--data-block-size={data_block_size}"))
            .arg(format!("--hash-block-size={hash_block_size}"))
            .arg(format!("--salt={salt}"))
            .arg(format!("--uuid={uuid}"))
            .arg(format!("--hash-offset={hash_offset}"));
        let output = run_command_capture_tail(
            spec,
            &mut command,
            process_output_retention(spec),
            cancel_check.clone(),
        )?;
        if !output.status.success() {
            return Err(format!(
                "verity transform failed for '{}' using '{}': {}",
                src.display(),
                tool.display,
                output.failure_context(&command)
            )
            .into());
        }
        let report = output.stdout_tail();
        let root_hash = verity_report_value(&report, "Root hash")?;
        if !is_hex_of_len(&root_hash, ROOT_HASH_HEX_LEN) {
            return Err(format!(
                "verity transform for '{}' reported root hash '{root_hash}', which is not a sha256 hex digest",
                src.display()
            )
            .into());
        }
        let reported_blocks = verity_report_value(&report, "Data blocks")?;
        if reported_blocks != data_blocks.to_string() {
            return Err(format!(
                "verity transform for '{}' covered {reported_blocks} data blocks, expected {data_blocks}",
                src.display()
            )
            .into());
        }
        Ok(root_hash)
    })();
    let root_hash = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = std_fs::remove_file(&temp_data);
            if hash_dest.is_some() {
                let _ = std_fs::remove_file(&temp_hash);
            }
            return Err(error);
        }
    };
    publish_assembly_output(&temp_data, &dest)?;
    if let Some(hash_dest) = &hash_dest {
        publish_assembly_output(&temp_hash, hash_dest)?;
    }
    let hash = hash_dest.clone().unwrap_or_else(|| dest.clone());
    Ok(VerityOutput {
        src: src.clone(),
        data: dest.clone(),
        bytes: file_len(&dest)?,
        sha256: file_sha256(&dest)?,
        hash,
        root_hash,
        salt,
        uuid,
        hash_offset,
        data_blocks,
        data_block_size,
        hash_block_size,
        tool_path: tool.display.clone(),
        tool_version: tool_version(&tool, ["--version"]),
    })
}

/// Copies `src` to `dest`, zero-padded to `padded_len` bytes.
fn copy_padded(src: &Path, dest: &Path, padded_len: u64) -> Result<(), String> {
    std_fs::copy(src, dest).map_err(|error| {
        format!(
            "failed to copy verity source '{}' to '{}': {error}",
            src.display(),
            dest.display()
        )
    })?;
    let file = std_fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .map_err(|error| format!("failed to open '{}': {error}", dest.display()))?;
    file.set_len(padded_len).map_err(|error| {
        format!(
            "failed to pad verity data '{}' to {padded_len} bytes: {error}",
            dest.display()
        )
    })
}

/// The value after `label:` in `veritysetup`'s report (`Root hash:  <hex>`).
fn verity_report_value(report: &str, label: &str) -> Result<String, String> {
    report
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == label).then(|| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("veritysetup output has no '{label}' line"))
}

fn is_hex_of_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A UUID (version 4 layout) derived from `parts`: the first 16 bytes of
/// their sha256.
pub(super) fn derived_uuid(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verity_layout_pads_to_blocks_and_places_the_superblock_after_the_data() {
        assert_eq!(verity_padded_len(1, 4096), 4096);
        assert_eq!(verity_padded_len(4096, 4096), 4096);
        assert_eq!(verity_padded_len(4097, 4096), 8192);
        assert_eq!(verity_hash_offset(8192, 4096), 8192);
        assert_eq!(verity_hash_offset(8192, 512), 8192);
        assert_eq!(verity_hash_offset(12288, 8192), 16384);
    }

    #[test]
    fn derived_uuid_is_stable_and_well_formed() {
        let first = derived_uuid(&[b"domain", b"data"]);
        assert_eq!(first, derived_uuid(&[b"domain", b"data"]));
        assert_ne!(first, derived_uuid(&[b"domain", b"other"]));
        assert_eq!(first.len(), 36);
        assert_eq!(first.as_bytes()[14], b'4');
        assert!(matches!(first.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn report_values_are_read_by_label() {
        let report = "VERITY header information for x.img\nUUID:            \tabc\nData blocks:   \t16\nRoot hash:      \t0123\n";
        assert_eq!(
            verity_report_value(report, "Root hash").expect("root"),
            "0123"
        );
        assert_eq!(
            verity_report_value(report, "Data blocks").expect("blocks"),
            "16"
        );
        assert!(verity_report_value(report, "Salt").is_err());
    }
}
