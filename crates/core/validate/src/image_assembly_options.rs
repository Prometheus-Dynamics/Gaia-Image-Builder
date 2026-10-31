//! Options that only some transform and filesystem kinds accept: the
//! dm-verity options of `verity` transforms and the ext4 options of `ext4`
//! filesystems. Also checks that every `${assembly.verity:...}` archive
//! token names the output of a verity transform.

use std::collections::HashSet;

use gaia_spec::{
    AssemblyFilesystemKindSpec, AssemblyTransformKindSpec, AssemblyValueToken, ImageAssemblySpec,
    ResolvedBuildSpec, VFAT_FAT_SIZES, assembly_value_tokens,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// ext4 labels are at most 16 bytes.
const EXT4_LABEL_MAX_BYTES: usize = 16;
/// ext4 images are whole blocks.
const EXT4_BLOCK_BYTES: u64 = 4096;
/// The largest `reserved_percent` an ext4 filesystem accepts.
const EXT4_MAX_RESERVED_PERCENT: u32 = 50;
/// The smallest and largest dm-verity block size: a power of two that the
/// kernel accepts for a data or hash device.
const VERITY_BLOCK_SIZE_MIN: u32 = 512;
const VERITY_BLOCK_SIZE_MAX: u32 = 4096;
/// The longest salt dm-verity accepts, in bytes.
const VERITY_SALT_MAX_BYTES: usize = 256;
/// The smallest U-Boot environment that holds anything: the CRC, the flags
/// byte of a redundant one, and the two NULs of an empty one.
const UBOOT_ENV_SIZE_MIN: u64 = 7;

pub(crate) fn validate_transform_options(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.transforms".into());
    for transform in &assembly.transforms {
        let kind = transform.kind.as_str();
        if transform.long && transform.kind != AssemblyTransformKindSpec::Zstd {
            diagnostics.push(error(
                "assembly_transform_option_unsupported",
                format!(
                    "assembly transform kind '{kind}' does not accept long; only kind 'zstd' does"
                ),
                location(),
            ));
        }
        if transform.kind == AssemblyTransformKindSpec::UbootEnv {
            match transform.env_size.as_deref().map(str::parse::<gaia_spec::ByteSize>) {
                None => diagnostics.push(error(
                    "assembly_uboot_env_size_invalid",
                    format!(
                        "assembly uboot-env transform to '{}' needs env_size, the board's CONFIG_ENV_SIZE",
                        transform.dest
                    ),
                    location(),
                )),
                Some(Err(parse_error)) => diagnostics.push(error(
                    "assembly_uboot_env_size_invalid",
                    format!(
                        "assembly uboot-env transform to '{}' has invalid env_size: {parse_error}",
                        transform.dest
                    ),
                    location(),
                )),
                Some(Ok(size)) if size.bytes() < UBOOT_ENV_SIZE_MIN => diagnostics.push(error(
                    "assembly_uboot_env_size_invalid",
                    format!(
                        "assembly uboot-env transform to '{}' env_size {} is too small to hold an environment",
                        transform.dest,
                        size.bytes()
                    ),
                    location(),
                )),
                Some(Ok(_)) => {}
            }
        } else {
            for (set, option) in [
                (transform.env_size.is_some(), "env_size"),
                (transform.redundant, "redundant"),
            ] {
                if set {
                    diagnostics.push(error(
                        "assembly_transform_option_unsupported",
                        format!(
                            "assembly transform kind '{kind}' does not accept {option}; only kind 'uboot-env' does"
                        ),
                        location(),
                    ));
                }
            }
        }
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
        let erofs = filesystem.kind == AssemblyFilesystemKindSpec::Erofs;
        if let Some(percent) = filesystem.reserved_percent {
            if filesystem.kind != AssemblyFilesystemKindSpec::Ext4 {
                diagnostics.push(error(
                    "assembly_filesystem_option_unsupported",
                    format!(
                        "assembly filesystem '{}' kind '{}' does not accept reserved_percent; only kind 'ext4' does",
                        filesystem.id,
                        filesystem.kind.as_str()
                    ),
                    location(),
                ));
            } else if percent > EXT4_MAX_RESERVED_PERCENT {
                diagnostics.push(error(
                    "assembly_filesystem_reserved_percent_invalid",
                    format!(
                        "assembly filesystem '{}' reserved_percent {percent} must be 0 to {EXT4_MAX_RESERVED_PERCENT}",
                        filesystem.id
                    ),
                    location(),
                ));
            }
        }
        if let Some(fat_size) = filesystem.fat_size {
            if filesystem.kind != AssemblyFilesystemKindSpec::Vfat {
                diagnostics.push(error(
                    "assembly_filesystem_option_unsupported",
                    format!(
                        "assembly filesystem '{}' kind '{}' does not accept fat_size; only kind 'vfat' does",
                        filesystem.id,
                        filesystem.kind.as_str()
                    ),
                    location(),
                ));
            } else if !VFAT_FAT_SIZES.contains(&fat_size) {
                diagnostics.push(error(
                    "assembly_filesystem_fat_size_invalid",
                    format!(
                        "assembly filesystem '{}' fat_size {fat_size} must be 12, 16 or 32",
                        filesystem.id
                    ),
                    location(),
                ));
            }
        }
        if filesystem.kind != AssemblyFilesystemKindSpec::Ext4 {
            for (set, option, kinds) in [
                (
                    filesystem.label.is_some() && !erofs,
                    "label",
                    "'ext4' and 'erofs'",
                ),
                (
                    filesystem.uuid.is_some() && !erofs,
                    "uuid",
                    "'ext4' and 'erofs'",
                ),
                (filesystem.journal.is_some(), "journal", "'ext4'"),
            ] {
                if set {
                    diagnostics.push(error(
                        "assembly_filesystem_option_unsupported",
                        format!(
                            "assembly filesystem '{}' kind '{}' does not accept {option}; only kinds {kinds} do",
                            filesystem.id,
                            filesystem.kind.as_str()
                        ),
                        location(),
                    ));
                }
            }
        }
        if filesystem.kind == AssemblyFilesystemKindSpec::Ext4 && filesystem.size.is_none() {
            diagnostics.push(error(
                "assembly_filesystem_size_required",
                format!(
                    "assembly filesystem '{}' kind 'ext4' requires size: a byte size, or \"auto\" to size it from the tree",
                    filesystem.id
                ),
                location(),
            ));
        }
        if filesystem.kind != AssemblyFilesystemKindSpec::Ext4 && !erofs {
            continue;
        }
        if let Ok(Some(size)) = filesystem.parsed_size()
            && filesystem.kind == AssemblyFilesystemKindSpec::Ext4
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

/// Every `${assembly.verity:<path>:<field>}` token, in archive entries,
/// archive JSON and generated files, must name the `dest` of a verity
/// transform, written the same way.
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
    for site in crate::image_assembly_generated::token_sites(assembly) {
        let _ = assembly_value_tokens(site.text, |token| {
            if let AssemblyValueToken::Verity(verity) = token
                && !verity_dests.contains(verity.path.as_str().trim())
            {
                diagnostics.push(error(
                    "assembly_archive_verity_target_unknown",
                    format!(
                        "{} reads verity field '{}' of '{}', which no verity transform writes; use the transform's dest exactly as written",
                        site.label,
                        verity.field.as_str(),
                        verity.path
                    ),
                    Some(site.location.into()),
                ));
            }
            Ok(String::new())
        });
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
