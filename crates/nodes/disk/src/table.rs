//! The bytes of a partition table: a GPT (protective MBR, both headers and
//! both entry arrays, UEFI 2.10 section 5.3) or an MBR with up to four
//! primary entries. Each function returns the sectors to write, as
//! `(offset, bytes)`, and nothing else; the disk writer puts them in place.
//!
//! GUIDs are the mixed-endian bytes GPT stores. Derived GUIDs and the MBR
//! disk signature are hashes of names, so the same description gives the same
//! table on every host.

use crate::crc::crc32;
use gaia_store::Digest;

pub const SECTOR: u64 = 512;
/// The entry array GPT headers describe: 128 entries of 128 bytes.
pub const GPT_ENTRY_COUNT: usize = 128;
const GPT_ENTRY_BYTES: usize = 128;
const GPT_ARRAY_BYTES: usize = GPT_ENTRY_COUNT * GPT_ENTRY_BYTES;
/// Sectors of the entry array.
pub const GPT_ARRAY_SECTORS: u64 = (GPT_ARRAY_BYTES as u64) / SECTOR;
/// The sectors a GPT disk starts with: the protective MBR, the primary
/// header and the entries. The first usable LBA follows them.
pub const GPT_HEAD_SECTORS: u64 = 2 + GPT_ARRAY_SECTORS;
/// The sectors a GPT disk ends with: the backup entries and the backup header.
pub const GPT_TAIL_SECTORS: u64 = GPT_ARRAY_SECTORS + 1;
const GPT_HEADER_BYTES: usize = 92;
const GPT_REVISION_1_0: u32 = 0x0001_0000;
/// GPT attribute bit 2: legacy BIOS bootable, what `bootable` sets.
const GPT_ATTR_LEGACY_BOOTABLE: u64 = 1 << 2;
/// The longest partition name: 36 UTF-16 code units.
pub const GPT_NAME_UNITS: usize = 36;

/// One GPT partition entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptEntry {
    pub name: String,
    /// The type GUID, in disk byte order.
    pub type_guid: [u8; 16],
    /// The unique GUID, in disk byte order.
    pub guid: [u8; 16],
    pub start: u64,
    pub size: u64,
    pub bootable: bool,
}

/// One MBR primary entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbrEntry {
    pub start: u64,
    pub size: u64,
    pub type_byte: u8,
    pub bootable: bool,
}

/// `text` as a GUID's disk bytes: the first three fields little-endian, the
/// rest as written.
pub fn guid_bytes(text: &str) -> Result<[u8; 16], String> {
    let groups: Vec<&str> = text.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    if groups.len() != 5
        || groups
            .iter()
            .zip(lengths)
            .any(|(group, len)| group.len() != len || !group.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(format!("'{text}' is not a GUID (8-4-4-4-12 hex digits)"));
    }
    let hex: String = groups.concat();
    let mut bytes = [0u8; 16];
    for (index, pair) in hex.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(pair).unwrap_or("00");
        bytes[index] = u8::from_str_radix(text, 16).unwrap_or(0);
    }
    bytes[0..4].reverse();
    bytes[4..6].reverse();
    bytes[6..8].reverse();
    Ok(bytes)
}

/// A GUID derived from `parts` under `domain`: the first 16 bytes of a hash,
/// marked as a name-based (version 5) RFC 4122 GUID.
pub fn derived_guid(domain: &str, parts: &[&[u8]]) -> [u8; 16] {
    let mut input = domain.as_bytes().to_vec();
    for part in parts {
        input.extend_from_slice(part);
    }
    let hash = Digest::of_bytes(&input);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// The writes for a GPT disk of `disk_size` bytes, whose first usable LBA is
/// `first_usable` and whose entries are `entries` in order.
pub fn gpt_writes(
    disk_size: u64,
    disk_guid: [u8; 16],
    first_usable: u64,
    entries: &[GptEntry],
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    if entries.len() > GPT_ENTRY_COUNT {
        return Err(format!(
            "a GPT disk holds at most {GPT_ENTRY_COUNT} partitions, not {}",
            entries.len()
        ));
    }
    let total = disk_size / SECTOR;
    let last_lba = total - 1;
    let backup_entries_lba = total - GPT_ARRAY_SECTORS - 1;
    let array = gpt_array(entries)?;
    let layout = Header {
        disk_guid,
        first_usable,
        last_usable: backup_entries_lba - 1,
        entries_crc: crc32(&array),
    };
    let primary = header(&layout, 1, last_lba, 2);
    let backup = header(&layout, last_lba, 1, backup_entries_lba);
    Ok(vec![
        (0, protective_mbr(total)),
        (SECTOR, primary.to_vec()),
        (2 * SECTOR, array.clone()),
        (backup_entries_lba * SECTOR, array),
        (last_lba * SECTOR, backup.to_vec()),
    ])
}

struct Header {
    disk_guid: [u8; 16],
    first_usable: u64,
    last_usable: u64,
    entries_crc: u32,
}

/// One header sector. The primary and backup differ only in where each is,
/// where the other is and where the entries are.
fn header(layout: &Header, my_lba: u64, alternate_lba: u64, entries_lba: u64) -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[0..8].copy_from_slice(b"EFI PART");
    sector[8..12].copy_from_slice(&GPT_REVISION_1_0.to_le_bytes());
    sector[12..16].copy_from_slice(&(GPT_HEADER_BYTES as u32).to_le_bytes());
    sector[24..32].copy_from_slice(&my_lba.to_le_bytes());
    sector[32..40].copy_from_slice(&alternate_lba.to_le_bytes());
    sector[40..48].copy_from_slice(&layout.first_usable.to_le_bytes());
    sector[48..56].copy_from_slice(&layout.last_usable.to_le_bytes());
    sector[56..72].copy_from_slice(&layout.disk_guid);
    sector[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    sector[80..84].copy_from_slice(&(GPT_ENTRY_COUNT as u32).to_le_bytes());
    sector[84..88].copy_from_slice(&(GPT_ENTRY_BYTES as u32).to_le_bytes());
    sector[88..92].copy_from_slice(&layout.entries_crc.to_le_bytes());
    // The header CRC covers the header with its own CRC field zeroed.
    let crc = crc32(&sector[..GPT_HEADER_BYTES]);
    sector[16..20].copy_from_slice(&crc.to_le_bytes());
    sector
}

/// The entry array: the entries in order, the rest zero.
fn gpt_array(entries: &[GptEntry]) -> Result<Vec<u8>, String> {
    let mut array = vec![0u8; GPT_ARRAY_BYTES];
    for (index, entry) in entries.iter().enumerate() {
        let slot = &mut array[index * GPT_ENTRY_BYTES..][..GPT_ENTRY_BYTES];
        let first = entry.start / SECTOR;
        let last = (entry.start + entry.size) / SECTOR - 1;
        let attributes = if entry.bootable {
            GPT_ATTR_LEGACY_BOOTABLE
        } else {
            0
        };
        slot[0..16].copy_from_slice(&entry.type_guid);
        slot[16..32].copy_from_slice(&entry.guid);
        slot[32..40].copy_from_slice(&first.to_le_bytes());
        slot[40..48].copy_from_slice(&last.to_le_bytes());
        slot[48..56].copy_from_slice(&attributes.to_le_bytes());
        let units: Vec<u16> = entry.name.encode_utf16().collect();
        if units.len() > GPT_NAME_UNITS {
            return Err(format!(
                "GPT partition name '{}' is longer than {GPT_NAME_UNITS} UTF-16 code units",
                entry.name
            ));
        }
        for (unit_index, unit) in units.iter().enumerate() {
            slot[56 + unit_index * 2..][..2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    Ok(array)
}

/// A protective MBR: one 0xEE partition over the disk, capped at the 32-bit
/// sector count, so MBR-only tools see the disk as in use.
fn protective_mbr(total: u64) -> Vec<u8> {
    let mut sector = boot_sector();
    let count = u32::try_from(total - 1).unwrap_or(u32::MAX);
    set_entry(&mut sector, 0, false, 0xee, 1, count);
    sector.to_vec()
}

/// The writes for an MBR of up to four primary entries, with `signature` as
/// its disk signature.
pub fn mbr_writes(signature: [u8; 4], entries: &[MbrEntry]) -> Result<Vec<(u64, Vec<u8>)>, String> {
    if entries.len() > 4 {
        return Err(format!(
            "an MBR disk here holds 4 primary partitions, not {}; logical partitions are not built",
            entries.len()
        ));
    }
    let mut sector = boot_sector();
    sector[440..444].copy_from_slice(&signature);
    for (slot, entry) in entries.iter().enumerate() {
        let start = u32::try_from(entry.start / SECTOR)
            .map_err(|_| "a partition starts past the MBR's 32-bit sector limit".to_string())?;
        let count = u32::try_from(entry.size / SECTOR)
            .map_err(|_| "a partition is past the MBR's 32-bit sector limit".to_string())?;
        set_entry(
            &mut sector,
            slot,
            entry.bootable,
            entry.type_byte,
            start,
            count,
        );
    }
    Ok(vec![(0, sector.to_vec())])
}

fn boot_sector() -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[510] = 0x55;
    sector[511] = 0xaa;
    sector
}

/// One MBR primary entry with LBA addressing; the CHS fields carry the
/// conventional "use LBA" values.
fn set_entry(
    sector: &mut [u8],
    slot: usize,
    bootable: bool,
    type_byte: u8,
    start: u32,
    count: u32,
) {
    let at = 446 + slot * 16;
    sector[at] = if bootable { 0x80 } else { 0x00 };
    sector[at + 1..at + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    sector[at + 4] = type_byte;
    sector[at + 5..at + 8].copy_from_slice(&[0xff, 0xff, 0xff]);
    sector[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
    sector[at + 12..at + 16].copy_from_slice(&count.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_text_becomes_mixed_endian_disk_bytes() {
        let bytes = guid_bytes("C12A7328-F81F-11D2-BA4B-00A0C93EC93B").expect("guid");
        assert_eq!(
            bytes,
            [
                0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e,
                0xc9, 0x3b
            ]
        );
        assert!(guid_bytes("C12A7328F81F11D2BA4B00A0C93EC93B").is_err());
        assert!(guid_bytes("C12A732-F81F-11D2-BA4B-00A0C93EC93B").is_err());
    }

    #[test]
    fn derived_guids_are_stable_and_version_five() {
        let a = derived_guid("gaia gpt disk guid\n", &[b"demo-board"]);
        assert_eq!(a, derived_guid("gaia gpt disk guid\n", &[b"demo-board"]));
        assert_ne!(a, derived_guid("gaia gpt disk guid\n", &[b"other"]));
        assert_eq!(a[6] >> 4, 5);
        assert_eq!(a[8] & 0xc0, 0x80);
    }

    #[test]
    fn a_name_longer_than_36_units_is_refused() {
        let entry = GptEntry {
            name: "x".repeat(37),
            type_guid: [0; 16],
            guid: [0; 16],
            start: 1 << 20,
            size: 1 << 20,
            bootable: false,
        };
        assert!(gpt_writes(4 << 20, [0; 16], 34, &[entry]).is_err());
    }
}
