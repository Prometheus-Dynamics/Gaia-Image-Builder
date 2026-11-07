//! Reading a finished UBI image (Gaia's `[[image.assembly.ubis]]`, or
//! `ubinize`'s): every PEB's headers checked, the layout volume's two tables
//! compared, and each volume's reservation and the LEBs the image carries.

use super::{
    ERASED, EcHeader, GeometryError, HEADER_SIZE, HeaderError, LAYOUT_VOLUME_ID,
    LAYOUT_VOLUME_LEBS, PebLayout, VTBL_RECORD_SIZE, VidHeader, VolumeKind, VtblRecord,
};
use crate::crc::ubi_crc;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};

/// A volume the image's volume table declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub id: u32,
    pub name: String,
    pub kind: VolumeKind,
    pub reserved_lebs: u32,
    pub autoresize: bool,
    /// The LEBs the image carries (0 for a volume the system fills).
    pub image_lebs: u32,
    /// The image's PEB holding LEB 0, when it carries any.
    pub first_peb: Option<u32>,
    /// Static volumes: the data bytes, from the VID headers.
    pub static_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub layout: PebLayout,
    pub image_seq: u32,
    /// PEBs in the image.
    pub pebs: u32,
    pub volumes: Vec<Volume>,
}

impl Image {
    pub fn volume(&self, name: &str) -> Option<&Volume> {
        self.volumes.iter().find(|volume| volume.name == name)
    }

    /// The PEBs a flash needs for this image, before bad-block reserves:
    /// the layout volume, UBI's two working PEBs and every reservation.
    pub fn reserved_pebs(&self) -> u64 {
        super::reserved_pebs(self.volumes.iter().map(|volume| volume.reserved_lebs))
    }
}

/// Why an image is refused; `peb` is the index of the PEB at fault.
#[derive(Debug)]
pub enum ImageError {
    Io(std::io::Error),
    NotWholePebs {
        len: u64,
        peb_size: u32,
    },
    NoPebSize,
    ErasedPeb {
        peb: u32,
    },
    Header {
        peb: u32,
        error: HeaderError,
    },
    Layout {
        peb: u32,
        error: GeometryError,
    },
    /// The PEB's offsets or image sequence differ from PEB 0's.
    Inconsistent {
        peb: u32,
    },
    LayoutLebOutOfPlace {
        peb: u32,
        lnum: u32,
    },
    LebOutOfOrder {
        peb: u32,
        volume_id: u32,
        lnum: u32,
        expected: u32,
    },
    StaticDataCrc {
        peb: u32,
        volume_id: u32,
        lnum: u32,
    },
    MissingLayoutVolume,
    TablesDiffer,
    Record {
        id: u32,
        error: HeaderError,
    },
    KindMismatch {
        id: u32,
        name: String,
    },
    TooManyLebs {
        id: u32,
        name: String,
        lebs: u32,
        reserved: u32,
    },
    UnlistedVolume {
        id: u32,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "the image: {error}"),
            Self::NotWholePebs { len, peb_size } => write!(
                f,
                "the image is {len} bytes, not a whole number of {peb_size}-byte PEBs"
            ),
            Self::NoPebSize => f.write_str(
                "no second erase-counter header at any power-of-two offset: not a UBI image of two or more PEBs",
            ),
            Self::ErasedPeb { peb } => write!(f, "PEB {peb} is erased: an image has none"),
            Self::Header { peb, error } => write!(f, "PEB {peb}: {error}"),
            Self::Layout { peb, error } => write!(f, "PEB {peb}: {error}"),
            Self::Inconsistent { peb } => write!(
                f,
                "PEB {peb}: its offsets or image sequence differ from PEB 0's"
            ),
            Self::LayoutLebOutOfPlace { peb, lnum } => {
                write!(f, "PEB {peb}: layout volume LEB {lnum} out of place")
            }
            Self::LebOutOfOrder {
                peb,
                volume_id,
                lnum,
                expected,
            } => write!(
                f,
                "PEB {peb}: volume {volume_id} LEB {lnum} where LEB {expected} was expected"
            ),
            Self::StaticDataCrc {
                peb,
                volume_id,
                lnum,
            } => write!(
                f,
                "PEB {peb}: volume {volume_id} LEB {lnum}'s data doesn't match its CRC"
            ),
            Self::MissingLayoutVolume => {
                f.write_str("the image lacks the layout volume's two copies")
            }
            Self::TablesDiffer => f.write_str("the layout volume's two volume tables differ"),
            Self::Record { id, error } => write!(f, "volume table record {id}: {error}"),
            Self::KindMismatch { id, name } => write!(
                f,
                "volume {id} ({name}) has another type in the table than in its LEBs"
            ),
            Self::TooManyLebs {
                id,
                name,
                lebs,
                reserved,
            } => write!(
                f,
                "volume {id} ({name}) carries {lebs} LEBs but reserves {reserved}"
            ),
            Self::UnlistedVolume { id } => write!(
                f,
                "PEBs belong to volume {id}, which the table doesn't list"
            ),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The eraseblock size of an image: the distance to the second PEB's EC
/// header, tried at every power of two from 4 KiB to 4 MiB.
pub fn find_peb_size(image: &mut (impl Read + Seek)) -> Result<u32, ImageError> {
    let len = image.seek(SeekFrom::End(0))?;
    let mut header = [0u8; HEADER_SIZE];
    for shift in 12..=22u32 {
        let size = 1u64 << shift;
        if size >= len {
            break;
        }
        image.seek(SeekFrom::Start(size))?;
        image.read_exact(&mut header)?;
        if matches!(EcHeader::decode(&header), Ok(Some(_))) {
            return Ok(size as u32);
        }
    }
    Err(ImageError::NoPebSize)
}

/// Per volume id: the LEBs seen, the first PEB, static bytes and the type.
struct Seen {
    lebs: u32,
    first_peb: u32,
    static_bytes: u64,
    kind: VolumeKind,
}

/// Reads and checks a whole image of `peb_size` eraseblocks.
pub fn read_image(image: &mut (impl Read + Seek), peb_size: u32) -> Result<Image, ImageError> {
    let len = image.seek(SeekFrom::End(0))?;
    image.rewind()?;
    if peb_size == 0 || len == 0 || len % u64::from(peb_size) != 0 {
        return Err(ImageError::NotWholePebs { len, peb_size });
    }
    let pebs = u32::try_from(len / u64::from(peb_size))
        .map_err(|_| ImageError::NotWholePebs { len, peb_size })?;
    if pebs < LAYOUT_VOLUME_LEBS {
        return Err(ImageError::MissingLayoutVolume);
    }

    let mut layout: Option<PebLayout> = None;
    let mut image_seq = 0;
    let mut tables: Vec<Vec<u8>> = Vec::new();
    let mut seen: BTreeMap<u32, Seen> = BTreeMap::new();
    let mut buffer = vec![0u8; peb_size as usize];
    for peb in 0..pebs {
        image.read_exact(&mut buffer)?;
        let header = |error| ImageError::Header { peb, error };
        let ec = EcHeader::decode(&buffer)
            .map_err(header)?
            .ok_or(ImageError::ErasedPeb { peb })?;
        let this = PebLayout {
            peb_size,
            vid_header_offset: ec.vid_header_offset,
            data_offset: ec.data_offset,
        };
        match layout {
            None => {
                this.validate()
                    .map_err(|error| ImageError::Layout { peb, error })?;
                ec.validate().map_err(header)?;
                layout = Some(this);
                image_seq = ec.image_seq;
            }
            Some(first) if first != this || ec.image_seq != image_seq => {
                return Err(ImageError::Inconsistent { peb });
            }
            Some(_) => {}
        }
        let vid_at = this.vid_header_offset as usize;
        let vid = VidHeader::decode(&buffer[vid_at..vid_at + HEADER_SIZE])
            .map_err(header)?
            .ok_or(ImageError::ErasedPeb { peb })?;
        vid.validate().map_err(header)?;
        let data = &buffer[this.data_offset as usize..];
        if vid.volume_id == LAYOUT_VOLUME_ID {
            if vid.lnum != tables.len() as u32 || peb >= LAYOUT_VOLUME_LEBS {
                return Err(ImageError::LayoutLebOutOfPlace {
                    peb,
                    lnum: vid.lnum,
                });
            }
            tables.push(data[..this.vtbl_records() as usize * VTBL_RECORD_SIZE].to_vec());
            continue;
        }
        let entry = seen.entry(vid.volume_id).or_insert(Seen {
            lebs: 0,
            first_peb: peb,
            static_bytes: 0,
            kind: vid.kind,
        });
        if vid.lnum != entry.lebs {
            return Err(ImageError::LebOutOfOrder {
                peb,
                volume_id: vid.volume_id,
                lnum: vid.lnum,
                expected: entry.lebs,
            });
        }
        if vid.kind == VolumeKind::Static {
            let size = vid.data_size as usize;
            if size > data.len() || ubi_crc(&data[..size]) != vid.data_crc {
                return Err(ImageError::StaticDataCrc {
                    peb,
                    volume_id: vid.volume_id,
                    lnum: vid.lnum,
                });
            }
            entry.static_bytes += size as u64;
        }
        entry.lebs += 1;
    }

    let Some(layout) = layout else {
        return Err(ImageError::MissingLayoutVolume);
    };
    if tables.len() != LAYOUT_VOLUME_LEBS as usize {
        return Err(ImageError::MissingLayoutVolume);
    }
    if tables[0] != tables[1] {
        return Err(ImageError::TablesDiffer);
    }
    let mut volumes = Vec::new();
    for (id, record) in tables[0]
        .as_chunks::<VTBL_RECORD_SIZE>()
        .0
        .iter()
        .enumerate()
    {
        let id = id as u32;
        let record =
            VtblRecord::decode(record).map_err(|error| ImageError::Record { id, error })?;
        let Some(record) = record else { continue };
        let found = seen.remove(&id);
        if found
            .as_ref()
            .is_some_and(|found| found.kind != record.kind)
        {
            return Err(ImageError::KindMismatch {
                id,
                name: record.name,
            });
        }
        let image_lebs = found.as_ref().map_or(0, |found| found.lebs);
        if image_lebs > record.reserved_pebs {
            return Err(ImageError::TooManyLebs {
                id,
                name: record.name,
                lebs: image_lebs,
                reserved: record.reserved_pebs,
            });
        }
        volumes.push(Volume {
            id,
            name: record.name,
            kind: record.kind,
            reserved_lebs: record.reserved_pebs,
            autoresize: record.autoresize,
            image_lebs,
            first_peb: found.as_ref().map(|found| found.first_peb),
            static_bytes: (record.kind == VolumeKind::Static)
                .then(|| found.as_ref().map_or(0, |found| found.static_bytes)),
        });
    }
    if let Some(id) = seen.keys().next() {
        return Err(ImageError::UnlistedVolume { id: *id });
    }
    Ok(Image {
        layout,
        image_seq,
        pebs,
        volumes,
    })
}

/// The bytes of `peb` worth programming: up to its last non-erased byte,
/// rounded up to whole pages (ubiformat's `drop_ffs`). Programming the
/// erased pages after it would leave them unusable for UBI's later writes.
pub fn programmed_len(peb: &[u8], page_size: usize) -> usize {
    let used = peb
        .iter()
        .rposition(|byte| *byte != ERASED)
        .map_or(0, |at| at + 1);
    used.div_ceil(page_size) * page_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_erased_pages_are_not_programmed() {
        let mut peb = vec![0xffu8; 8 * 512];
        assert_eq!(programmed_len(&peb, 512), 0);
        peb[0] = 0;
        assert_eq!(programmed_len(&peb, 512), 512);
        peb[1024 + 7] = 0x12;
        assert_eq!(programmed_len(&peb, 512), 1536);
    }
}
