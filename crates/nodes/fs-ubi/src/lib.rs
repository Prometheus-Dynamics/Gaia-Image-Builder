//! `gaia.fs.ubi`: volumes as a UBI image for raw NAND flash.
//!
//! The image is what `ubinize` writes and the kernel's UBI attaches: the
//! layout volume's two copies of the volume table in PEBs 0 and 1, then each
//! volume's data, one PEB per LEB, in volume order. Volumes with no data, and
//! LEBs reserved past the data, take no PEBs; UBI hands them out from the
//! erased flash after the image.
//!
//! Every byte is fixed by the inputs: the image sequence number is the
//! flash's, or a CRC of the image's id, and the headers carry erase counters
//! of 0, sequence numbers of 0 and no timestamps. The same volumes, flash and
//! id give the same bytes on every host.
//!
//! The on-media format is in [`mod@format`] and the PEB geometry and reserves in
//! [`mod@geometry`]: the writer's half of `gaia_formats::ubi` on dev. The device
//! side reads the same bytes, so a change to either changes every image.

pub mod format;
pub mod geometry;

pub use format::{VolumeKind, ubi_crc};
pub use geometry::{Geometry, GeometryError, bad_peb_reserve, minimum_device_pebs};

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Image, ImageKind, ObjectKind, Size, StoreRef};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use format::{
    EcHeader, LAYOUT_VOLUME_COMPAT, LAYOUT_VOLUME_ID, LAYOUT_VOLUME_LEBS, VOLUME_NAME_MAX,
    VidHeader, VtblRecord, encode_peb, encode_volume_table,
};
use geometry::reserved_pebs;

const NAME: &str = "gaia.fs.ubi";

/// The flash a UBI image is for, as the board describes it.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:ubi-flash")]
pub struct UbiFlash {
    /// The eraseblock size (`ubinize -p`).
    pub peb_size: Size,
    /// The page size (`ubinize -m`).
    pub min_io_size: Size,
    /// The sub-page size (`ubinize -s`); unset means no sub-pages.
    pub sub_page_size: Option<Size>,
    /// The VID header offset (`ubinize -O`); unset puts it at the sub-page
    /// size, as the kernel does.
    pub vid_header_offset: Option<Size>,
    /// The image sequence number; unset derives it from the image's id.
    pub image_seq: Option<u32>,
    /// The size of the whole flash device the image is for. Set, the build
    /// fails when the volumes and UBI's reserves do not fit it.
    pub flash_size: Option<Size>,
    /// PEBs UBI keeps for bad blocks per 1024 on the device: the kernel's
    /// `CONFIG_MTD_UBI_BEB_LIMIT` or `ubi.mtd=...,<limit>`.
    pub bad_pebs_per_1024: u32,
}

/// One volume of a UBI image: a slot, a data area or a raw blob.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:ubi-volume")]
pub struct UbiVolume {
    pub name: String,
    /// The volume id; unset is the volume's position among the volumes.
    pub id: Option<u32>,
    pub kind: VolumeKind,
    /// The volume's contents, a file in the store; unset leaves it empty.
    pub data: Option<StoreRef>,
    /// The bytes reserved for the volume, at least its data: a slot sized for
    /// later, larger updates. Unset reserves the data's size.
    pub size: Option<Size>,
    /// UBI grows this volume over the free PEBs on first attach.
    pub autoresize: bool,
}

/// The UBI image of `volumes` on `flash`: the node. The volumes are a fan-in
/// (`volumes0`, `volumes1`, ...) taken in port order, which gives their
/// positions. `id` names the image, for its default sequence number.
#[node(id = "gaia.fs.ubi", inputs(store, id, flash, volumes), outputs(image))]
pub fn fs_ubi(
    store: &str,
    id: &str,
    flash: UbiFlash,
    volumes: FanIn<'_, UbiVolume>,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let volumes: Vec<UbiVolume> = volumes.iter().map(|volume| (*volume).clone()).collect();
    ubi_image(store, id, &flash, &volumes, cx)
}

/// The UBI image of `volumes` on `flash`, as [`fs_ubi`] makes it.
pub fn ubi_image(
    store: &str,
    id: &str,
    flash: &UbiFlash,
    volumes: &[UbiVolume],
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, format!("ubi '{id}': {e}"));
    let geometry = Geometry::new(
        flash.peb_size.0,
        flash.min_io_size.0,
        flash.sub_page_size.map(|s| s.0),
        flash.vid_header_offset.map(|s| s.0),
    )
    .map_err(|e| fail(e.to_string()))?;
    let problems = volume_problems(volumes, &geometry);
    if !problems.is_empty() {
        return Err(fail(problems.join("; ")));
    }
    let image_seq = flash
        .image_seq
        .unwrap_or_else(|| ubi_crc(id.as_bytes()).max(1));
    let ids = volume_ids(volumes);

    let mut key = KeyBuilder::new(NAME, 1)
        .config("peb_size", geometry.peb_size.to_string())
        .config("min_io_size", geometry.min_io_size.to_string())
        .config("vid_header_offset", geometry.vid_header_offset.to_string())
        .config("data_offset", geometry.data_offset.to_string())
        .config("image_seq", image_seq.to_string())
        .config("bad_pebs_per_1024", flash.bad_pebs_per_1024.to_string())
        .config(
            "flash_size",
            flash.flash_size.map_or(0, |s| s.0).to_string(),
        );
    for (index, (volume, id)) in volumes.iter().zip(&ids).enumerate() {
        key = key
            .config(&format!("volume {index} id"), id.to_string())
            .config(&format!("volume {index} name"), &volume.name)
            .config(
                &format!("volume {index} kind"),
                format!("{:?}", volume.kind),
            )
            .config(
                &format!("volume {index} size"),
                volume
                    .size
                    .map_or_else(|| "none".to_string(), |s| s.0.to_string()),
            )
            .config(
                &format!("volume {index} autoresize"),
                volume.autoresize.to_string(),
            );
        key = match &volume.data {
            Some(data) => key.input(
                &format!("volume {index} data"),
                digest_of(data).map_err(fail)?,
            ),
            None => key.config(&format!("volume {index} data"), "none"),
        };
    }

    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("image", Path::new("image.ubi"))],
        |work| {
            let mut readers = Vec::new();
            for volume in volumes {
                readers.push(match &volume.data {
                    Some(data) => {
                        let path = data.path_in(&store).ok_or_else(|| {
                            fail("a volume's data that is not in the store".into())
                        })?;
                        let file = std::fs::File::open(&path)
                            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
                        let bytes = file
                            .metadata()
                            .map_err(|e| fail(e.to_string()))?
                            .len();
                        Some((BufReader::new(file), bytes))
                    }
                    None => None,
                });
            }
            let mut sources: Vec<Source<'_>> = volumes
                .iter()
                .zip(&ids)
                .zip(readers.iter_mut())
                .map(|((volume, id), reader)| Source {
                    id: *id,
                    name: &volume.name,
                    kind: volume.kind,
                    size: volume.size.map(|s| s.0),
                    autoresize: volume.autoresize,
                    data: reader
                        .as_mut()
                        .map(|(reader, bytes)| (reader as &mut dyn Read, *bytes)),
                })
                .collect();
            let out = work.dir().join("image.ubi");
            let file = std::fs::File::create(&out)
                .map_err(|e| fail(format!("{}: {e}", out.display())))?;
            let mut writer = BufWriter::new(file);
            let summary = write_image(&geometry, image_seq, &mut sources, &mut writer)
                .map_err(fail)?;
            writer.flush().map_err(|e| fail(e.to_string()))?;
            drop(writer);

            if let Some(flash_size) = flash.flash_size {
                let peb = u64::from(geometry.peb_size);
                let device_pebs = flash_size.0 / peb;
                let reserved = summary.reserved_pebs();
                let needed = reserved + bad_peb_reserve(device_pebs, u64::from(flash.bad_pebs_per_1024));
                if needed > device_pebs {
                    let minimum = minimum_device_pebs(reserved, u64::from(flash.bad_pebs_per_1024));
                    return Err(fail(format!(
                        "the volumes and UBI's reserves need {needed} PEBs ({} bytes), more than the {device_pebs} of a {flash_size}-byte flash; the image needs at least {} bytes",
                        needed * peb,
                        minimum * peb
                    )));
                }
            }
            Ok(())
        },
    )?;
    let object = made.outputs["image"];
    let len = std::fs::metadata(store.path(&object))
        .map_err(|e| fail(e.to_string()))?
        .len();
    Ok(Image {
        kind: ImageKind::parse("ubi"),
        object: object.into(),
        size: Size(len),
        partitions: Vec::new(),
    })
}

/// Each volume's id: its own, else its position.
fn volume_ids(volumes: &[UbiVolume]) -> Vec<u32> {
    volumes
        .iter()
        .enumerate()
        .map(|(index, volume)| volume.id.unwrap_or(index as u32))
        .collect()
}

/// What is wrong with the volume set on `geometry`: too many volumes, ids
/// past the volume table, names out of range, repeated ids or names, more
/// than one autoresize volume, and a volume with nothing to hold.
fn volume_problems(volumes: &[UbiVolume], geometry: &Geometry) -> Vec<String> {
    let mut problems = Vec::new();
    let records = geometry.vtbl_records();
    if volumes.len() > records as usize {
        problems.push(format!(
            "{} volumes are more than the {records} a {}-byte LEB's volume table holds",
            volumes.len(),
            geometry.leb_size()
        ));
    }
    if volumes.iter().filter(|volume| volume.autoresize).count() > 1 {
        problems.push("only one volume can be autoresize".into());
    }
    let ids = volume_ids(volumes);
    for (index, volume) in volumes.iter().enumerate() {
        if ids[index] >= records {
            problems.push(format!(
                "volume '{}' id {} is past the last volume table record ({})",
                volume.name,
                ids[index],
                records.saturating_sub(1)
            ));
        }
        if volume.name.is_empty() || volume.name.len() > VOLUME_NAME_MAX {
            problems.push(format!(
                "volume name '{}' must be 1 to {VOLUME_NAME_MAX} bytes",
                volume.name
            ));
        }
        if volume.data.is_none() && volume.size.is_none() && !volume.autoresize {
            problems.push(format!(
                "volume '{}' has no data, size or autoresize, so it would hold nothing",
                volume.name
            ));
        }
        for other in 0..index {
            if ids[other] == ids[index] {
                problems.push(format!(
                    "volumes '{}' and '{}' share id {}",
                    volumes[other].name, volume.name, ids[index]
                ));
            }
            if volumes[other].name == volume.name {
                problems.push(format!("two volumes are named '{}'", volume.name));
            }
        }
    }
    problems
}

/// A volume's data object's digest, for the key. Only files hold data.
fn digest_of(data: &StoreRef) -> Result<gaia_store::Digest, String> {
    if data.kind != ObjectKind::File {
        return Err("a volume's data that is not a file".into());
    }
    data.object_ref()
        .map(|object| object.digest)
        .ok_or_else(|| "a volume's data that is not in the store".into())
}

/// One volume as the writer reads it: its id and layout, and its data.
struct Source<'a> {
    id: u32,
    name: &'a str,
    kind: VolumeKind,
    size: Option<u64>,
    autoresize: bool,
    data: Option<(&'a mut dyn Read, u64)>,
}

/// What the writer wrote: the PEBs, and the LEBs each volume reserves.
struct Summary {
    reserved_lebs: Vec<u32>,
}

impl Summary {
    /// The PEBs the attached image needs before bad-block reserves.
    fn reserved_pebs(&self) -> u64 {
        reserved_pebs(self.reserved_lebs.iter().copied())
    }
}

/// Writes the image of `volumes` to `out`: the layout volume in PEBs 0 and 1,
/// then each volume's data, one PEB per LEB. The caller checks the volume set.
fn write_image(
    geometry: &Geometry,
    image_seq: u32,
    volumes: &mut [Source<'_>],
    out: &mut dyn Write,
) -> Result<Summary, String> {
    let leb = u64::from(geometry.leb_size());
    let mut records = Vec::new();
    let mut used_lebs = Vec::new();
    let mut reserved_lebs = Vec::new();
    for volume in volumes.iter() {
        let data_bytes = volume.data.as_ref().map_or(0, |(_, bytes)| *bytes);
        let used = data_bytes.div_ceil(leb);
        // At least one LEB: the kernel refuses a volume table record that
        // names a volume but reserves nothing.
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
        let used =
            u32::try_from(used).map_err(|_| format!("volume '{}' is too large", volume.name))?;
        let reserved = u32::try_from(reserved)
            .map_err(|_| format!("volume '{}' is too large", volume.name))?;
        records.push((
            volume.id,
            VtblRecord::new(reserved, volume.kind, volume.name, volume.autoresize),
        ));
        used_lebs.push(used);
        reserved_lebs.push(reserved);
    }
    let table = encode_volume_table(geometry, &records).map_err(|error| error.to_string())?;

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
        write(out, &encode_peb(geometry, &ec, &vid, &table))?;
    }
    let mut buffer = vec![0u8; leb as usize];
    for (volume, used) in volumes.iter_mut().zip(used_lebs.iter().copied()) {
        let Some((reader, bytes)) = volume.data.as_mut() else {
            continue;
        };
        let mut left = *bytes;
        for lnum in 0..used {
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
                VolumeKind::Dynamic => VidHeader::new(VolumeKind::Dynamic, volume.id, lnum),
                VolumeKind::Static => VidHeader {
                    data_size: take as u32,
                    used_ebs: used,
                    data_crc: ubi_crc(data),
                    ..VidHeader::new(VolumeKind::Static, volume.id, lnum)
                },
            };
            write(out, &encode_peb(geometry, &ec, &vid, data))?;
        }
    }
    Ok(Summary { reserved_lebs })
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 1] = [&FS_UBI_NODE];
