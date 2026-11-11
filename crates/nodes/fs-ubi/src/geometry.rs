//! Where the headers and the data sit in each PEB, from the flash's page and
//! sub-page sizes, as `ubinize` and the kernel derive them; and the PEBs an
//! attached image needs, with UBI's bad-block reserve.

use crate::format::{
    HEADER_SIZE, LAYOUT_VOLUME_LEBS, MAX_VOLUMES, RUNTIME_RESERVED_PEBS, VTBL_RECORD_SIZE,
};
use std::fmt;

/// A UBI image's flash geometry, in bytes: the flash's page size and the
/// offsets every PEB of the image uses. The kernel derives the same offsets
/// from the MTD device and refuses an image whose differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub peb_size: u32,
    pub min_io_size: u32,
    pub vid_header_offset: u32,
    pub data_offset: u32,
}

/// Why a geometry is refused. The messages are the build's errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeometryError {
    TooLarge {
        name: &'static str,
        value: u64,
    },
    NotPowersOfTwo {
        peb_size: u32,
        min_io_size: u32,
    },
    PebTooSmall {
        peb_size: u32,
        min_io_size: u32,
    },
    SubPage {
        sub_page_size: u32,
        min_io_size: u32,
    },
    VidHeaderOffset {
        vid_header_offset: u32,
    },
    NoRoomForData {
        data_offset: u32,
        peb_size: u32,
    },
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { name, value } => write!(f, "the {name} ({value}) is too large"),
            Self::NotPowersOfTwo {
                peb_size,
                min_io_size,
            } => write!(
                f,
                "the PEB size ({peb_size}) and minimum I/O size ({min_io_size}) must be powers of two"
            ),
            Self::PebTooSmall {
                peb_size,
                min_io_size,
            } => write!(
                f,
                "a PEB of {peb_size} bytes holds less than two {min_io_size}-byte pages"
            ),
            Self::SubPage {
                sub_page_size,
                min_io_size,
            } => write!(
                f,
                "the sub-page size ({sub_page_size}) must be a power of two no larger than the minimum I/O size ({min_io_size})"
            ),
            Self::VidHeaderOffset { vid_header_offset } => write!(
                f,
                "the VID header offset ({vid_header_offset}) must be a multiple of 8 past the 64-byte EC header"
            ),
            Self::NoRoomForData {
                data_offset,
                peb_size,
            } => write!(
                f,
                "the data offset ({data_offset}) leaves no room in a {peb_size}-byte PEB"
            ),
        }
    }
}

impl std::error::Error for GeometryError {}

impl Geometry {
    /// The geometry `ubinize -p peb -m min_io [-s sub_page] [-O vid_offset]`
    /// uses: the VID header at the sub-page size (the minimum I/O size when
    /// the flash has no sub-pages), and data at the next minimum-I/O unit
    /// after it.
    pub fn new(
        peb_size: u64,
        min_io_size: u64,
        sub_page_size: Option<u64>,
        vid_header_offset: Option<u64>,
    ) -> Result<Self, GeometryError> {
        let small = |name: &'static str, value: u64| {
            u32::try_from(value).map_err(|_| GeometryError::TooLarge { name, value })
        };
        let peb_size = small("PEB size", peb_size)?;
        let min_io_size = small("minimum I/O size", min_io_size)?;
        if !min_io_size.is_power_of_two() || !peb_size.is_power_of_two() {
            return Err(GeometryError::NotPowersOfTwo {
                peb_size,
                min_io_size,
            });
        }
        if peb_size < min_io_size.saturating_mul(2) {
            return Err(GeometryError::PebTooSmall {
                peb_size,
                min_io_size,
            });
        }
        let sub_page_size = small("sub-page size", sub_page_size.unwrap_or(min_io_size.into()))?;
        if !sub_page_size.is_power_of_two() || sub_page_size > min_io_size {
            return Err(GeometryError::SubPage {
                sub_page_size,
                min_io_size,
            });
        }
        let vid_header_offset = small(
            "VID header offset",
            vid_header_offset.unwrap_or(sub_page_size.into()),
        )?;
        if vid_header_offset < HEADER_SIZE as u32 || !vid_header_offset.is_multiple_of(8) {
            return Err(GeometryError::VidHeaderOffset { vid_header_offset });
        }
        let data_offset = (vid_header_offset + HEADER_SIZE as u32).next_multiple_of(min_io_size);
        if data_offset >= peb_size {
            return Err(GeometryError::NoRoomForData {
                data_offset,
                peb_size,
            });
        }
        Ok(Self {
            peb_size,
            min_io_size,
            vid_header_offset,
            data_offset,
        })
    }

    /// The data bytes one PEB holds: a logical eraseblock.
    pub fn leb_size(&self) -> u32 {
        self.peb_size - self.data_offset
    }

    /// The records in the volume table: one per possible volume id, as many
    /// as fit a LEB, at most [`MAX_VOLUMES`].
    pub fn vtbl_records(&self) -> u32 {
        MAX_VOLUMES.min(self.leb_size() / VTBL_RECORD_SIZE as u32)
    }
}

/// The PEBs an attached image needs before bad-block reserves: the layout
/// volume, UBI's two working PEBs and each volume's reserved LEBs.
pub fn reserved_pebs(volume_reserved_lebs: impl IntoIterator<Item = u32>) -> u64 {
    u64::from(LAYOUT_VOLUME_LEBS)
        + RUNTIME_RESERVED_PEBS
        + volume_reserved_lebs.into_iter().map(u64::from).sum::<u64>()
}

/// The bad-block reserve UBI keeps on a device of `device_pebs` PEBs
/// (`get_bad_peb_limit`): `per_1024` per 1024 PEBs, rounded up. With
/// `per_1024` 0 the kernel takes its figure from its config, which this
/// cannot know; this counts 1 PEB then.
pub fn bad_peb_reserve(device_pebs: u64, per_1024: u64) -> u64 {
    let limit = device_pebs * per_1024 / 1024;
    if limit * 1024 / per_1024.max(1) < device_pebs {
        limit + 1
    } else {
        limit
    }
}

/// The fewest PEBs a flash device needs to attach an image that reserves
/// `reserved_pebs`, counting the bad-block reserve that grows with it.
pub fn minimum_device_pebs(reserved_pebs: u64, per_1024: u64) -> u64 {
    let mut pebs = reserved_pebs;
    while pebs < reserved_pebs + bad_peb_reserve(pebs, per_1024) {
        pebs += 1;
    }
    pebs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubinize_defaults_and_refusals() {
        let nand = Geometry::new(128 * 1024, 2048, None, None).expect("NAND");
        assert_eq!((nand.vid_header_offset, nand.data_offset), (2048, 4096));
        assert_eq!((nand.leb_size(), nand.vtbl_records()), (126_976, 128));
        let nor = Geometry::new(64 * 1024, 1, None, Some(64)).expect("NOR");
        assert_eq!((nor.vid_header_offset, nor.data_offset), (64, 128));
        assert_eq!(
            Geometry::new(4096, 512, None, None)
                .expect("small")
                .vtbl_records(),
            17
        );
        assert!(matches!(
            Geometry::new(100_000, 2048, None, None),
            Err(GeometryError::NotPowersOfTwo { .. })
        ));
        assert!(matches!(
            Geometry::new(4096, 4096, None, None),
            Err(GeometryError::PebTooSmall { .. })
        ));
        assert!(matches!(
            Geometry::new(128 * 1024, 2048, Some(4096), None),
            Err(GeometryError::SubPage { .. })
        ));
        let error = Geometry::new(128 * 1024, 2048, None, Some(32)).expect_err("offset");
        assert!(error.to_string().contains("multiple of 8"), "{error}");
    }

    #[test]
    fn the_smallest_device_covers_the_bad_block_reserve() {
        assert_eq!(bad_peb_reserve(1024, 20), 20);
        assert_eq!(bad_peb_reserve(1000, 20), 20);
        assert_eq!(bad_peb_reserve(1025, 20), 21);
        assert_eq!(bad_peb_reserve(2048, 20), 40);
        assert_eq!(bad_peb_reserve(2048, 0), 1);
        let pebs = minimum_device_pebs(500, 20);
        assert_eq!(pebs, 510);
        assert!(pebs >= 500 + bad_peb_reserve(pebs, 20));
        assert!(pebs - 1 < 500 + bad_peb_reserve(pebs - 1, 20));
        assert_eq!(reserved_pebs([4, 4, 1]), 2 + 2 + 9);
    }
}
