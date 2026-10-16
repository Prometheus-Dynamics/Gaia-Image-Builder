//! Options that only some transform and filesystem kinds accept: the
//! dm-verity options of `verity` transforms and the ext4 options of `ext4`
//! filesystems. Also checks that every `${assembly.verity:...}` archive
//! token names the output of a verity transform.

use std::collections::HashSet;

use gaia_spec::{
    AssemblyFilesystemKindSpec, AssemblyTransformKindSpec, AssemblyValueToken, ImageAssemblySpec,
    ResolvedBuildSpec, assembly_value_tokens,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// ext4 labels are at most 16 bytes.
const EXT4_LABEL_MAX_BYTES: usize = 16;
/// ext4 images are whole blocks.
const EXT4_BLOCK_BYTES: u64 = 4096;
/// The smallest and largest dm-verity block size: a power of two that the
/// kernel accepts for a data or hash device.
const VERITY_BLOCK_SIZE_MIN: u32 = 512;
const VERITY_BLOCK_SIZE_MAX: u32 = 4096;
/// The longest salt dm-verity accepts, in bytes.
const VERITY_SALT_MAX_BYTES: usize = 256;

pub(crate) fn validate_transform_options(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.transforms".into());
    for transform in &assembly.transforms {
        let kind = transform.kind.as_str();
        if transform.kind != AssemblyTransformKindSpec::Verity {
            for (set, option) in [
                (transform.hash_dest.is_some(), "hash_dest"),
                (transform.data_block_size.is_some(), "data_block_size"),
                (transform.hash_block_size.is_some(), "hash_block_size"),
                (transform.salt.is_some(), "salt"),
                (transform.uuid.is_some(), "uuid"),
            ] {
                if set {
                    diagnostics.push(error(
                        "assembly_transform_option_unsupported",
                        format!(
                            "assembly transform kind '{kind}' does not accept {option}; only kind 'verity' does"
                        ),
                        location(),
                    ));
                }
            }
            continue;
        }
        if let Some(hash_dest) = &transform.hash_dest {
            validate_assembly_path_template(
                spec,
                tree_ids,
                hash_dest,
                "image.assembly.transforms.hash_dest",
                diagnostics,
            );
            if hash_dest.as_str().trim() == transform.dest.as_str().trim() {
                diagnostics.push(error(
                    "assembly_verity_hash_dest_same_as_dest",
                    format!(
                        "assembly verity transform to '{}' sets hash_dest to the same path; the tree is appended to dest when hash_dest is unset",
                        transform.dest
                    ),
                    location(),
                ));
            }
        }
        for (value, option) in [
            (transform.data_block_size, "data_block_size"),
            (transform.hash_block_size, "hash_block_size"),
        ] {
            if let Some(size) = value
                && !is_verity_block_size(size)
            {
                diagnostics.push(error(
                    "assembly_verity_block_size_invalid",
                    format!(
                        "assembly verity transform to '{}' {option} {size} must be a power of two from {VERITY_BLOCK_SIZE_MIN} to {VERITY_BLOCK_SIZE_MAX}",
                        transform.dest
                    ),
                    location(),
                ));
            }
        }
        if let Some(salt) = &transform.salt
            && !is_verity_salt(salt)
        {
            diagnostics.push(error(
                "assembly_verity_salt_invalid",
                format!(
                    "assembly verity transform to '{}' salt must be hex of 1 to {VERITY_SALT_MAX_BYTES} bytes",
                    transform.dest
                ),
                location(),
            ));
        }
        if let Some(uuid) = &transform.uuid
            && !is_uuid(uuid)
        {
            diagnostics.push(error(
                "assembly_verity_uuid_invalid",
                format!(
                    "assembly verity transform to '{}' uuid '{uuid}' must be a UUID such as 8-4-4-4-12 hex digits",
                    transform.dest
                ),
                location(),
            ));
        }
    }
}

pub(crate) fn validate_filesystem_options(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.filesystems".into());
    for filesystem in &assembly.filesystems {
        if filesystem.kind != AssemblyFilesystemKindSpec::Ext4 {
            for (set, option) in [
                (filesystem.label.is_some(), "label"),
                (filesystem.uuid.is_some(), "uuid"),
                (filesystem.journal.is_some(), "journal"),
            ] {
                if set {
                    diagnostics.push(error(
                        "assembly_filesystem_option_unsupported",
                        format!(
                            "assembly filesystem '{}' kind '{}' does not accept {option}; only kind 'ext4' does",
                            filesystem.id,
                            filesystem.kind.as_str()
                        ),
                        location(),
                    ));
                }
            }
            continue;
        }
        if filesystem.size.is_none() {
            diagnostics.push(error(
                "assembly_filesystem_size_required",
                format!(
                    "assembly filesystem '{}' kind 'ext4' requires size: a byte size, or \"auto\" to size it from the tree",
                    filesystem.id
                ),
                location(),
            ));
        }
        if let Ok(Some(size)) = filesystem.parsed_size()
            && !size.bytes().is_multiple_of(EXT4_BLOCK_BYTES)
        {
            diagnostics.push(error(
                "assembly_filesystem_size_unaligned",
                format!(
                    "assembly filesystem '{}' size {} must be a multiple of {EXT4_BLOCK_BYTES} bytes",
                    filesystem.id,
                    size.bytes()
                ),
                location(),
            ));
        }
        if let Some(label) = &filesystem.label
            && !is_ext4_label(label)
        {
            diagnostics.push(error(
                "assembly_filesystem_label_invalid",
                format!(
                    "assembly filesystem '{}' label must be 1 to {EXT4_LABEL_MAX_BYTES} bytes without '/' or NUL",
                    filesystem.id
                ),
                location(),
            ));
        }
        if let Some(uuid) = &filesystem.uuid
            && !is_uuid(uuid)
        {
            diagnostics.push(error(
                "assembly_filesystem_uuid_invalid",
                format!(
                    "assembly filesystem '{}' uuid '{uuid}' must be a UUID such as 8-4-4-4-12 hex digits",
                    filesystem.id
                ),
                location(),
            ));
        }
    }
}

/// Every `${assembly.verity:<path>:<field>}` archive token must name the
/// `dest` of a verity transform, written the same way.
pub(crate) fn validate_verity_token_targets(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let verity_dests: HashSet<&str> = assembly
        .transforms
        .iter()
        .filter(|transform| transform.kind == AssemblyTransformKindSpec::Verity)
        .map(|transform| transform.dest.as_str().trim())
        .collect();
    for archive in &assembly.archives {
        for member in &archive.members {
            for (key, value) in member.entries.iter().flatten() {
                let _ = assembly_value_tokens(value, |token| {
                    if let AssemblyValueToken::Verity(verity) = token
                        && !verity_dests.contains(verity.path.as_str().trim())
                    {
                        diagnostics.push(error(
                            "assembly_archive_verity_target_unknown",
                            format!(
                                "assembly archive '{}' member '{}' entry '{key}' reads verity field '{}' of '{}', which no verity transform writes; use the transform's dest exactly as written",
                                archive.id,
                                member.name,
                                verity.field.as_str(),
                                verity.path
                            ),
                            Some("image.assembly.archives.generated.entries".into()),
                        ));
                    }
                    Ok(String::new())
                });
            }
        }
    }
}

fn is_verity_block_size(size: u32) -> bool {
    size.is_power_of_two() && (VERITY_BLOCK_SIZE_MIN..=VERITY_BLOCK_SIZE_MAX).contains(&size)
}

fn is_verity_salt(salt: &str) -> bool {
    !salt.is_empty()
        && salt.len().is_multiple_of(2)
        && salt.len() / 2 <= VERITY_SALT_MAX_BYTES
        && salt.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_ext4_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= EXT4_LABEL_MAX_BYTES && !label.contains(['/', '\0'])
}

/// `8-4-4-4-12` hex digits.
pub(crate) fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
