use std::collections::HashSet;

use gaia_spec::{AssemblyArchiveMemberSourceSpec, ImageAssemblySpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// ustar `name` field length; longer names are rejected rather than split
/// into the `prefix` field.
const TAR_NAME_MAX_BYTES: usize = 100;
const ZSTD_LEVELS: std::ops::RangeInclusive<u32> = 1..=19;

pub(crate) fn validate_disk_layout(
    disk: &gaia_spec::AssemblyDiskSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.disks.partitions".into());
    let extended = disk.partitions.len() > 4;
    for (index, partition) in disk.partitions.iter().enumerate() {
        match partition.parsed_size() {
            Ok(Some(size)) if size.bytes() == 0 => diagnostics.push(error(
                "assembly_partition_size_invalid",
                format!(
                    "assembly disk '{}' partition '{}' size must be greater than zero",
                    disk.id, partition.name
                ),
                location(),
            )),
            Ok(_) => {}
            Err(parse_error) => diagnostics.push(error(
                "assembly_partition_size_invalid",
                format!(
                    "assembly disk '{}' partition '{}' has invalid size: {parse_error}",
                    disk.id, partition.name
                ),
                location(),
            )),
        }
        if partition.image.is_none() && partition.size.is_none() {
            diagnostics.push(error(
                "assembly_partition_image_or_size_required",
                format!(
                    "assembly disk '{}' partition '{}' must set image, size, or both",
                    disk.id, partition.name
                ),
                location(),
            ));
        }
        if partition.wipe && partition.image.is_some() {
            diagnostics.push(error(
                "assembly_partition_wipe_with_image",
                format!(
                    "assembly disk '{}' partition '{}' sets wipe = true and an image; wipe only applies to empty partitions",
                    disk.id, partition.name
                ),
                location(),
            ));
        }
        if !partition.materialize {
            if partition.size.is_none() {
                diagnostics.push(error(
                    "assembly_partition_unmaterialized_size_required",
                    format!(
                        "assembly disk '{}' partition '{}' sets materialize = false and no size; an unmaterialized partition needs a size to be placed in the table",
                        disk.id, partition.name
                    ),
                    location(),
                ));
            }
            if partition.image.is_some() {
                diagnostics.push(error(
                    "assembly_partition_unmaterialized_image",
                    format!(
                        "assembly disk '{}' partition '{}' sets materialize = false and an image; an unmaterialized partition is not written",
                        disk.id, partition.name
                    ),
                    location(),
                ));
            }
            if partition.wipe {
                diagnostics.push(error(
                    "assembly_partition_unmaterialized_wipe",
                    format!(
                        "assembly disk '{}' partition '{}' sets materialize = false and wipe = true; an unmaterialized partition is not written, so it cannot be wiped",
                        disk.id, partition.name
                    ),
                    location(),
                ));
            }
        }
        if extended && index >= 3 && partition.bootable {
            diagnostics.push(error(
                "assembly_partition_bootable_logical",
                format!(
                    "assembly disk '{}' partition '{}' is a logical partition (p{}); bootable is only allowed on primary partitions p1-p3 when a disk has more than 4 partitions",
                    disk.id,
                    partition.name,
                    index + 2
                ),
                location(),
            ));
        }
    }
}

pub(crate) fn validate_transform_levels(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for transform in &assembly.transforms {
        let Some(level) = transform.level else {
            continue;
        };
        if transform.kind != gaia_spec::AssemblyTransformKindSpec::Zstd {
            diagnostics.push(error(
                "assembly_transform_level_unsupported",
                format!(
                    "assembly transform kind '{}' does not accept level",
                    transform.kind.as_str()
                ),
                Some("image.assembly.transforms".into()),
            ));
        } else if !ZSTD_LEVELS.contains(&level) {
            diagnostics.push(error(
                "assembly_transform_level_invalid",
                format!("assembly zstd transform level {level} must be between 1 and 19"),
                Some("image.assembly.transforms".into()),
            ));
        }
    }
}

pub(crate) fn validate_archives(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.archives".into());
    let mut archive_ids = HashSet::new();
    for archive in &assembly.archives {
        if archive.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_archive_id_empty",
                "assembly archive id cannot be empty".into(),
                location(),
            ));
        } else if !archive_ids.insert(archive.id.as_str()) {
            diagnostics.push(error(
                "assembly_archive_duplicate",
                format!(
                    "assembly archive '{}' is declared more than once",
                    archive.id
                ),
                location(),
            ));
        }
        if archive.output.trim().is_empty() {
            diagnostics.push(error(
                "assembly_archive_output_empty",
                format!("assembly archive '{}' output cannot be empty", archive.id),
                location(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &archive.output,
                "image.assembly.archives.output",
                diagnostics,
            );
        }
        if archive.members.is_empty() {
            diagnostics.push(error(
                "assembly_archive_members_empty",
                format!("assembly archive '{}' has no members", archive.id),
                location(),
            ));
        }
        let mut names = HashSet::new();
        for member in &archive.members {
            if let Some(problem) = tar_member_name_problem(&member.name) {
                diagnostics.push(error(
                    "assembly_archive_member_name_invalid",
                    format!(
                        "assembly archive '{}' member name '{}' {problem}",
                        archive.id, member.name
                    ),
                    Some("image.assembly.archives.members".into()),
                ));
            } else if !names.insert(member.name.as_str()) {
                diagnostics.push(error(
                    "assembly_archive_member_duplicate",
                    format!(
                        "assembly archive '{}' contains member '{}' more than once",
                        archive.id, member.name
                    ),
                    Some("image.assembly.archives.members".into()),
                ));
            }
            match member.source() {
                None => diagnostics.push(error(
                    "assembly_archive_member_source_invalid",
                    format!(
                        "assembly archive '{}' member '{}' must set exactly one of src or entries",
                        archive.id, member.name
                    ),
                    Some("image.assembly.archives.members".into()),
                )),
                Some(AssemblyArchiveMemberSourceSpec::File(src)) => {
                    validate_assembly_path_template(
                        spec,
                        tree_ids,
                        src,
                        "image.assembly.archives.members.src",
                        diagnostics,
                    );
                }
                Some(AssemblyArchiveMemberSourceSpec::Generated(entries)) => {
                    validate_generated_entries(
                        spec,
                        tree_ids,
                        &archive.id,
                        &member.name,
                        entries,
                        diagnostics,
                    );
                }
            }
        }
    }
}

fn validate_generated_entries(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    archive_id: &str,
    member_name: &str,
    entries: &[(String, String)],
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.archives.generated.entries".into());
    for (key, value) in entries {
        if !is_env_key(key) {
            diagnostics.push(error(
                "assembly_archive_entry_key_invalid",
                format!(
                    "assembly archive '{archive_id}' member '{member_name}' key '{key}' must match [A-Za-z_][A-Za-z0-9_]*"
                ),
                location(),
            ));
        }
        if value.contains(['\n', '\r', '\0']) {
            diagnostics.push(error(
                "assembly_archive_entry_value_invalid",
                format!(
                    "assembly archive '{archive_id}' member '{member_name}' value for '{key}' cannot contain newlines or NUL"
                ),
                location(),
            ));
        }
        let rendered = gaia_spec::assembly_digest_tokens(value, |token| {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &token.path,
                "image.assembly.archives.generated.entries",
                diagnostics,
            );
            Ok(String::new())
        });
        if let Err(message) = rendered {
            diagnostics.push(error(
                "assembly_archive_entry_value_invalid",
                format!(
                    "assembly archive '{archive_id}' member '{member_name}' value for '{key}': {message}"
                ),
                location(),
            ));
        }
    }
}

fn tar_member_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("cannot be empty");
    }
    if name.len() > TAR_NAME_MAX_BYTES {
        return Some("is longer than the 100-byte ustar name limit");
    }
    if name.starts_with('/') || name.ends_with('/') {
        return Some("must be a relative file path");
    }
    if name
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Some("cannot contain empty, '.' or '..' path components");
    }
    if name.contains('\0') {
        return Some("cannot contain NUL");
    }
    None
}

fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
