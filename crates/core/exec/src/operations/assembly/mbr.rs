use super::*;

/// Partition entries in an MBR (and in each EBR).
pub(super) const MBR_PRIMARY_SLOTS: usize = 4;
/// MBR slot that holds the extended partition when a disk has more than
/// four partitions; it is also the index of the first logical partition.
pub(super) const MBR_EXTENDED_SLOT: usize = 3;
/// Bytes zeroed at the start of an empty partition with `wipe = true`.
pub(super) const WIPE_BYTES: u64 = 1024 * 1024;

const MBR_EXTENDED_TYPE: u8 = 0x05;
const MBR_ENTRY_OFFSET: usize = 446;
const MBR_ENTRY_LEN: usize = 16;

/// Writes the MBR and, for a disk with more than four partitions, the
/// extended partition entry plus the chain of EBRs (the standard sfdisk
/// layout).
pub(super) fn write_mbr_tables(
    file: &mut std_fs::File,
    output: &Path,
    disk: &gaia_spec::AssemblyDiskSpec,
    partitions: &[AssemblyDiskPartitionSummary],
) -> Result<(), String> {
    let mut mbr = boot_sector();
    if let Some(signature) = disk_signature_bytes(disk)? {
        mbr[440..444].copy_from_slice(&signature);
    }
    let extended = partitions.len() > MBR_PRIMARY_SLOTS;
    let primary_count = if extended {
        MBR_EXTENDED_SLOT
    } else {
        partitions.len()
    };
    for (index, partition) in partitions.iter().take(primary_count).enumerate() {
        set_entry(
            &mut mbr,
            index,
            partition.bootable,
            partition.partition_type,
            partition.start_lba,
            partition.sector_count,
        );
    }
    if extended {
        let logical = &partitions[MBR_EXTENDED_SLOT..];
        let extended_start = logical_ebr_lba(&logical[0])?;
        let extended_end = logical
            .last()
            .map(|partition| partition.start_lba + partition.sector_count)
            .unwrap_or(extended_start);
        set_entry(
            &mut mbr,
            MBR_EXTENDED_SLOT,
            false,
            MBR_EXTENDED_TYPE,
            extended_start,
            extended_end - extended_start,
        );
        for (index, partition) in logical.iter().enumerate() {
            let ebr_lba = logical_ebr_lba(partition)?;
            let mut ebr = boot_sector();
            set_entry(
                &mut ebr,
                0,
                false,
                partition.partition_type,
                partition.start_lba - ebr_lba,
                partition.sector_count,
            );
            if let Some(next) = logical.get(index + 1) {
                let next_ebr = logical_ebr_lba(next)?;
                set_entry(
                    &mut ebr,
                    1,
                    false,
                    MBR_EXTENDED_TYPE,
                    next_ebr - extended_start,
                    next.start_lba + next.sector_count - next_ebr,
                );
            }
            write_sector(file, output, ebr_lba, &ebr)?;
        }
    }
    write_sector(file, output, 0, &mbr)
}

fn logical_ebr_lba(partition: &AssemblyDiskPartitionSummary) -> Result<u32, String> {
    partition.ebr_lba.ok_or_else(|| {
        format!(
            "assembly partition '{}' is logical but has no EBR",
            partition.name
        )
    })
}

fn boot_sector() -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[510] = 0x55;
    sector[511] = 0xaa;
    sector
}

/// Encodes one partition entry with LBA addressing; CHS fields carry the
/// conventional "use LBA" placeholders.
fn set_entry(
    sector: &mut [u8; 512],
    slot: usize,
    bootable: bool,
    partition_type: u8,
    start_lba: u32,
    sector_count: u32,
) {
    let offset = MBR_ENTRY_OFFSET + slot * MBR_ENTRY_LEN;
    sector[offset] = if bootable { 0x80 } else { 0x00 };
    sector[offset + 1] = 0x00;
    sector[offset + 2] = 0x02;
    sector[offset + 3] = 0x00;
    sector[offset + 4] = partition_type;
    sector[offset + 5] = 0xff;
    sector[offset + 6] = 0xff;
    sector[offset + 7] = 0xff;
    sector[offset + 8..offset + 12].copy_from_slice(&start_lba.to_le_bytes());
    sector[offset + 12..offset + 16].copy_from_slice(&sector_count.to_le_bytes());
}

fn write_sector(
    file: &mut std_fs::File,
    output: &Path,
    lba: u32,
    sector: &[u8; 512],
) -> Result<(), String> {
    file.seek(SeekFrom::Start(lba as u64 * 512))
        .and_then(|_| file.write_all(sector))
        .map_err(|error| {
            format!(
                "failed to write partition table sector {lba} of assembly disk '{}': {error}",
                output.display()
            )
        })
}
