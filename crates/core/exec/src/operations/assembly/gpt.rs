//! GPT disks: a protective MBR, the primary header at LBA 1 with its
//! partition entries from LBA 2, and the backup entries and header in the
//! last sectors of the disk (UEFI 2.10, section 5.3).
//!
//! Reproducible: the disk GUID is derived from the disk id and each
//! partition GUID from the disk GUID and the partition name, unless the spec
//! sets them. They depend on neither the images nor the build, so a
//! `root=PARTUUID=` on a kernel command line stays valid across rebuilds.

use super::*;
use gaia_spec::GptGuid;

const SECTOR: u64 = 512;
/// The entry array the header describes: 128 entries of 128 bytes, the
/// size every tool expects (32 sectors).
const GPT_ENTRY_COUNT: u32 = 128;
const GPT_ENTRY_BYTES: u32 = 128;
const GPT_ENTRY_SECTORS: u64 = (GPT_ENTRY_COUNT as u64 * GPT_ENTRY_BYTES as u64) / SECTOR;
/// The first LBA a partition can use: past the MBR, header and entries.
pub(super) const GPT_MIN_FIRST_LBA: u64 = 2 + GPT_ENTRY_SECTORS;
/// The sectors at the end of the disk: the backup entries and header.
const GPT_BACKUP_SECTORS: u64 = GPT_ENTRY_SECTORS + 1;
const GPT_HEADER_BYTES: u32 = 92;
const GPT_REVISION_1_0: u32 = 0x0001_0000;
/// Attribute bit 2: legacy BIOS bootable, what `bootable` sets on GPT.
const GPT_ATTRIBUTE_LEGACY_BIOS_BOOTABLE: u64 = 1 << 2;
/// The longest partition name: 36 UTF-16 code units.
pub(super) const GPT_NAME_MAX_UNITS: usize = 36;
const GPT_DISK_GUID_DOMAIN: &[u8] = b"gaia gpt disk guid\n";
const GPT_PARTITION_GUID_DOMAIN: &[u8] = b"gaia gpt partition guid\n";

/// The GUIDs of one GPT partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GptPartitionIds {
    pub(super) type_guid: GptGuid,
    pub(super) guid: GptGuid,
}

/// The disk GUID: the spec's, or one derived from the disk id.
pub(super) fn gpt_disk_guid(disk: &gaia_spec::AssemblyDiskSpec) -> Result<GptGuid, String> {
    match &disk.gpt.disk_guid {
        Some(raw) => GptGuid::parse(raw)
            .map_err(|error| format!("assembly disk '{}' disk_guid {error}", disk.id)),
        None => Ok(GptGuid::from_digest(&Sha256::digest(
            [GPT_DISK_GUID_DOMAIN, disk.id.as_bytes()].concat(),
        ))),
    }
}

/// The type and unique GUID of a partition; a derived GUID hashes the disk
/// GUID and the partition name.
pub(super) fn gpt_partition_ids(
    disk: &gaia_spec::AssemblyDiskSpec,
    disk_guid: GptGuid,
    partition: &gaia_spec::AssemblyDiskPartitionSpec,
) -> Result<GptPartitionIds, String> {
    let type_guid = partition.gpt_partition_type().map_err(|error| {
        format!(
            "assembly disk '{}' partition '{}' has invalid partition type: {error}",
            disk.id, partition.name
        )
    })?;
    let guid = match &partition.gpt.guid {
        Some(raw) => GptGuid::parse(raw).map_err(|error| {
            format!(
                "assembly disk '{}' partition '{}' guid {error}",
                disk.id, partition.name
            )
        })?,
        None => {
            let mut hasher = Sha256::new();
            hasher.update(GPT_PARTITION_GUID_DOMAIN);
            hasher.update(disk_guid.to_disk_bytes());
            hasher.update(partition.name.as_bytes());
            GptGuid::from_digest(&hasher.finalize())
        }
    };
    Ok(GptPartitionIds { type_guid, guid })
}

/// Sectors of a GPT disk whose partitions end at `last_end_lba`: room for
/// the backup entries and header, rounded up to the alignment.
pub(super) fn gpt_total_sectors(last_end_lba: u64, alignment_lba: u64) -> u64 {
    align_to(last_end_lba + GPT_BACKUP_SECTORS, alignment_lba)
}

/// Writes the protective MBR and both GPT headers and entry arrays of a
/// disk of `total_sectors`.
pub(super) fn write_gpt_tables(
    file: &mut std_fs::File,
    output: &Path,
    disk_guid: GptGuid,
    total_sectors: u64,
    partitions: &[AssemblyDiskPartitionSummary],
) -> Result<(), String> {
    let entries = gpt_entries(partitions)?;
    let entries_crc = crc32(&entries);
    let last_lba = total_sectors - 1;
    let backup_entries_lba = total_sectors - GPT_BACKUP_SECTORS;
    let layout = GptHeaderLayout {
        disk_guid,
        first_usable_lba: GPT_MIN_FIRST_LBA,
        last_usable_lba: backup_entries_lba - 1,
        entries_crc,
    };
    let primary = gpt_header(&layout, 1, last_lba, 2);
    let backup = gpt_header(&layout, last_lba, 1, backup_entries_lba);
    write_at(file, output, 0, &protective_mbr(total_sectors))?;
    write_at(file, output, SECTOR, &primary)?;
    write_at(file, output, 2 * SECTOR, &entries)?;
    write_at(file, output, backup_entries_lba * SECTOR, &entries)?;
    write_at(file, output, last_lba * SECTOR, &backup)
}

struct GptHeaderLayout {
    disk_guid: GptGuid,
    first_usable_lba: u64,
    last_usable_lba: u64,
    entries_crc: u32,
}

/// One header sector. The primary and the backup differ only in where they
/// are, where the other is, and where their entries are.
fn gpt_header(
    layout: &GptHeaderLayout,
    my_lba: u64,
    alternate_lba: u64,
    entries_lba: u64,
) -> [u8; SECTOR as usize] {
    let mut sector = [0u8; SECTOR as usize];
    sector[0..8].copy_from_slice(b"EFI PART");
    sector[8..12].copy_from_slice(&GPT_REVISION_1_0.to_le_bytes());
    sector[12..16].copy_from_slice(&GPT_HEADER_BYTES.to_le_bytes());
    sector[24..32].copy_from_slice(&my_lba.to_le_bytes());
    sector[32..40].copy_from_slice(&alternate_lba.to_le_bytes());
    sector[40..48].copy_from_slice(&layout.first_usable_lba.to_le_bytes());
    sector[48..56].copy_from_slice(&layout.last_usable_lba.to_le_bytes());
    sector[56..72].copy_from_slice(&layout.disk_guid.to_disk_bytes());
    sector[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    sector[80..84].copy_from_slice(&GPT_ENTRY_COUNT.to_le_bytes());
    sector[84..88].copy_from_slice(&GPT_ENTRY_BYTES.to_le_bytes());
    sector[88..92].copy_from_slice(&layout.entries_crc.to_le_bytes());
    // The header CRC covers the header with its own CRC field zeroed.
    let header_crc = crc32(&sector[..GPT_HEADER_BYTES as usize]);
    sector[16..20].copy_from_slice(&header_crc.to_le_bytes());
    sector
}

/// The entry array: one entry per partition in order, the rest zero.
fn gpt_entries(partitions: &[AssemblyDiskPartitionSummary]) -> Result<Vec<u8>, String> {
    if partitions.len() > GPT_ENTRY_COUNT as usize {
        return Err(format!(
            "a GPT disk holds at most {GPT_ENTRY_COUNT} partitions, not {}",
            partitions.len()
        ));
    }
    let mut entries = vec![0u8; (GPT_ENTRY_COUNT * GPT_ENTRY_BYTES) as usize];
    for (index, partition) in partitions.iter().enumerate() {
        let ids = partition.gpt.ok_or_else(|| {
            format!(
                "assembly partition '{}' on a GPT disk has no GUIDs",
                partition.name
            )
        })?;
        let entry = &mut entries[index * GPT_ENTRY_BYTES as usize..][..GPT_ENTRY_BYTES as usize];
        let first_lba = u64::from(partition.start_lba);
        let last_lba = first_lba + u64::from(partition.sector_count) - 1;
        let attributes = if partition.bootable {
            GPT_ATTRIBUTE_LEGACY_BIOS_BOOTABLE
        } else {
            0
        };
        entry[0..16].copy_from_slice(&ids.type_guid.to_disk_bytes());
        entry[16..32].copy_from_slice(&ids.guid.to_disk_bytes());
        entry[32..40].copy_from_slice(&first_lba.to_le_bytes());
        entry[40..48].copy_from_slice(&last_lba.to_le_bytes());
        entry[48..56].copy_from_slice(&attributes.to_le_bytes());
        let name = gpt_name_units(&partition.name)?;
        for (unit_index, unit) in name.iter().enumerate() {
            entry[56 + unit_index * 2..][..2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    Ok(entries)
}

/// A partition name as GPT stores it: UTF-16LE, at most 36 code units.
pub(super) fn gpt_name_units(name: &str) -> Result<Vec<u16>, String> {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() > GPT_NAME_MAX_UNITS {
        return Err(format!(
            "GPT partition name '{name}' is longer than {GPT_NAME_MAX_UNITS} UTF-16 code units"
        ));
    }
    Ok(units)
}

/// An MBR with one partition of type 0xEE over the whole disk (capped at
/// the 32-bit limit), so MBR-only tools see the disk as in use.
fn protective_mbr(total_sectors: u64) -> [u8; SECTOR as usize] {
    let mut sector = [0u8; SECTOR as usize];
    let entry = &mut sector[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0xee;
    entry[5..8].copy_from_slice(&[0xff, 0xff, 0xff]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let count = u32::try_from(total_sectors - 1).unwrap_or(u32::MAX);
    entry[12..16].copy_from_slice(&count.to_le_bytes());
    sector[510] = 0x55;
    sector[511] = 0xaa;
    sector
}

fn write_at(
    file: &mut std_fs::File,
    output: &Path,
    offset: u64,
    bytes: &[u8],
) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.write_all(bytes))
        .map_err(|error| {
            format!(
                "failed to write GPT data at offset {offset} of assembly disk '{}': {error}",
                output.display()
            )
        })
}

/// CRC-32 (IEEE 802.3, reflected), the checksum GPT headers use.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 != 0 {
                    0xedb8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    !bytes.iter().fold(!0u32, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_disk_ends_past_the_backup_rounded_to_the_alignment() {
        assert_eq!(gpt_total_sectors(4096, 2048), 6144);
        assert_eq!(gpt_total_sectors(4096, 1), 4096 + 33);
    }

    #[test]
    fn names_longer_than_36_units_are_rejected() {
        assert_eq!(gpt_name_units("root").expect("short").len(), 4);
        assert!(gpt_name_units(&"x".repeat(37)).is_err());
    }
}
