//! UBI images, the bytes `ubinize` writes and the kernel's UBI attaches
//! (`drivers/mtd/ubi/ubi-media.h`). Each physical eraseblock (PEB) starts
//! with an erase-counter header; each one holding data has a volume-ID
//! header at `vid_header_offset` and the logical eraseblock (LEB) at
//! `data_offset`. PEBs 0 and 1 hold the layout volume: two copies of the
//! volume table. Every header field is big-endian, and every CRC is the
//! kernel's `crc32_le(UBI_CRC32_INIT, ...)`: the standard CRC32 without its
//! final inversion.

use super::*;
use gaia_spec::{AssemblyUbiSpec, AssemblyUbiVolumeKindSpec as UbiVolumeKind, UbiGeometry};
use std::io::{BufReader, BufWriter, Read, Write};

const EC_MAGIC: u32 = 0x5542_4923; // "UBI#"
const VID_MAGIC: u32 = 0x5542_4921; // "UBI!"
const VERSION: u8 = 1;
const HEADER_SIZE: u32 = gaia_spec::UBI_HEADER_SIZE;
/// The bytes of a header its CRC covers: all but the CRC itself.
const HEADER_CRC_SIZE: usize = 60;
const VTBL_RECORD_SIZE: usize = gaia_spec::UBI_VTBL_RECORD_SIZE as usize;
const VTBL_RECORD_CRC_SIZE: usize = VTBL_RECORD_SIZE - 4;
const LAYOUT_VOLUME_ID: u32 = 0x7fff_efff;
const LAYOUT_VOLUME_LEBS: u32 = 2;
/// `UBI_COMPAT_REJECT`: an older UBI that does not know the layout volume
/// refuses the image.
const LAYOUT_VOLUME_COMPAT: u8 = 5;
const VTBL_AUTORESIZE: u8 = 0x01;
/// The PEBs UBI keeps for itself beyond the layout volume: one for wear
/// levelling (`WL_RESERVED_PEBS`) and one for atomic LEB changes
/// (`EBA_RESERVED_PEBS`).
const RUNTIME_RESERVED_PEBS: u64 = 2;
const ERASED: u8 = 0xff;

pub(super) fn ubi_crc(bytes: &[u8]) -> u32 {
    !crc32(bytes)
}

fn kind_code(kind: UbiVolumeKind) -> u8 {
    match kind {
        UbiVolumeKind::Dynamic => 1,
        UbiVolumeKind::Static => 2,
    }
}

/// The flash geometry an image is written for. The kernel refuses an image
/// whose header offsets differ from the ones it derives from the MTD
/// device, so these must match the flash part.
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
        u64::from(LAYOUT_VOLUME_LEBS)
            + RUNTIME_RESERVED_PEBS
            + self
                .volumes
                .iter()
                .map(|volume| u64::from(volume.reserved_lebs))
                .sum::<u64>()
    }
}

/// The bad-block reserve UBI keeps on a device of `device_pebs` PEBs
/// (`get_bad_peb_limit`): `per_1024` per 1024 PEBs, rounded up.
pub(super) fn bad_peb_reserve(device_pebs: u64, per_1024: u64) -> u64 {
    let limit = device_pebs * per_1024 / 1024;
    if limit * 1024 / per_1024.max(1) < device_pebs {
        limit + 1
    } else {
        limit
    }
}

/// The fewest PEBs a flash device needs to attach an image that reserves
/// `reserved_pebs`, counting the bad-block reserve that grows with it.
pub(super) fn minimum_device_pebs(reserved_pebs: u64, per_1024: u64) -> u64 {
    let mut pebs = reserved_pebs;
    while pebs < reserved_pebs + bad_peb_reserve(pebs, per_1024) {
        pebs += 1;
    }
    pebs
}

fn ec_header(geometry: &UbiGeometry, image_seq: u32) -> [u8; HEADER_SIZE as usize] {
    let mut header = [0u8; HEADER_SIZE as usize];
    header[0..4].copy_from_slice(&EC_MAGIC.to_be_bytes());
    header[4] = VERSION;
    // The erase counter (8..16) starts at 0, as ubinize's default.
    header[16..20].copy_from_slice(&geometry.vid_header_offset.to_be_bytes());
    header[20..24].copy_from_slice(&geometry.data_offset.to_be_bytes());
    header[24..28].copy_from_slice(&image_seq.to_be_bytes());
    let crc = ubi_crc(&header[..HEADER_CRC_SIZE]);
    header[60..64].copy_from_slice(&crc.to_be_bytes());
    header
}

struct VidHeader {
    kind: UbiVolumeKind,
    compat: u8,
    volume_id: u32,
    lnum: u32,
    /// Static volumes only: the bytes in this LEB, the volume's LEB count
    /// and the CRC of this LEB's bytes.
    data_size: u32,
    used_ebs: u32,
    data_crc: u32,
}

fn vid_header(fields: &VidHeader) -> [u8; HEADER_SIZE as usize] {
    let mut header = [0u8; HEADER_SIZE as usize];
    header[0..4].copy_from_slice(&VID_MAGIC.to_be_bytes());
    header[4] = VERSION;
    header[5] = kind_code(fields.kind);
    header[7] = fields.compat;
    header[8..12].copy_from_slice(&fields.volume_id.to_be_bytes());
    header[12..16].copy_from_slice(&fields.lnum.to_be_bytes());
    header[20..24].copy_from_slice(&fields.data_size.to_be_bytes());
    header[24..28].copy_from_slice(&fields.used_ebs.to_be_bytes());
    // data_pad (28..32) is 0: every volume has alignment 1.
    header[32..36].copy_from_slice(&fields.data_crc.to_be_bytes());
    // The sequence number (40..48) is 0, as ubinize writes it: the kernel
    // numbers writes from the highest it finds.
    let crc = ubi_crc(&header[..HEADER_CRC_SIZE]);
    header[60..64].copy_from_slice(&crc.to_be_bytes());
    header
}

fn vtbl_record(
    record: &mut [u8],
    reserved_lebs: u32,
    kind: UbiVolumeKind,
    name: &str,
    autoresize: bool,
) {
    record[0..4].copy_from_slice(&reserved_lebs.to_be_bytes());
    record[4..8].copy_from_slice(&1u32.to_be_bytes());
    record[12] = kind_code(kind);
    record[14..16].copy_from_slice(&(name.len() as u16).to_be_bytes());
    record[16..16 + name.len()].copy_from_slice(name.as_bytes());
    record[144] = if autoresize { VTBL_AUTORESIZE } else { 0 };
}

fn seal_vtbl_records(table: &mut [u8]) {
    for record in table.as_chunks_mut::<VTBL_RECORD_SIZE>().0 {
        let crc = ubi_crc(&record[..VTBL_RECORD_CRC_SIZE]);
        record[VTBL_RECORD_CRC_SIZE..].copy_from_slice(&crc.to_be_bytes());
    }
}

/// One whole PEB: the EC header, the VID header and the LEB's bytes, with
/// erased (0xff) bytes between them and after the data.
fn peb(geometry: &UbiGeometry, image_seq: u32, vid: &VidHeader, data: &[u8]) -> Vec<u8> {
    let mut peb = vec![ERASED; geometry.peb_size as usize];
    peb[..HEADER_SIZE as usize].copy_from_slice(&ec_header(geometry, image_seq));
    let vid_at = geometry.vid_header_offset as usize;
    peb[vid_at..vid_at + HEADER_SIZE as usize].copy_from_slice(&vid_header(vid));
    let data_at = geometry.data_offset as usize;
    peb[data_at..data_at + data.len()].copy_from_slice(data);
    peb
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
    let leb = u64::from(geometry.leb_size());
    let mut table = vec![0u8; geometry.vtbl_records() as usize * VTBL_RECORD_SIZE];
    let mut summaries = Vec::new();
    for volume in volumes.iter() {
        let data_bytes = volume.data.as_ref().map_or(0, |(_, bytes)| *bytes);
        let used = data_bytes.div_ceil(leb);
        let reserved = volume
            .size
            .map_or(used, |size| size.div_ceil(leb))
            .max(used);
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
        let at = volume.id as usize * VTBL_RECORD_SIZE;
        vtbl_record(
            &mut table[at..at + VTBL_RECORD_SIZE],
            reserved_lebs,
            volume.kind,
            volume.name,
            volume.autoresize,
        );
        summaries.push(UbiVolumeSummary {
            id: volume.id,
            name: volume.name.to_string(),
            data_bytes,
            used_lebs,
            reserved_lebs,
        });
    }
    seal_vtbl_records(&mut table);

    let write = |out: &mut dyn Write, bytes: &[u8]| {
        out.write_all(bytes)
            .map_err(|error| format!("failed to write the UBI image: {error}"))
    };
    for lnum in 0..LAYOUT_VOLUME_LEBS {
        let vid = VidHeader {
            kind: UbiVolumeKind::Dynamic,
            compat: LAYOUT_VOLUME_COMPAT,
            volume_id: LAYOUT_VOLUME_ID,
            lnum,
            data_size: 0,
            used_ebs: 0,
            data_crc: 0,
        };
        write(out, &peb(geometry, image_seq, &vid, &table))?;
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
            let vid = match volume.kind {
                UbiVolumeKind::Dynamic => VidHeader {
                    kind: volume.kind,
                    compat: 0,
                    volume_id: volume.id,
                    lnum,
                    data_size: 0,
                    used_ebs: 0,
                    data_crc: 0,
                },
                UbiVolumeKind::Static => VidHeader {
                    kind: volume.kind,
                    compat: 0,
                    volume_id: volume.id,
                    lnum,
                    data_size: take as u32,
                    used_ebs: summary.used_lebs,
                    data_crc: ubi_crc(data),
                },
            };
            write(out, &peb(geometry, image_seq, &vid, data))?;
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
