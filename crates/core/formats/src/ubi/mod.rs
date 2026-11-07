//! UBI images, the bytes `ubinize` writes and the kernel's UBI attaches
//! (`drivers/mtd/ubi/ubi-media.h`). Each physical eraseblock (PEB) starts
//! with an erase-counter (EC) header. Each PEB holding data has a volume-ID
//! (VID) header at `vid_header_offset` and the logical eraseblock (LEB) at
//! `data_offset`. The layout volume's two LEBs each hold a copy of the
//! volume table. Every header field is big-endian, and every CRC is
//! [`crate::crc::ubi_crc`].
//!
//! Gaia writes an image as: the layout volume in PEBs 0 and 1, then each
//! volume's data LEBs in volume order, one PEB per LEB. Volumes without
//! data and LEBs reserved past the data take no PEBs in the image; UBI hands
//! them out from the erased flash after it.

mod geometry;
mod headers;
#[cfg(feature = "io")]
mod image;
#[cfg(feature = "builder")]
mod peb;
mod reserve;

pub use geometry::{Geometry, GeometryError, PebLayout};
pub use headers::{EcHeader, HeaderError, MAX_ERASE_COUNTER, VidHeader, VtblRecord};
#[cfg(feature = "io")]
pub use image::{Image, ImageError, Volume, find_peb_size, programmed_len, read_image};
#[cfg(feature = "builder")]
pub use peb::{encode_peb, encode_volume_table};
pub use reserve::{bad_peb_reserve, minimum_device_pebs, reserved_pebs};

/// The EC header magic, "UBI#".
pub const EC_MAGIC: u32 = 0x5542_4923;
/// The VID header magic, "UBI!".
pub const VID_MAGIC: u32 = 0x5542_4921;
/// The on-media format version (`UBI_VERSION`).
pub const VERSION: u8 = 1;
/// The bytes of an EC or VID header.
pub const HEADER_SIZE: usize = 64;
/// The bytes of a header its CRC covers: all but the CRC itself.
pub const HEADER_CRC_SIZE: usize = 60;
/// The bytes of one volume-table record.
pub const VTBL_RECORD_SIZE: usize = 172;
/// The bytes of a volume-table record its CRC covers.
pub const VTBL_RECORD_CRC_SIZE: usize = VTBL_RECORD_SIZE - 4;
/// The most volumes a UBI device holds (`UBI_MAX_VOLUMES`).
pub const MAX_VOLUMES: u32 = 128;
/// The longest volume name, in bytes (`UBI_VOL_NAME_MAX`).
pub const VOLUME_NAME_MAX: usize = 127;
/// The layout volume's id (`UBI_LAYOUT_VOLUME_ID`).
pub const LAYOUT_VOLUME_ID: u32 = 0x7fff_efff;
/// The layout volume's LEBs: two copies of the volume table.
pub const LAYOUT_VOLUME_LEBS: u32 = 2;
/// `UBI_COMPAT_REJECT`, the layout volume's compat: an older UBI that does
/// not know the layout volume refuses the image.
pub const LAYOUT_VOLUME_COMPAT: u8 = 5;
/// The volume-table flag for the volume UBI grows over the free PEBs on
/// first attach (`UBI_VTBL_AUTORESIZE_FLG`).
pub const VTBL_AUTORESIZE: u8 = 0x01;
/// The PEBs UBI keeps for itself beyond the layout volume: one for wear
/// levelling (`WL_RESERVED_PEBS`) and one for atomic LEB changes
/// (`EBA_RESERVED_PEBS`).
pub const RUNTIME_RESERVED_PEBS: u64 = 2;
/// Linux's default `CONFIG_MTD_UBI_BEB_LIMIT`: PEBs UBI keeps for bad blocks
/// per 1024 PEBs of the whole flash device.
pub const DEFAULT_BAD_PEBS_PER_1024: u32 = 20;
/// An erased flash byte.
pub const ERASED: u8 = 0xff;

/// A volume's type, in VID headers and volume-table records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeKind {
    /// `UBI_VID_DYNAMIC`: read-write, no per-LEB data size or CRC.
    Dynamic,
    /// `UBI_VID_STATIC`: read-only, each LEB carries its size and CRC.
    Static,
}

impl VolumeKind {
    pub fn code(self) -> u8 {
        match self {
            Self::Dynamic => 1,
            Self::Static => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Dynamic),
            2 => Some(Self::Static),
            _ => None,
        }
    }
}

pub(crate) fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub(crate) fn be64(bytes: &[u8], at: usize) -> u64 {
    (u64::from(be32(bytes, at)) << 32) | u64::from(be32(bytes, at + 4))
}
