//! Where the headers and the data sit in each PEB, derived from the flash's
//! page and sub-page sizes as `ubinize` and the kernel derive them.

use super::{HEADER_SIZE, MAX_VOLUMES, VTBL_RECORD_SIZE};
use std::fmt;

/// The header and data offsets of every PEB of an image. The kernel derives
/// them from the MTD device and refuses an image whose offsets differ.
/// [`crate::ubi::read_image`] reads them from the EC headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PebLayout {
    pub peb_size: u32,
    pub vid_header_offset: u32,
    pub data_offset: u32,
}

impl PebLayout {
    pub fn leb_size(&self) -> u32 {
        self.peb_size - self.data_offset
    }

    /// The records in the volume table: one per possible volume id, as many
    /// as fit a LEB, at most [`MAX_VOLUMES`].
    pub fn vtbl_records(&self) -> u32 {
        MAX_VOLUMES.min(self.leb_size() / VTBL_RECORD_SIZE as u32)
    }

    /// What makes the layout unusable: the VID header overlapping the EC
    /// header or the data, or no room for data.
    pub fn validate(&self) -> Result<(), GeometryError> {
        let header = HEADER_SIZE as u64;
        if u64::from(self.vid_header_offset) < header
            || u64::from(self.vid_header_offset) + header > u64::from(self.data_offset)
            || self.data_offset >= self.peb_size
        {
            return Err(GeometryError::Offsets {
                peb_size: self.peb_size,
                vid_header_offset: self.vid_header_offset,
                data_offset: self.data_offset,
            });
        }
        Ok(())
    }
}

/// A UBI image's flash geometry, in bytes: the flash's page size and the
/// [`PebLayout`] derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub peb_size: u32,
    pub min_io_size: u32,
    pub vid_header_offset: u32,
    pub data_offset: u32,
}

/// Why a geometry is refused. The messages are Gaia's build errors.
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
    Offsets {
        peb_size: u32,
        vid_header_offset: u32,
        data_offset: u32,
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
            Self::Offsets {
                peb_size,
                vid_header_offset,
                data_offset,
            } => write!(
                f,
                "VID header at {vid_header_offset}, data at {data_offset}, in a {peb_size}-byte PEB"
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

    pub fn layout(&self) -> PebLayout {
        PebLayout {
            peb_size: self.peb_size,
            vid_header_offset: self.vid_header_offset,
            data_offset: self.data_offset,
        }
    }

    pub fn leb_size(&self) -> u32 {
        self.layout().leb_size()
    }

    /// The records in the volume table: one per possible volume id.
    pub fn vtbl_records(&self) -> u32 {
        self.layout().vtbl_records()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubinize_defaults_and_refusals() {
        let nand = Geometry::new(128 * 1024, 2048, None, None).unwrap();
        assert_eq!((nand.vid_header_offset, nand.data_offset), (2048, 4096));
        assert_eq!((nand.leb_size(), nand.vtbl_records()), (126_976, 128));
        let nor = Geometry::new(64 * 1024, 1, None, Some(64)).unwrap();
        assert_eq!((nor.vid_header_offset, nor.data_offset), (64, 128));
        assert_eq!(
            Geometry::new(4096, 512, None, None).unwrap().vtbl_records(),
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
        let error = Geometry::new(128 * 1024, 2048, None, Some(32)).unwrap_err();
        assert!(error.to_string().contains("multiple of 8"), "{error}");
        assert!(nand.layout().validate().is_ok());
        let overlapping = PebLayout {
            data_offset: 2080,
            ..nand.layout()
        };
        assert!(overlapping.validate().is_err());
    }
}
