use super::*;

pub(super) struct AssemblyDiskSummary {
    pub(super) output: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) partitions: Vec<AssemblyDiskPartitionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssemblyDiskPartitionSummary {
    pub(super) name: String,
    /// Partition number as the kernel sees it: p1-p4 for primaries, p5.. for
    /// logical partitions inside the extended partition.
    pub(super) number: u32,
    /// `None` for an empty partition.
    pub(super) image: Option<PathBuf>,
    pub(super) partition_type: u8,
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
    if disk.partition_table != gaia_spec::AssemblyPartitionTableSpec::Mbr {
        return Err(format!(
            "assembly disk '{}' partition table '{}' is not implemented",
            disk.id,
            disk.partition_table.as_str()
        ));
    }
    let output = roots.resolve_path(spec, &disk.output)?;
    if let Some(parent) = output.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly disk output dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let partitions = plan_mbr_partitions(spec, roots, disk)?;
    let temp = temporary_assembly_output_path(&output);
    write_mbr_disk(&temp, disk, &partitions)?;
    publish_assembly_output(&temp, &output)?;
    Ok(AssemblyDiskSummary {
        bytes: file_len(&output)?,
        sha256: file_sha256(&output)?,
        output,
        partitions,
    })
}

/// Lays partitions out in order. With more than 4 partitions the first 3
/// stay primary and the rest become logical partitions, each preceded by an
/// EBR at the start of its aligned slot with the data at the next boundary.
fn plan_mbr_partitions(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    disk: &gaia_spec::AssemblyDiskSpec,
) -> Result<Vec<AssemblyDiskPartitionSummary>, String> {
    let extended = disk.partitions.len() > MBR_PRIMARY_SLOTS;
    let mut planned = Vec::new();
    let alignment_lba = disk.alignment_lba.unwrap_or(2048).max(1);
    let mut next_lba = disk.first_lba.unwrap_or(2048);
    for (index, partition) in disk.partitions.iter().enumerate() {
        let logical = extended && index >= MBR_EXTENDED_SLOT;
        if logical && partition.bootable {
            return Err(format!(
                "assembly disk '{}' partition '{}' is a logical partition; bootable is only allowed on primary partitions",
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
        let (ebr_lba, start_lba) = if logical {
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
            number: if logical { index + 2 } else { index + 1 } as u32,
            image,
            partition_type: partition_type_byte(partition)?,
            bootable: partition.bootable,
            start_lba: start_lba as u32,
            sector_count: sector_count as u32,
            bytes,
            ebr_lba: ebr_lba.map(|lba| lba as u32),
            wipe_bytes,
        });
        next_lba = end_lba;
    }
    Ok(planned)
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

fn write_mbr_disk(
    output: &Path,
    disk: &gaia_spec::AssemblyDiskSpec,
    partitions: &[AssemblyDiskPartitionSummary],
) -> Result<(), String> {
    let total_sectors = partitions
        .iter()
        .map(|partition| partition.start_lba as u64 + partition.sector_count as u64)
        .max()
        .unwrap_or(2048);
    let total_bytes = total_sectors
        .checked_mul(512)
        .ok_or_else(|| format!("assembly disk '{}' is too large", disk.id))?;
    let mut output_file = std_fs::File::create(output).map_err(|error| {
        format!(
            "failed to create assembly disk '{}': {error}",
            output.display()
        )
    })?;
    // Sized up front so unwritten partition space stays sparse.
    output_file.set_len(total_bytes).map_err(|error| {
        format!(
            "failed to size assembly disk '{}' to {total_bytes} bytes: {error}",
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
        std::io::copy(&mut image, &mut output_file).map_err(|error| {
            format!(
                "failed to copy partition image '{}' into disk '{}': {error}",
                image_path.display(),
                output.display()
            )
        })?;
    }

    write_mbr_tables(&mut output_file, output, disk, partitions)
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
    state.insert(
        disk_state.field("partition_count"),
        summary.partitions.len(),
    );
    for (partition_index, partition) in summary.partitions.iter().enumerate() {
        let field = |name: &str| disk_state.child_field("partition", partition_index + 1, name);
        state.insert(field("name"), &partition.name);
        state.insert(field("type"), format!("0x{:02X}", partition.partition_type));
        match &partition.image {
            Some(image) => state.insert(field("image"), image.display().to_string()),
            None => state.insert(field("empty"), true),
        }
        state.insert(field("start_lba"), partition.start_lba);
        state.insert(field("sector_count"), partition.sector_count);
        state.insert(field("bytes"), partition.bytes);
        if summary.partitions.len() > MBR_PRIMARY_SLOTS {
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
