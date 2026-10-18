use std::collections::HashSet;

use gaia_spec::{AssemblyArchiveMemberSourceSpec, ImageAssemblySpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::{validate_assembly_path_template, validate_exclude};
use crate::image_assembly_collected::validate_collected_reference;

/// Longest tar member name: names past the ustar `name` field use the
/// `prefix` field or PAX records (see the assembly tar writer).
const TAR_NAME_MAX_BYTES: usize = 4096;
const GZIP_LEVELS: std::ops::RangeInclusive<u32> = 1..=9;
const ZSTD_LEVELS: std::ops::RangeInclusive<u32> = 1..=19;
const XZ_LEVELS: std::ops::RangeInclusive<u32> = 0..=9;

pub(crate) fn validate_disk_layout(
    disk: &gaia_spec::AssemblyDiskSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.disks.partitions".into());
    let extended = disk.partition_table == gaia_spec::AssemblyPartitionTableSpec::Mbr
        && disk.partitions.len() > 4;
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
        let (kind, range) = match transform.kind {
            gaia_spec::AssemblyTransformKindSpec::Gzip => ("gzip", GZIP_LEVELS),
            gaia_spec::AssemblyTransformKindSpec::Zstd => ("zstd", ZSTD_LEVELS),
            gaia_spec::AssemblyTransformKindSpec::Xz => ("xz", XZ_LEVELS),
            _ => {
                diagnostics.push(error(
                    "assembly_transform_level_unsupported",
                    format!(
                        "assembly transform kind '{}' does not accept level",
                        transform.kind.as_str()
                    ),
                    Some("image.assembly.transforms".into()),
                ));
                continue;
            }
        };
        if !range.contains(&level) {
            diagnostics.push(error(
                "assembly_transform_level_invalid",
                format!(
                    "assembly {kind} transform level {level} must be between {} and {}",
                    range.start(),
                    range.end()
                ),
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
            let member_label = member_label(member);
            validate_exclude(
                &member.exclude,
                matches!(
                    member.source(),
                    Some(AssemblyArchiveMemberSourceSpec::Tree(_))
                        | Some(AssemblyArchiveMemberSourceSpec::Glob(_))
                ),
                &format!(
                    "assembly archive '{}' member '{member_label}' exclude",
                    archive.id
                ),
                "image.assembly.archives.members",
                diagnostics,
            );
            let prefix_allowed = matches!(
                member.source(),
                Some(AssemblyArchiveMemberSourceSpec::Tree(_))
                    | Some(AssemblyArchiveMemberSourceSpec::Dir(_))
            );
            if !member.prefix.is_empty() {
                if !prefix_allowed {
                    diagnostics.push(error(
                        "assembly_archive_member_prefix_invalid",
                        format!(
                            "assembly archive '{}' member '{member_label}' sets prefix, which only applies to tree or src_dir members",
                            archive.id
                        ),
                        Some("image.assembly.archives.members".into()),
                    ));
                } else if !member.prefix.ends_with('/')
                    || tar_member_name_problem(member.prefix.trim_end_matches('/')).is_some()
                {
                    diagnostics.push(error(
                        "assembly_archive_member_prefix_invalid",
                        format!(
                            "assembly archive '{}' member '{member_label}' prefix '{}' must be a relative directory path ending in '/'",
                            archive.id, member.prefix
                        ),
                        Some("image.assembly.archives.members".into()),
                    ));
                }
            }
            if let Some(problem) = member_name_problem(member) {
                diagnostics.push(error(
                    "assembly_archive_member_name_invalid",
                    format!(
                        "assembly archive '{}' member name '{}' {problem}",
                        archive.id, member.name
                    ),
                    Some("image.assembly.archives.members".into()),
                ));
            } else if !member.name.is_empty() && !names.insert(member.name.as_str()) {
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
                        "assembly archive '{}' member '{member_label}' must set exactly one of src, entries, json, tree, src_dir or src_glob",
                        archive.id
                    ),
                    Some("image.assembly.archives.members".into()),
                )),
                Some(AssemblyArchiveMemberSourceSpec::Tree(tree)) => {
                    if !tree_ids.contains(tree) {
                        diagnostics.push(error(
                            "assembly_archive_member_tree_unknown",
                            format!(
                                "assembly archive '{}' member '{member_label}' names tree '{}', which is not defined",
                                archive.id,
                                tree.as_str()
                            ),
                            Some("image.assembly.archives.members.tree".into()),
                        ));
                    }
                }
                Some(AssemblyArchiveMemberSourceSpec::Dir(dir)) => {
                    validate_assembly_path_template(
                        spec,
                        tree_ids,
                        dir,
                        "image.assembly.archives.members.src_dir",
                        diagnostics,
                    );
                    validate_collected_reference(
                        spec,
                        dir,
                        &format!("archives[{}].members[{member_label}].src_dir", archive.id),
                        "image.assembly.archives.members",
                        diagnostics,
                    );
                }
                Some(AssemblyArchiveMemberSourceSpec::Glob(pattern)) => {
                    validate_assembly_path_template(
                        spec,
                        tree_ids,
                        pattern,
                        "image.assembly.archives.members.src_glob",
                        diagnostics,
                    );
                    validate_collected_reference(
                        spec,
                        pattern,
                        &format!("archives[{}].members[{member_label}].src_glob", archive.id),
                        "image.assembly.archives.members",
                        diagnostics,
                    );
                }
                Some(AssemblyArchiveMemberSourceSpec::File(src)) => {
                    validate_assembly_path_template(
                        spec,
                        tree_ids,
                        src,
                        "image.assembly.archives.members.src",
                        diagnostics,
                    );
                    validate_collected_reference(
                        spec,
                        src,
                        &format!("archives[{}].members[{}].src", archive.id, member.name),
                        "image.assembly.archives.members",
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
                Some(AssemblyArchiveMemberSourceSpec::Json(json)) => {
                    for text in json.strings() {
                        crate::image_assembly_generated::validate_json_tokens(
                            spec,
                            tree_ids,
                            text,
                            "image.assembly.archives.generated.json",
                            &format!(
                                "assembly archive '{}' member '{}' json",
                                archive.id, member.name
                            ),
                            diagnostics,
                        );
                    }
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
        let rendered = gaia_spec::assembly_value_tokens(value, |token| {
            validate_assembly_path_template(
                spec,
                tree_ids,
                token.path(),
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

/// How a member is named in messages: its name, else its source.
fn member_label(member: &gaia_spec::AssemblyArchiveMemberSpec) -> String {
    if !member.name.is_empty() {
        return member.name.clone();
    }
    match member.source() {
        Some(AssemblyArchiveMemberSourceSpec::Tree(tree)) => format!("tree {}", tree.as_str()),
        Some(AssemblyArchiveMemberSourceSpec::Dir(dir)) => format!("src_dir {}", dir.as_str()),
        _ => String::new(),
    }
}

/// `name` is the archived path, except for tree and directory members (where
/// it may be left empty and `prefix` is used) and globs (where it is a
/// directory prefix ending in `/`).
fn member_name_problem(member: &gaia_spec::AssemblyArchiveMemberSpec) -> Option<&'static str> {
    match member.source() {
        Some(AssemblyArchiveMemberSourceSpec::Tree(_))
        | Some(AssemblyArchiveMemberSourceSpec::Dir(_))
            if member.name.is_empty() =>
        {
            None
        }
        Some(AssemblyArchiveMemberSourceSpec::Glob(_)) => {
            if !member.name.ends_with('/') {
                Some("must be a directory prefix ending in '/' for src_glob members")
            } else {
                tar_member_name_problem(member.name.trim_end_matches('/'))
            }
        }
        _ => tar_member_name_problem(&member.name),
    }
}

fn tar_member_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("cannot be empty");
    }
    if name.len() > TAR_NAME_MAX_BYTES {
        return Some("is longer than the 4096-byte tar member name limit");
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
