//! Where everything on a disk goes, checked before a byte is written.
//!
//! Partitions are placed in order: each starts at its `offset`, or at the
//! next `alignment` boundary after the one before it, and takes its `size`,
//! or its content rounded up to a sector. Blobs are placed at their offsets.
//! Nothing may overlap, and the partition table's own sectors are kept clear:
//! the first sectors of a GPT disk (the protective MBR, the header and the
//! entries) and the backup entries and header at its end, or the sector 0 of
//! an MBR. A refused description says which rule it broke.

use crate::TableScheme;
use crate::table::{GPT_HEAD_SECTORS, GPT_TAIL_SECTORS, SECTOR};
use gaia_types::PartitionRole;

const MIB: u64 = 1024 * 1024;
/// Where partitions start when the table names no `first`.
pub const DEFAULT_FIRST: u64 = MIB;
/// The boundary partitions start on when the table names no `alignment`.
pub const DEFAULT_ALIGNMENT: u64 = MIB;
/// The most partitions an MBR holds here: its four primary entries.
pub const MBR_PRIMARY_MAX: usize = 4;
/// The most partitions a GPT holds: its entry array.
pub const GPT_PARTITION_MAX: usize = crate::table::GPT_ENTRY_COUNT;

/// One partition as the layout needs it: its description and the length of
/// its content, when it has any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionInput {
    pub name: String,
    pub role: PartitionRole,
    pub table_type: String,
    pub bootable: bool,
    pub offset: Option<u64>,
    pub size: Option<u64>,
    pub content_len: Option<u64>,
}

/// One raw blob: bytes written at a fixed offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInput {
    pub name: String,
    pub offset: u64,
    pub len: u64,
}

/// A partition with its place: `start` and `size` in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub start: u64,
    pub size: u64,
}

/// The whole layout: the disk's size and where each partition and blob is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub scheme: TableScheme,
    pub size: u64,
    pub first: u64,
    pub alignment: u64,
    pub partitions: Vec<Placed>,
    pub blobs: Vec<Placed>,
}

/// Where a GPT disk's backup entries and header start, in bytes from the
/// disk's end.
pub const GPT_TAIL_BYTES: u64 = GPT_TAIL_SECTORS * SECTOR;

/// Places `partitions` and `blobs` on a disk of `scheme`, with `first` and
/// `alignment` (bytes; `None` takes the defaults) and `size` (`None` makes the
/// disk exactly as large as its partitions and, for GPT, its backup table).
pub fn layout(
    scheme: TableScheme,
    first: Option<u64>,
    alignment: Option<u64>,
    size: Option<u64>,
    partitions: &[PartitionInput],
    blobs: &[BlobInput],
) -> Result<Layout, String> {
    let first = first.unwrap_or(DEFAULT_FIRST);
    let alignment = alignment.unwrap_or(DEFAULT_ALIGNMENT);
    let reserved = match scheme {
        TableScheme::Gpt => GPT_HEAD_SECTORS * SECTOR,
        TableScheme::Mbr => SECTOR,
    };
    for (what, value) in [("first", first), ("alignment", alignment)] {
        if value == 0 || !value.is_multiple_of(SECTOR) {
            return Err(format!(
                "{what} {value} is not a whole number of 512-byte sectors"
            ));
        }
    }
    if first < reserved {
        return Err(format!(
            "first {first} is inside the partition table, which takes the first {reserved} bytes"
        ));
    }
    let limit = match scheme {
        TableScheme::Gpt => GPT_PARTITION_MAX,
        TableScheme::Mbr => MBR_PRIMARY_MAX,
    };
    if partitions.len() > limit {
        return Err(format!(
            "{} partitions do not fit this table, which holds {limit}",
            partitions.len()
        ));
    }

    let mut placed = Vec::with_capacity(partitions.len());
    let mut cursor = first;
    for partition in partitions {
        let who = format!("partition '{}'", partition.name);
        let start = match partition.offset {
            Some(offset) => offset,
            None => align_up(cursor, alignment),
        };
        if !start.is_multiple_of(SECTOR) {
            return Err(format!("{who} starts at {start}, not on a sector"));
        }
        if start < cursor {
            return Err(format!(
                "{who} starts at {start}, inside the space before it, which ends at {cursor}"
            ));
        }
        let size = match (partition.size, partition.content_len) {
            (Some(size), Some(len)) if len > size => {
                return Err(format!("{who} is {size} bytes, but its content is {len}"));
            }
            (Some(size), _) => size,
            (None, Some(len)) => align_up(len, SECTOR),
            (None, None) => {
                return Err(format!("{who} needs a size, or content to size it by"));
            }
        };
        if size == 0 || !size.is_multiple_of(SECTOR) {
            return Err(format!(
                "{who} is {size} bytes, not a whole number of sectors"
            ));
        }
        if scheme == TableScheme::Mbr
            && (start / SECTOR > u64::from(u32::MAX) || size / SECTOR > u64::from(u32::MAX))
        {
            return Err(format!("{who} is past the MBR's 32-bit sector limit"));
        }
        cursor = start + size;
        placed.push(Placed { start, size });
    }

    let required = match scheme {
        TableScheme::Gpt => align_up(cursor + GPT_TAIL_BYTES, alignment),
        TableScheme::Mbr => cursor,
    };
    let size = size.unwrap_or(required);
    if !size.is_multiple_of(SECTOR) {
        return Err(format!(
            "the disk size {size} is not a whole number of sectors"
        ));
    }
    if size < required {
        return Err(format!(
            "the disk is {size} bytes, but its partitions and table need {required}"
        ));
    }
    // Where the backup table starts, so nothing else may reach it.
    let tail_start = match scheme {
        TableScheme::Gpt => size - GPT_TAIL_BYTES,
        TableScheme::Mbr => size,
    };

    let mut blob_places: Vec<(u64, u64, &str)> = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let who = format!("blob '{}'", blob.name);
        if blob.len == 0 {
            return Err(format!("{who} is empty"));
        }
        let end = blob.offset.saturating_add(blob.len);
        if blob.offset < reserved {
            return Err(format!(
                "{who} at {} is inside the partition table, which takes the first {reserved} bytes",
                blob.offset
            ));
        }
        if end > tail_start {
            return Err(format!(
                "{who} ends at {end}, past the space before the disk's end table at {tail_start}"
            ));
        }
        for (partition, place) in partitions.iter().zip(&placed) {
            if blob.offset < place.start + place.size && place.start < end {
                return Err(format!(
                    "{who} overlaps partition '{}' at {}",
                    partition.name, place.start
                ));
            }
        }
        blob_places.push((blob.offset, end, &blob.name));
    }
    blob_places.sort_unstable();
    for pair in blob_places.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(format!(
                "blob '{}' overlaps blob '{}'",
                pair[1].2, pair[0].2
            ));
        }
    }
    let blob_placed = blobs
        .iter()
        .map(|blob| Placed {
            start: blob.offset,
            size: blob.len,
        })
        .collect();

    Ok(Layout {
        scheme,
        size,
        first,
        alignment,
        partitions: placed,
        blobs: blob_placed,
    })
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, size: Option<u64>, content: Option<u64>) -> PartitionInput {
        PartitionInput {
            name: name.into(),
            role: PartitionRole::parse("data"),
            table_type: "0x83".into(),
            bootable: false,
            offset: None,
            size,
            content_len: content,
        }
    }

    #[test]
    fn partitions_are_aligned_in_order_and_sized_by_their_content() {
        let parts = [part("a", None, Some(1000)), part("b", Some(4096), Some(10))];
        let made = layout(TableScheme::Mbr, None, None, None, &parts, &[]).expect("layout");
        assert_eq!(
            made.partitions[0],
            Placed {
                start: MIB,
                size: 1024
            }
        );
        assert_eq!(
            made.partitions[1],
            Placed {
                start: 2 * MIB,
                size: 4096
            }
        );
        assert_eq!(made.size, 2 * MIB + 4096);
    }

    #[test]
    fn a_gpt_disk_keeps_room_for_its_backup_table() {
        let parts = [part("a", Some(MIB), None)];
        let made = layout(TableScheme::Gpt, None, None, None, &parts, &[]).expect("layout");
        assert_eq!(made.size, 2 * MIB + MIB);
    }

    #[test]
    fn refusals_name_the_rule() {
        let refuse = |parts: &[PartitionInput], blobs: &[BlobInput], size: Option<u64>| {
            layout(TableScheme::Gpt, None, None, size, parts, blobs).expect_err("refused")
        };
        assert!(refuse(&[part("a", None, None)], &[], None).contains("needs a size"));
        assert!(refuse(&[part("a", Some(512), Some(1024))], &[], None).contains("content is 1024"));
        assert!(refuse(&[part("a", Some(MIB), None)], &[], Some(MIB)).contains("need"));
        let blob = |offset, len| BlobInput {
            name: "idbloader".into(),
            offset,
            len,
        };
        assert!(refuse(&[], &[blob(0, 512)], None).contains("inside the partition table"));
        assert!(
            refuse(&[], &[blob(32 * 1024, 8 * MIB)], Some(8 * MIB))
                .contains("past the space before the disk's end table")
        );
        let mut overlap = part("a", Some(MIB), None);
        overlap.offset = Some(MIB);
        assert!(
            refuse(&[overlap], &[blob(MIB - 512, 1024)], None).contains("overlaps partition 'a'")
        );
        assert!(
            refuse(&[], &[blob(64 * 1024, 4096), blob(66 * 1024, 4096)], None)
                .contains("overlaps blob")
        );
        let five: Vec<_> = (0..5)
            .map(|i| part(&format!("p{i}"), Some(512), None))
            .collect();
        assert!(
            layout(TableScheme::Mbr, None, None, None, &five, &[])
                .expect_err("five")
                .contains("do not fit")
        );
    }
}
