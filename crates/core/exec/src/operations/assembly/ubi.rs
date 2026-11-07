//! UBI images, the bytes `ubinize` writes and the kernel's UBI attaches.
//! The format (headers, CRCs, geometry, reserves) is `gaia_formats::ubi`,
//! the contract shared with Atlas; this is Gaia's layout of an image: the
//! layout volume in PEBs 0 and 1, then each volume's data LEBs.

use super::*;
use gaia_formats::ubi::{
    EcHeader, LAYOUT_VOLUME_COMPAT, LAYOUT_VOLUME_ID, LAYOUT_VOLUME_LEBS, VidHeader, VolumeKind,
    VtblRecord, encode_peb, encode_volume_table,
};
pub(super) use gaia_formats::ubi::{bad_peb_reserve, minimum_device_pebs};
use gaia_spec::{AssemblyUbiSpec, AssemblyUbiVolumeKindSpec as UbiVolumeKind, UbiGeometry};
use std::io::{BufReader, BufWriter, Read, Write};

#[cfg(test)]
use gaia_formats::ubi::{
    EC_MAGIC, VID_MAGIC, VTBL_AUTORESIZE, VTBL_RECORD_CRC_SIZE, VTBL_RECORD_SIZE,
};

pub(super) fn ubi_crc(bytes: &[u8]) -> u32 {
    gaia_formats::crc::ubi_crc(bytes)
}

fn volume_kind(kind: UbiVolumeKind) -> VolumeKind {
    match kind {
        UbiVolumeKind::Dynamic => VolumeKind::Dynamic,
        UbiVolumeKind::Static => VolumeKind::Static,
    }
}

/// One volume to write. `size` reserves LEBs past the data, for a slot that
/// a later, larger update must fit in; it is rounded up to whole LEBs.
pub(super) struct UbiVolume<'a> {
    pub(super) id: u32,
    pub(super) name: &'a str,
    pub(super) kind: UbiVolumeKind,
    pub(super) size: Option<u64>,
    pub(super) autoresize: bool,
    pub(super) data: Option<(&'a mut dyn Read, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UbiVolumeSummary {
    pub(super) id: u32,
    pub(super) name: String,
    pub(super) data_bytes: u64,
    pub(super) used_lebs: u32,
    pub(super) reserved_lebs: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UbiImageSummary {
    pub(super) pebs_written: u64,
    pub(super) volumes: Vec<UbiVolumeSummary>,
}

impl UbiImageSummary {
    /// The PEBs the attached image needs before bad-block reserves: the
    /// layout volume, UBI's own reserves and every volume's reservation.
    pub(super) fn reserved_pebs(&self) -> u64 {
        gaia_formats::ubi::reserved_pebs(self.volumes.iter().map(|volume| volume.reserved_lebs))
    }
}

/// Writes the UBI image of `volumes` to `out`: the layout volume in PEBs 0
/// and 1, then each volume's data, one PEB per LEB. Volumes without data
/// and LEBs reserved past the data take no PEBs in the image; UBI hands them
/// out from the erased flash after it.
/// The caller checks the volume set (`AssemblyUbiSpec::volume_problems`).
pub(super) fn write_ubi_image(
    geometry: &UbiGeometry,
    image_seq: u32,
    volumes: &mut [UbiVolume<'_>],
    out: &mut dyn Write,
) -> Result<UbiImageSummary, String> {
    let layout = geometry.layout();
    let leb = u64::from(geometry.leb_size());
    let mut records = Vec::new();
    let mut summaries = Vec::new();
    for volume in volumes.iter() {
        let data_bytes = volume.data.as_ref().map_or(0, |(_, bytes)| *bytes);
        let used = data_bytes.div_ceil(leb);
        // At least one LEB: the kernel refuses a volume table record that
        // names a volume but reserves nothing (an autoresize volume with no
        // size, or an empty image).
        let reserved = volume
            .size
            .map_or(used, |size| size.div_ceil(leb))
            .max(used)
            .max(1);
        if volume.size.is_some_and(|size| size < data_bytes) {
            return Err(format!(
                "volume '{}' holds {data_bytes} bytes, more than its size of {} bytes",
                volume.name,
                volume.size.unwrap_or_default()
            ));
        }
        let used_lebs =
            u32::try_from(used).map_err(|_| format!("volume '{}' is too large", volume.name))?;
        let reserved_lebs = u32::try_from(reserved)
            .map_err(|_| format!("volume '{}' is too large", volume.name))?;
        records.push((
            volume.id,
            VtblRecord::new(
                reserved_lebs,
                volume_kind(volume.kind),
                volume.name,
                volume.autoresize,
            ),
        ));
        summaries.push(UbiVolumeSummary {
            id: volume.id,
            name: volume.name.to_string(),
            data_bytes,
            used_lebs,
            reserved_lebs,
        });
    }
    let table = encode_volume_table(&layout, &records).map_err(|error| error.to_string())?;

    let ec = EcHeader {
        erase_counter: 0,
        vid_header_offset: geometry.vid_header_offset,
        data_offset: geometry.data_offset,
        image_seq,
    };
    let write = |out: &mut dyn Write, bytes: &[u8]| {
        out.write_all(bytes)
            .map_err(|error| format!("failed to write the UBI image: {error}"))
    };
    for lnum in 0..LAYOUT_VOLUME_LEBS {
        let vid = VidHeader {
            compat: LAYOUT_VOLUME_COMPAT,
            ..VidHeader::new(VolumeKind::Dynamic, LAYOUT_VOLUME_ID, lnum)
        };
        write(out, &encode_peb(&layout, &ec, &vid, &table))?;
    }
    let mut pebs_written = u64::from(LAYOUT_VOLUME_LEBS);
    let mut buffer = vec![0u8; leb as usize];
    for (volume, summary) in volumes.iter_mut().zip(&summaries) {
        let Some((reader, bytes)) = volume.data.as_mut() else {
            continue;
        };
        let mut left = *bytes;
        for lnum in 0..summary.used_lebs {
            let take = left.min(leb) as usize;
            reader.read_exact(&mut buffer[..take]).map_err(|error| {
                format!(
                    "failed to read volume '{}' LEB {lnum}: {error}",
                    volume.name
                )
            })?;
            left -= take as u64;
            let data = &buffer[..take];
            let kind = volume_kind(volume.kind);
            let vid = match kind {
                VolumeKind::Dynamic => VidHeader::new(kind, volume.id, lnum),
                VolumeKind::Static => VidHeader {
                    data_size: take as u32,
                    used_ebs: summary.used_lebs,
                    data_crc: ubi_crc(data),
                    ..VidHeader::new(kind, volume.id, lnum)
                },
            };
            write(out, &encode_peb(&layout, &ec, &vid, data))?;
            pebs_written += 1;
        }
    }
    Ok(UbiImageSummary {
        pebs_written,
        volumes: summaries,
    })
}

/// The image sequence number of `ubi`: its own, else a CRC of its id, so
/// the same config always writes the same bytes.
fn image_seq(ubi: &AssemblyUbiSpec) -> u32 {
    ubi.image_seq
        .unwrap_or_else(|| ubi_crc(ubi.id.as_bytes()).max(1))
}

pub(super) struct UbiSummary {
    pub(super) output: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) image: UbiImageSummary,
    pub(super) peb_size: u32,
    /// The fewest bytes of flash the attached image needs, bad-block
    /// reserve included.
    pub(super) minimum_flash_bytes: u64,
}

pub(super) fn execute_assembly_ubi(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    ubi: &AssemblyUbiSpec,
) -> Result<UbiSummary, AssemblyError> {
    let label = |error: String| format!("assembly ubi '{}': {error}", ubi.id);
    let geometry = ubi.geometry().map_err(label)?;
    let problems = ubi.volume_problems(&geometry);
    if !problems.is_empty() {
        return Err(label(problems.join("; ")).into());
    }
    let flash_size = ubi.parsed_flash_size().map_err(label)?;
    let output = roots.resolve_path(spec, ubi.output.as_str())?;
    if let Some(parent) = output.parent() {
        std_fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let mut sources = Vec::new();
    for volume in &ubi.volumes {
        let Some(image) = &volume.image else {
            sources.push(None);
            continue;
        };
        let path = roots.resolve_path(spec, image.as_str())?;
        let file = std_fs::File::open(&path).map_err(|error| {
            label(format!(
                "volume '{}' image '{}': {error}",
                volume.name,
                path.display()
            ))
        })?;
        let bytes = file
            .metadata()
            .map_err(|error| format!("failed to stat '{}': {error}", path.display()))?
            .len();
        sources.push(Some((BufReader::new(file), bytes)));
    }
    let ids = ubi.volume_ids();
    let mut volumes = Vec::new();
    for ((volume, source), id) in ubi.volumes.iter().zip(sources.iter_mut()).zip(ids) {
        let size = volume
            .size
            .as_deref()
            .map(|value| {
                value
                    .parse::<gaia_spec::ByteSize>()
                    .map(|size| size.bytes())
            })
            .transpose()
            .map_err(|error| label(format!("volume '{}' size: {error}", volume.name)))?;
        volumes.push(UbiVolume {
            id,
            name: &volume.name,
            kind: volume.kind,
            size,
            autoresize: volume.autoresize,
            data: source
                .as_mut()
                .map(|(reader, bytes)| (reader as &mut dyn Read, *bytes)),
        });
    }

    let temp = temporary_assembly_output_path(&output);
    let file = std_fs::File::create(&temp)
        .map_err(|error| format!("failed to create '{}': {error}", temp.display()))?;
    let mut writer = BufWriter::new(file);
    let image =
        write_ubi_image(&geometry, image_seq(ubi), &mut volumes, &mut writer).map_err(label)?;
    writer
        .flush()
        .map_err(|error| format!("failed to write '{}': {error}", temp.display()))?;
    drop(writer);

    let peb = u64::from(geometry.peb_size);
    let per_1024 = u64::from(ubi.bad_pebs_per_1024);
    let minimum_flash_bytes = minimum_device_pebs(image.reserved_pebs(), per_1024) * peb;
    if let Some(flash) = flash_size {
        let device_pebs = flash / peb;
        let needed = image.reserved_pebs() + bad_peb_reserve(device_pebs, per_1024);
        if needed > device_pebs {
            let _ = std_fs::remove_file(&temp);
            return Err(label(format!(
                "the volumes and UBI's reserves need {needed} PEBs ({} bytes), more than the {device_pebs} of a {flash}-byte flash; the image needs at least {minimum_flash_bytes} bytes",
                needed * peb
            ))
            .into());
        }
    }
    let bytes = image.pebs_written * peb;
    let sha256 = publish_assembly_output(&temp, &output)?;
    Ok(UbiSummary {
        output,
        bytes,
        sha256,
        image,
        peb_size: geometry.peb_size,
        minimum_flash_bytes,
    })
}

pub(super) fn record_ubi_state(
    state: &mut KeyValueState,
    index: usize,
    ubi: &AssemblyUbiSpec,
    summary: &UbiSummary,
) {
    let key = AssemblyStateKey::new("ubi", index);
    state.insert(key.field("id"), ubi.id.as_str());
    state.insert(key.field("output"), summary.output.display().to_string());
    state.insert(key.field("bytes"), summary.bytes);
    state.insert(key.field("sha256"), &summary.sha256);
    state.insert(key.field("peb_size"), summary.peb_size);
    state.insert(key.field("reserved_pebs"), summary.image.reserved_pebs());
    state.insert(
        key.field("minimum_flash_bytes"),
        summary.minimum_flash_bytes,
    );
    for (volume_index, volume) in summary.image.volumes.iter().enumerate() {
        let field = |name| key.child_field("volume", volume_index, name);
        state.insert(field("name"), volume.name.as_str());
        state.insert(field("id"), volume.id);
        state.insert(field("data_bytes"), volume.data_bytes);
        state.insert(field("reserved_lebs"), volume.reserved_lebs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(bytes: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(bytes[at..at + 4].try_into().expect("u32"))
    }

    const SEQ: u32 = 0x1234_5678;

    fn spi_nand() -> UbiGeometry {
        UbiGeometry::new(128 * 1024, 2048, None, None).expect("geometry")
    }

    #[test]
    fn the_crc_is_crc32_without_the_final_inversion() {
        assert_eq!(ubi_crc(b"123456789"), 0x340b_c6d9);
        // An empty volume table record's CRC, as it appears in every UBI
        // image's layout volume.
        assert_eq!(ubi_crc(&[0u8; VTBL_RECORD_CRC_SIZE]), 0xf116_c36b);
    }

    #[test]
    fn layout_volume_and_a_dynamic_volume_are_written_as_the_kernel_reads_them() {
        let geometry = spi_nand();
        let leb = geometry.leb_size() as usize;
        let rootfs: Vec<u8> = (0..leb + 100).map(|index| index as u8).collect();
        let mut reader: &[u8] = &rootfs;
        let mut volumes = [
            UbiVolume {
                id: 0,
                name: "rootfs_a",
                kind: UbiVolumeKind::Dynamic,
                size: Some(4 * leb as u64),
                autoresize: false,
                data: Some((&mut reader, rootfs.len() as u64)),
            },
            UbiVolume {
                id: 2,
                name: "data",
                kind: UbiVolumeKind::Dynamic,
                size: Some(1),
                autoresize: true,
                data: None,
            },
        ];
        let mut image = Vec::new();

        let summary = write_ubi_image(&geometry, SEQ, &mut volumes, &mut image).expect("image");

        assert_eq!(summary.pebs_written, 4);
        assert_eq!(image.len(), 4 * 128 * 1024);
        assert_eq!(
            summary.volumes,
            [
                UbiVolumeSummary {
                    id: 0,
                    name: "rootfs_a".into(),
                    data_bytes: rootfs.len() as u64,
                    used_lebs: 2,
                    reserved_lebs: 4,
                },
                UbiVolumeSummary {
                    id: 2,
                    name: "data".into(),
                    data_bytes: 0,
                    used_lebs: 0,
                    reserved_lebs: 1,
                },
            ]
        );
        assert_eq!(summary.reserved_pebs(), 2 + 2 + 5);

        for peb in image.as_chunks::<{ 128 * 1024 }>().0 {
            assert_eq!(be32(peb, 0), EC_MAGIC);
            assert_eq!(peb[4], 1);
            assert_eq!(be32(peb, 16), 2048);
            assert_eq!(be32(peb, 20), 4096);
            assert_eq!(be32(peb, 24), SEQ);
            assert_eq!(be32(peb, 60), ubi_crc(&peb[..60]));
            assert!(peb[64..2048].iter().all(|byte| *byte == 0xff));
            let vid = &peb[2048..2048 + 64];
            assert_eq!(be32(vid, 0), VID_MAGIC);
            assert_eq!(be32(vid, 60), ubi_crc(&vid[..60]));
            assert!(peb[2048 + 64..4096].iter().all(|byte| *byte == 0xff));
        }

        // Both layout copies hold the same table.
        let layout = |index: usize| &image[index * 128 * 1024..][..128 * 1024];
        assert_eq!(layout(0)[4096..], layout(1)[4096..]);
        let vid = &layout(1)[2048..];
        assert_eq!((vid[5], vid[7]), (1, 5));
        assert_eq!((be32(vid, 8), be32(vid, 12)), (LAYOUT_VOLUME_ID, 1));
        let table = &layout(0)[4096..];
        let record = |id: usize| &table[id * VTBL_RECORD_SIZE..][..VTBL_RECORD_SIZE];
        assert_eq!(be32(record(0), 0), 4);
        assert_eq!(be32(record(0), 4), 1);
        assert_eq!(record(0)[12], 1);
        assert_eq!(u16::from_be_bytes([record(0)[14], record(0)[15]]), 8);
        assert_eq!(&record(0)[16..25], b"rootfs_a\0");
        assert_eq!(record(0)[144], 0);
        assert_eq!(&record(2)[16..20], b"data");
        assert_eq!(record(2)[144], VTBL_AUTORESIZE);
        for id in 0..128 {
            assert_eq!(be32(record(id), 168), ubi_crc(&record(id)[..168]));
        }
        assert_eq!(be32(record(1), 168), 0xf116_c36b);
        assert!(
            table[128 * VTBL_RECORD_SIZE..]
                .iter()
                .all(|byte| *byte == 0xff)
        );

        // The rootfs LEBs: dynamic, so no size or CRC; the last one padded.
        let leb1 = &image[3 * 128 * 1024..][..128 * 1024];
        assert_eq!((be32(&leb1[2048..], 8), be32(&leb1[2048..], 12)), (0, 1));
        assert_eq!(be32(&leb1[2048..], 20), 0);
        assert_eq!(&leb1[4096..4096 + 100], &rootfs[leb..]);
        assert!(leb1[4096 + 100..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn a_static_volume_records_its_lebs_sizes_and_crcs() {
        let geometry = UbiGeometry::new(16 * 1024, 512, None, None).expect("geometry");
        let leb = geometry.leb_size() as usize;
        let kernel = vec![0x5au8; 2 * leb + 10];
        let mut reader: &[u8] = &kernel;
        let mut volumes = [UbiVolume {
            id: 0,
            name: "kernel",
            kind: UbiVolumeKind::Static,
            size: None,
            autoresize: false,
            data: Some((&mut reader, kernel.len() as u64)),
        }];
        let mut image = Vec::new();

        write_ubi_image(&geometry, SEQ, &mut volumes, &mut image).expect("image");

        let pebs: Vec<&[u8]> = image
            .as_chunks::<{ 16 * 1024 }>()
            .0
            .iter()
            .map(|peb| &peb[..])
            .collect();
        assert_eq!(pebs.len(), 5);
        assert_eq!(pebs[0][1024 + 12], 2, "static in the volume table");
        for (lnum, peb) in pebs[2..].iter().enumerate() {
            let vid = &peb[512..512 + 64];
            let size = if lnum == 2 { 10 } else { leb };
            assert_eq!(vid[5], 2);
            assert_eq!(be32(vid, 12), lnum as u32);
            assert_eq!(be32(vid, 20), size as u32);
            assert_eq!(be32(vid, 24), 3);
            assert_eq!(be32(vid, 32), ubi_crc(&peb[1024..1024 + size]));
        }
    }

    #[test]
    fn the_image_reads_back_with_the_format_crate_and_every_record_reserves() {
        let geometry = spi_nand();
        let mut kernel: &[u8] = &[7; 300];
        let mut volumes = [
            UbiVolume {
                id: 0,
                name: "kernel",
                kind: UbiVolumeKind::Static,
                size: None,
                autoresize: false,
                data: Some((&mut kernel, 300)),
            },
            // No size: the record still reserves a LEB, which the kernel
            // needs of a record that names a volume.
            UbiVolume {
                id: 1,
                name: "data",
                kind: UbiVolumeKind::Dynamic,
                size: None,
                autoresize: true,
                data: None,
            },
        ];
        let mut bytes = Vec::new();
        let summary = write_ubi_image(&geometry, SEQ, &mut volumes, &mut bytes).expect("image");
        let image =
            gaia_formats::ubi::read_image(&mut std::io::Cursor::new(&bytes), geometry.peb_size)
                .expect("read back");
        assert_eq!(image.layout, geometry.layout());
        assert_eq!(image.image_seq, SEQ);
        assert_eq!(image.reserved_pebs(), summary.reserved_pebs());
        let data = image.volume("data").expect("data");
        assert_eq!((data.reserved_lebs, data.autoresize), (1, true));
        assert_eq!(
            image.volume("kernel").expect("kernel").static_bytes,
            Some(300)
        );
    }

    #[test]
    fn a_volume_smaller_than_its_image_is_refused() {
        let mut data: &[u8] = &[0; 10];
        let mut volumes = [UbiVolume {
            id: 0,
            name: "a",
            kind: UbiVolumeKind::Dynamic,
            size: Some(5),
            autoresize: false,
            data: Some((&mut data, 10)),
        }];
        let error =
            write_ubi_image(&spi_nand(), SEQ, &mut volumes, &mut Vec::new()).expect_err("refused");
        assert!(error.contains("more than its size"), "{error}");
    }

    #[test]
    fn the_smallest_device_covers_the_bad_block_reserve() {
        assert_eq!(bad_peb_reserve(1024, 20), 20);
        assert_eq!(bad_peb_reserve(1000, 20), 20);
        assert_eq!(bad_peb_reserve(2048, 20), 40);
        let pebs = minimum_device_pebs(500, 20);
        assert_eq!(pebs, 510);
        assert!(pebs >= 500 + bad_peb_reserve(pebs, 20));
        assert!(pebs - 1 < 500 + bad_peb_reserve(pebs - 1, 20));
    }
}
