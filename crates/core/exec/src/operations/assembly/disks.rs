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
    pub(super) image: PathBuf,
    pub(super) partition_type: u8,
    pub(super) start_lba: u32,
    pub(super) sector_count: u32,
    pub(super) bytes: u64,
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

fn plan_mbr_partitions(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    disk: &gaia_spec::AssemblyDiskSpec,
) -> Result<Vec<AssemblyDiskPartitionSummary>, String> {
    if disk.partitions.len() > 4 {
        return Err(format!(
            "assembly disk '{}' has {} partitions; MBR supports at most 4",
            disk.id,
            disk.partitions.len()
        ));
    }
    let mut planned = Vec::new();
    let alignment_lba = disk.alignment_lba.unwrap_or(2048).max(1);
    let mut next_lba = disk.first_lba.unwrap_or(2048);
    for partition in &disk.partitions {
        let image = roots.resolve_path(spec, &partition.image)?;
        let bytes = file_len(&image)?;
        let sector_count = bytes.div_ceil(512).max(1);
        let start_lba = align_to(next_lba, alignment_lba);
        let end_lba = start_lba
            .checked_add(sector_count)
            .ok_or_else(|| format!("assembly disk '{}' partition layout is too large", disk.id))?;
        if start_lba > u32::MAX as u64 || sector_count > u32::MAX as u64 {
            return Err(format!(
                "assembly disk '{}' partition '{}' exceeds MBR 32-bit LBA limits",
                disk.id, partition.name
            ));
        }
        planned.push(AssemblyDiskPartitionSummary {
            name: partition.name.clone(),
            image,
            partition_type: partition_type_byte(partition)?,
            start_lba: start_lba as u32,
            sector_count: sector_count as u32,
            bytes,
        });
        next_lba = end_lba;
    }
    Ok(planned)
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
    output_file.set_len(total_bytes).map_err(|error| {
        format!(
            "failed to size assembly disk '{}' to {total_bytes} bytes: {error}",
            output.display()
        )
    })?;

    for partition in partitions {
        let mut image = std_fs::File::open(&partition.image).map_err(|error| {
            format!(
                "failed to open assembly partition image '{}': {error}",
                partition.image.display()
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
                partition.image.display(),
                output.display()
            )
        })?;
    }

    let mut mbr = [0u8; 512];
    if let Some(signature) = disk_signature_bytes(disk)? {
        mbr[440..444].copy_from_slice(&signature);
    }
    for (index, partition) in partitions.iter().enumerate() {
        let spec_partition = &disk.partitions[index];
        let offset = 446 + index * 16;
        mbr[offset] = if spec_partition.bootable { 0x80 } else { 0x00 };
        mbr[offset + 1] = 0x00;
        mbr[offset + 2] = 0x02;
        mbr[offset + 3] = 0x00;
        mbr[offset + 4] = partition.partition_type;
        mbr[offset + 5] = 0xff;
        mbr[offset + 6] = 0xff;
        mbr[offset + 7] = 0xff;
        mbr[offset + 8..offset + 12].copy_from_slice(&partition.start_lba.to_le_bytes());
        mbr[offset + 12..offset + 16].copy_from_slice(&partition.sector_count.to_le_bytes());
    }
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    output_file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!(
            "failed to seek assembly disk MBR '{}': {error}",
            output.display()
        )
    })?;
    output_file.write_all(&mbr).map_err(|error| {
        format!(
            "failed to write assembly disk MBR '{}': {error}",
            output.display()
        )
    })
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
