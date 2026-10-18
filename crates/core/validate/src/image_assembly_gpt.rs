//! The options that belong to one partition table: GPT GUIDs, types and
//! names on `gpt` disks, and the MBR-only signature and EBR placement.

use std::collections::HashSet;

use gaia_spec::{
    AssemblyDiskSpec, AssemblyEbrPlacementSpec, AssemblyPartitionTableSpec, GptGuid,
    GptPartitionTypeError,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

/// Entries in the GPT entry array Gaia writes.
const GPT_MAX_PARTITIONS: usize = 128;
/// The MBR, the header and 32 sectors of entries come first.
const GPT_MIN_FIRST_LBA: u64 = 34;
/// A GPT partition name holds 36 UTF-16 code units.
const GPT_NAME_MAX_UNITS: usize = 36;

pub(crate) fn validate_disk_partition_table(
    disk: &AssemblyDiskSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.disks".into());
    let unsupported = |option: &str, table: &str| {
        error(
            "assembly_disk_option_unsupported",
            format!(
                "assembly disk '{}' sets {option}, which only applies to partition_table = \"{table}\"",
                disk.id
            ),
            location(),
        )
    };
    if disk.partition_table == AssemblyPartitionTableSpec::Mbr {
        if disk.gpt.disk_guid.is_some() {
            diagnostics.push(unsupported("disk_guid", "gpt"));
        }
        for partition in &disk.partitions {
            if partition.gpt.guid.is_some() {
                diagnostics.push(unsupported(
                    &format!("guid on partition '{}'", partition.name),
                    "gpt",
                ));
            }
        }
        return;
    }
    for (set, option) in [
        (disk.signature.is_some(), "signature"),
        (disk.signature_text.is_some(), "signature_text"),
        (
            disk.ebr_placement == AssemblyEbrPlacementSpec::Packed,
            "ebr_placement",
        ),
    ] {
        if set {
            diagnostics.push(unsupported(option, "mbr"));
        }
    }
    if disk.partitions.len() > GPT_MAX_PARTITIONS {
        diagnostics.push(error(
            "assembly_disk_partition_count_invalid",
            format!(
                "assembly disk '{}' has {} partitions; a GPT disk holds at most {GPT_MAX_PARTITIONS}",
                disk.id,
                disk.partitions.len()
            ),
            location(),
        ));
    }
    if let Some(first_lba) = disk.first_lba
        && first_lba < GPT_MIN_FIRST_LBA
    {
        diagnostics.push(error(
            "assembly_disk_first_lba_invalid",
            format!(
                "assembly disk '{}' first_lba {first_lba} overlaps the GPT header and entries; it must be at least {GPT_MIN_FIRST_LBA}",
                disk.id
            ),
            location(),
        ));
    }
    if let Some(guid) = &disk.gpt.disk_guid
        && let Err(parse_error) = GptGuid::parse(guid)
    {
        diagnostics.push(error(
            "assembly_disk_guid_invalid",
            format!("assembly disk '{}' disk_guid {parse_error}", disk.id),
            location(),
        ));
    }

    let location = || Some("image.assembly.disks.partitions".into());
    let mut names = HashSet::new();
    let mut guids = HashSet::new();
    for partition in &disk.partitions {
        // A derived partition GUID hashes the name, so names must differ.
        if !names.insert(partition.name.as_str()) {
            diagnostics.push(error(
                "assembly_partition_name_duplicate",
                format!(
                    "assembly disk '{}' has more than one partition named '{}'; GPT partition names must be unique",
                    disk.id, partition.name
                ),
                location(),
            ));
        }
        if partition.name.encode_utf16().count() > GPT_NAME_MAX_UNITS {
            diagnostics.push(error(
                "assembly_partition_name_invalid",
                format!(
                    "assembly disk '{}' partition name '{}' is longer than {GPT_NAME_MAX_UNITS} UTF-16 code units",
                    disk.id, partition.name
                ),
                location(),
            ));
        }
        if let Some(guid) = &partition.gpt.guid {
            match GptGuid::parse(guid) {
                Ok(parsed) if !guids.insert(parsed) => diagnostics.push(error(
                    "assembly_partition_guid_duplicate",
                    format!(
                        "assembly disk '{}' partition '{}' guid {guid} is used by another partition",
                        disk.id, partition.name
                    ),
                    location(),
                )),
                Ok(_) => {}
                Err(parse_error) => diagnostics.push(error(
                    "assembly_partition_guid_invalid",
                    format!(
                        "assembly disk '{}' partition '{}' guid {parse_error}",
                        disk.id, partition.name
                    ),
                    location(),
                )),
            }
        }
        if partition.kind.is_some() && partition.type_alias.is_some() {
            // Reported once for both tables by the caller.
            continue;
        }
        match partition.gpt_partition_type() {
            Ok(_) => {}
            Err(GptPartitionTypeError::InvalidRaw(raw)) => diagnostics.push(error(
                "assembly_partition_type_raw_invalid",
                format!(
                    "assembly disk '{}' partition '{}' type '{raw}' must be a GUID of 8-4-4-4-12 hex digits on a GPT disk",
                    disk.id, partition.name
                ),
                location(),
            )),
            Err(GptPartitionTypeError::UnknownAlias(alias)) => diagnostics.push(error(
                "assembly_partition_type_alias_invalid",
                format!(
                    "assembly disk '{}' partition '{}' uses unknown GPT type_alias '{alias}'",
                    disk.id, partition.name
                ),
                location(),
            )),
        }
    }
}
