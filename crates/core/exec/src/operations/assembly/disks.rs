use super::*;

pub(super) struct AssemblyDiskSummary {
    pub(super) output: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    /// GPT only: the disk GUID.
    pub(super) disk_guid: Option<gaia_spec::GptGuid>,
    pub(super) partitions: Vec<AssemblyDiskPartitionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssemblyDiskPartitionSummary {
    pub(super) name: String,
    /// Partition number as the kernel sees it: p1-p4 for primaries, p5.. for
    /// logical partitions inside the extended partition; 0 for an image
    /// outside the table.
    pub(super) number: u32,
    /// `false` for an image written by offset with no table entry.
    pub(super) in_table: bool,
    /// `None` for an empty partition.
    pub(super) image: Option<PathBuf>,
    /// The MBR type byte; 0 on a GPT disk, which uses `gpt`.
    pub(super) partition_type: u8,
    /// GPT only: the type and partition GUIDs.
    pub(super) gpt: Option<GptPartitionIds>,
    pub(super) bootable: bool,
    pub(super) start_lba: u32,
    pub(super) sector_count: u32,
    /// Image bytes written into the partition (0 when empty).
    pub(super) bytes: u64,
    /// Sector of the EBR describing a logical partition.
    pub(super) ebr_lba: Option<u32>,
    /// Bytes zeroed at the partition start (empty partitions with `wipe`).
    pub(super) wipe_bytes: u64,
}

pub(super) fn execute_assembly_disk(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    disk: &gaia_spec::AssemblyDiskSpec,
) -> Result<AssemblyDiskSummary, String> {
    let disk_guid = match disk.partition_table {
        gaia_spec::AssemblyPartitionTableSpec::Mbr => None,
        gaia_spec::AssemblyPartitionTableSpec::Gpt => Some(gpt_disk_guid(disk)?),
    };
    let output = roots.resolve_path(spec, &disk.output)?;
    if let Some(parent) = output.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly disk output dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let partitions = plan_disk_partitions(spec, roots, disk, disk_guid)?;
    let temp = temporary_assembly_output_path(&output);
    write_disk(&temp, disk, disk_guid, &partitions)?;
    let sha256 = publish_assembly_output(&temp, &output)?;
    Ok(AssemblyDiskSummary {
        bytes: file_len(&output)?,
        sha256,
        output,
        disk_guid,
        partitions,
    })
}

/// Lays partitions out in order. On an MBR disk with more than 4 table
/// partitions the first 3 stay primary and the rest become logical
/// partitions, each preceded by an EBR at the start of its aligned slot with
/// the data at the next boundary. A GPT disk (`disk_guid` set) has only
/// primaries. A partition with an `offset` starts there and the next one
/// follows it; one with `table = false` takes space on the disk but no slot
/// in the table.
fn plan_disk_partitions(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    disk: &gaia_spec::AssemblyDiskSpec,
    disk_guid: Option<gaia_spec::GptGuid>,
) -> Result<Vec<AssemblyDiskPartitionSummary>, String> {
    let table_count = disk
        .partitions
        .iter()
        .filter(|partition| partition.table)
        .count();
    let extended = disk_guid.is_none() && table_count > MBR_PRIMARY_SLOTS;
    let mut planned = Vec::new();
    let alignment_lba = disk.alignment_lba.unwrap_or(2048).max(1);
    let mut next_lba = disk.first_lba.unwrap_or(2048);
    if disk_guid.is_some() && next_lba < GPT_MIN_FIRST_LBA {
        return Err(format!(
            "assembly disk '{}' first_lba {next_lba} overlaps the GPT header and entries; it must be at least {GPT_MIN_FIRST_LBA}",
            disk.id
        ));
    }
    let packed = extended && disk.ebr_placement == gaia_spec::AssemblyEbrPlacementSpec::Packed;
    // Packed: the EBR chain takes the first sectors of the extended
    // partition, one per logical partition; data starts after it.
    let mut packed_extended = None;
    // Index among the table partitions.
    let mut index = 0;
    for partition in &disk.partitions {
        let slot = index;
        if partition.table {
            index += 1;
        }
        let logical = partition.table && extended && slot >= MBR_EXTENDED_SLOT;
        if packed && partition.table && slot == MBR_EXTENDED_SLOT {
            let extended_lba = align_to(next_lba, alignment_lba);
            let logical_count = (table_count - MBR_EXTENDED_SLOT) as u64;
            // Sector of the last EBR in the chain.
            packed_extended = Some((extended_lba, extended_lba + logical_count - 1));
            next_lba = align_to(extended_lba + logical_count, alignment_lba);
        }
        if logical && partition.bootable {
            return Err(format!(
                "assembly disk '{}' partition '{}' is a logical partition; bootable is only allowed on primary partitions",
                disk.id, partition.name
            ));
        }
        if !partition.materialize && (partition.image.is_some() || partition.wipe) {
            return Err(format!(
                "assembly disk '{}' partition '{}' is not materialized, so it cannot have an image or wipe",
                disk.id, partition.name
            ));
        }
        let image = partition
            .image
            .as_ref()
            .map(|image| roots.resolve_path(spec, image))
            .transpose()?;
        let bytes = image.as_deref().map(file_len).transpose()?.unwrap_or(0);
        let sector_count = partition_sector_count(disk, partition, image.as_deref(), bytes)?;
        let offset_lba = partition_offset_lba(disk, partition)?;
        if logical && offset_lba.is_some() {
            return Err(format!(
                "assembly disk '{}' partition '{}' is a logical partition; offset is only allowed on primary partitions and images outside the table",
                disk.id, partition.name
            ));
        }
        let (ebr_lba, start_lba) = if let Some(offset_lba) = offset_lba {
            (None, offset_lba)
        } else if let (true, Some((extended_lba, last_ebr_lba))) = (logical, packed_extended) {
            let ebr_lba = extended_lba + (slot - MBR_EXTENDED_SLOT) as u64;
            let start_lba = align_to(next_lba, alignment_lba);
            if start_lba <= last_ebr_lba {
                return Err(format!(
                    "assembly disk '{}' partition '{}' has no room after its packed EBR chain",
                    disk.id, partition.name
                ));
            }
            (Some(ebr_lba), start_lba)
        } else if logical {
            let ebr_lba = align_to(next_lba, alignment_lba);
            (Some(ebr_lba), align_to(ebr_lba + 1, alignment_lba))
        } else {
            (None, align_to(next_lba, alignment_lba))
        };
        let end_lba = start_lba
            .checked_add(sector_count)
            .ok_or_else(|| format!("assembly disk '{}' partition layout is too large", disk.id))?;
        if start_lba > u32::MAX as u64
            || sector_count > u32::MAX as u64
            || (logical && end_lba > u32::MAX as u64)
        {
            return Err(format!(
                "assembly disk '{}' partition '{}' exceeds MBR 32-bit LBA limits",
                disk.id, partition.name
            ));
        }
        let wipe_bytes = if partition.wipe && image.is_none() {
            (sector_count * 512).min(WIPE_BYTES)
        } else {
            0
        };
        planned.push(AssemblyDiskPartitionSummary {
            name: partition.name.clone(),
            number: match (partition.table, logical) {
                (false, _) => 0,
                (true, true) => slot as u32 + 2,
                (true, false) => slot as u32 + 1,
            },
            in_table: partition.table,
            image,
            partition_type: match disk_guid {
                Some(_) => 0,
                None if !partition.table => 0,
                None => partition_type_byte(partition)?,
            },
            gpt: disk_guid
                .filter(|_| partition.table)
                .map(|disk_guid| gpt_partition_ids(disk, disk_guid, partition))
                .transpose()?,
            bootable: partition.bootable,
            start_lba: start_lba as u32,
            sector_count: sector_count as u32,
            bytes,
            ebr_lba: ebr_lba.map(|lba| lba as u32),
            wipe_bytes,
        });
        next_lba = next_lba.max(end_lba);
    }
    check_disk_overlaps(disk, disk_guid.is_some(), &planned)?;
    Ok(planned)
}

/// The start sector of a partition with an `offset`.
fn partition_offset_lba(
    disk: &gaia_spec::AssemblyDiskSpec,
    partition: &gaia_spec::AssemblyDiskPartitionSpec,
) -> Result<Option<u64>, String> {
    let offset = partition.parsed_offset().map_err(|error| {
        format!(
            "assembly disk '{}' partition '{}' has invalid offset: {error}",
            disk.id, partition.name
        )
    })?;
    match offset {
        Some(offset) if offset.bytes() % 512 != 0 => Err(format!(
            "assembly disk '{}' partition '{}' offset {} is not a multiple of 512 bytes",
            disk.id,
            partition.name,
            offset.bytes()
        )),
        offset => Ok(offset.map(|offset| offset.bytes() / 512)),
    }
}

/// Fails when two partitions, a partition and an EBR, or a partition and the
/// partition table share a sector. Only a partition with an `offset` can be
/// placed into another, but the check covers every partition.
fn check_disk_overlaps(
    disk: &gaia_spec::AssemblyDiskSpec,
    gpt: bool,
    planned: &[AssemblyDiskPartitionSummary],
) -> Result<(), String> {
    let table = if gpt {
        "the GPT header and entries"
    } else {
        "the MBR"
    };
    let table_end = if gpt { GPT_MIN_FIRST_LBA } else { 1 };
    let mut extents = vec![(0, table_end, table.to_string())];
    for partition in planned {
        let start = partition.start_lba as u64;
        extents.push((
            start,
            start + partition.sector_count as u64,
            format!("partition '{}'", partition.name),
        ));
        if let Some(ebr_lba) = partition.ebr_lba {
            let ebr_lba = ebr_lba as u64;
            extents.push((
                ebr_lba,
                ebr_lba + 1,
                format!("the EBR of partition '{}'", partition.name),
            ));
        }
    }
    extents.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in extents.windows(2) {
        let ((_, first_end, first), (second_start, _, second)) = (&pair[0], &pair[1]);
        if second_start < first_end {
            return Err(format!(
                "assembly disk '{}': {second} at sector {second_start} overlaps {first}, which ends at sector {first_end}",
                disk.id
            ));
        }
    }
    Ok(())
}

/// Sectors for a partition: its `size` when set (the image must fit), else
/// the image rounded up to whole sectors.
fn partition_sector_count(
    disk: &gaia_spec::AssemblyDiskSpec,
    partition: &gaia_spec::AssemblyDiskPartitionSpec,
    image: Option<&Path>,
    image_bytes: u64,
) -> Result<u64, String> {
    let size = partition.parsed_size().map_err(|error| {
        format!(
            "assembly disk '{}' partition '{}' has invalid size: {error}",
            disk.id, partition.name
        )
    })?;
    match (size, image) {
        (Some(size), _) if size.bytes() == 0 => Err(format!(
            "assembly disk '{}' partition '{}' size must be greater than zero",
            disk.id, partition.name
        )),
        (Some(size), Some(image)) if image_bytes > size.bytes() => Err(format!(
            "assembly disk '{}' partition '{}' image '{}' is {image_bytes} bytes, larger than the partition size {} ({} bytes)",
            disk.id,
            partition.name,
            image.display(),
            partition.size.as_deref().unwrap_or_default(),
            size.bytes()
        )),
        (Some(size), _) => Ok(size.bytes().div_ceil(512)),
        (None, Some(_)) => Ok(image_bytes.div_ceil(512).max(1)),
        (None, None) => Err(format!(
            "assembly disk '{}' partition '{}' needs an image or a size",
            disk.id, partition.name
        )),
    }
}

fn write_disk(
    output: &Path,
    disk: &gaia_spec::AssemblyDiskSpec,
    disk_guid: Option<gaia_spec::GptGuid>,
    partitions: &[AssemblyDiskPartitionSummary],
) -> Result<(), String> {
    let last_end = partitions
        .iter()
        .map(|partition| partition.start_lba as u64 + partition.sector_count as u64)
        .max()
        .unwrap_or(2048);
    let total_sectors = match disk_guid {
        Some(_) => gpt_total_sectors(last_end, disk.alignment_lba.unwrap_or(2048).max(1)),
        None => last_end,
    };
    if disk_guid.is_some() && disk.truncate.is_some() {
        return Err(format!(
            "assembly disk '{}' cannot truncate a GPT disk: its backup header is at the end",
            disk.id
        ));
    }
    let total_bytes = total_sectors
        .checked_mul(512)
        .ok_or_else(|| format!("assembly disk '{}' is too large", disk.id))?;
    let file_bytes = match disk.truncate {
        None => total_bytes,
        Some(gaia_spec::AssemblyDiskTruncateSpec::LastData) => {
            last_data_bytes(disk, partitions, total_bytes)
        }
    };
    let mut output_file = std_fs::File::create(output).map_err(|error| {
        format!(
            "failed to create assembly disk '{}': {error}",
            output.display()
        )
    })?;
    // Sized up front so unwritten partition space stays sparse. Every write
    // below lies inside `file_bytes`, so a truncated disk ends at its last
    // written byte.
    output_file.set_len(file_bytes).map_err(|error| {
        format!(
            "failed to size assembly disk '{}' to {file_bytes} bytes: {error}",
            output.display()
        )
    })?;

    for partition in partitions {
        let Some(image_path) = &partition.image else {
            if partition.wipe_bytes > 0 {
                write_zeros(
                    &mut output_file,
                    output,
                    partition.start_lba as u64 * 512,
                    partition.wipe_bytes,
                )?;
            }
            continue;
        };
        let mut image = std_fs::File::open(image_path).map_err(|error| {
            format!(
                "failed to open assembly partition image '{}': {error}",
                image_path.display()
            )
        })?;
        output_file
            .seek(SeekFrom::Start(partition.start_lba as u64 * 512))
            .map_err(|error| {
                format!(
                    "failed to seek assembly disk '{}' for partition '{}': {error}",
                    output.display(),
                    partition.name
                )
            })?;
        // Zero runs stay holes: an unwritten partition costs no disk space.
        // The disk file was sized up front, so no length change is needed.
        super::placement::copy_sparse_into(&mut image, &mut output_file).map_err(|error| {
            format!(
                "failed to copy partition image '{}' into disk '{}': {error}",
                image_path.display(),
                output.display()
            )
        })?;
    }

    let table: Vec<AssemblyDiskPartitionSummary> = partitions
        .iter()
        .filter(|partition| partition.in_table)
        .cloned()
        .collect();
    match disk_guid {
        Some(disk_guid) => {
            write_gpt_tables(&mut output_file, output, disk_guid, total_sectors, &table)
        }
        None => write_mbr_tables(&mut output_file, output, disk, &table),
    }
}

/// Length of a `truncate = "last-data"` disk: the end of the partition table
/// sectors (the MBR and each EBR), of every partition's image, or of every
/// wiped partition, rounded up to the alignment and capped at the full size.
/// The table itself is written from the full layout, so it still lists every
/// partition at its full size.
fn last_data_bytes(
    disk: &gaia_spec::AssemblyDiskSpec,
    partitions: &[AssemblyDiskPartitionSummary],
    total_bytes: u64,
) -> u64 {
    let mut end = 512u64;
    for partition in partitions {
        let start = partition.start_lba as u64 * 512;
        if let Some(ebr_lba) = partition.ebr_lba {
            end = end.max((ebr_lba as u64 + 1) * 512);
        }
        if partition.image.is_some() {
            end = end.max(start + partition.bytes);
        } else if partition.wipe_bytes > 0 {
            end = end.max(start + partition.sector_count as u64 * 512);
        }
    }
    let alignment = disk.alignment_lba.unwrap_or(2048).max(1) * 512;
    end.div_ceil(alignment)
        .saturating_mul(alignment)
        .min(total_bytes)
}

fn write_zeros(
    file: &mut std_fs::File,
    output: &Path,
    offset: u64,
    len: u64,
) -> Result<(), String> {
    let zeros = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.write_all(&zeros))
        .map_err(|error| {
            format!(
                "failed to wipe {len} bytes at offset {offset} in assembly disk '{}': {error}",
                output.display()
            )
        })
}

pub(super) fn record_disk_state(
    state: &mut KeyValueState,
    disk_index: usize,
    disk: &gaia_spec::AssemblyDiskSpec,
    summary: &AssemblyDiskSummary,
) {
    let disk_state = AssemblyStateKey::new("disk", disk_index);
    state.insert(disk_state.field("id"), disk.id.as_str());
    state.insert(
        disk_state.field("partition_table"),
        disk.partition_table.as_str(),
    );
    state.insert(
        disk_state.field("output"),
        summary.output.display().to_string(),
    );
    state.insert(disk_state.field("bytes"), summary.bytes);
    state.insert(disk_state.field("sha256"), &summary.sha256);
    if let Some(disk_guid) = summary.disk_guid {
        state.insert(disk_state.field("disk_guid"), disk_guid.to_string());
    }
    state.insert(
        disk_state.field("partition_count"),
        summary.partitions.len(),
    );
    for (partition_index, partition) in summary.partitions.iter().enumerate() {
        let field = |name: &str| disk_state.child_field("partition", partition_index + 1, name);
        state.insert(field("name"), &partition.name);
        match partition.gpt {
            _ if !partition.in_table => state.insert(field("table"), false),
            Some(ids) => {
                state.insert(field("type"), ids.type_guid.to_string());
                state.insert(field("guid"), ids.guid.to_string());
            }
            None => state.insert(field("type"), format!("0x{:02X}", partition.partition_type)),
        }
        match &partition.image {
            Some(image) => state.insert(field("image"), image.display().to_string()),
            None => state.insert(field("empty"), true),
        }
        state.insert(field("start_lba"), partition.start_lba);
        state.insert(field("sector_count"), partition.sector_count);
        state.insert(field("bytes"), partition.bytes);
        if summary.disk_guid.is_none()
            && partition.in_table
            && summary
                .partitions
                .iter()
                .filter(|partition| partition.in_table)
                .count()
                > MBR_PRIMARY_SLOTS
        {
            state.insert(field("number"), partition.number);
        }
        if let Some(ebr_lba) = partition.ebr_lba {
            state.insert(field("ebr_lba"), ebr_lba);
        }
        if partition.wipe_bytes > 0 {
            state.insert(field("wipe_bytes"), partition.wipe_bytes);
        }
    }
}

pub(super) fn partition_type_byte(
    partition: &gaia_spec::AssemblyDiskPartitionSpec,
) -> Result<u8, String> {
    partition
        .partition_type()
        .map(|kind| kind.byte())
        .map_err(|error| {
            format!(
                "assembly partition '{}' has invalid partition type: {error}",
                partition.name
            )
        })
}

pub(super) fn disk_signature_bytes(
    disk: &gaia_spec::AssemblyDiskSpec,
) -> Result<Option<[u8; 4]>, String> {
    if let Some(signature) = &disk.signature {
        let value = parse_hex_u32(signature).map_err(|error| {
            format!(
                "assembly disk '{}' has invalid signature '{}': {error}",
                disk.id, signature
            )
        })?;
        return Ok(Some(value.to_le_bytes()));
    }
    if let Some(text) = &disk.signature_text {
        let mut bytes = [0u8; 4];
        for (index, byte) in text.as_bytes().iter().copied().take(4).enumerate() {
            bytes[index] = byte;
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn parse_hex_u32(raw: &str) -> Result<u32, std::num::ParseIntError> {
    u32::from_str_radix(raw.trim_start_matches("0x").trim_start_matches("0X"), 16)
}

pub(super) fn align_to(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}
