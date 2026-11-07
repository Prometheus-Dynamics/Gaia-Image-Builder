//! UBI's on-flash headers (`ubi-media.h`), big-endian, each closed by its
//! CRC: the EC header at the start of every PEB, the VID header of every
//! PEB holding data, and the volume-table records the layout volume holds.
//!
//! `decode` checks what makes the bytes a header (magic, version, CRC,
//! volume type); `validate` checks the fields the way the kernel's
//! `validate_ec_hdr`, `validate_vid_hdr` and `vtbl_check` do, as far as they
//! can be checked without the device.

use super::{
    EC_MAGIC, ERASED, HEADER_CRC_SIZE, HEADER_SIZE, LAYOUT_VOLUME_ID, MAX_VOLUMES, VERSION,
    VID_MAGIC, VOLUME_NAME_MAX, VTBL_AUTORESIZE, VTBL_RECORD_CRC_SIZE, VTBL_RECORD_SIZE,
    VolumeKind, be32, be64,
};
use crate::crc::ubi_crc;
use std::fmt;

/// Erase counters above this are corrupt, not worn (`UBI_MAX_ERASECOUNTER`).
pub const MAX_ERASE_COUNTER: u64 = 0x7fff_ffff;

/// Why bytes are not a usable header or record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Fewer bytes than the header.
    TooShort {
        what: &'static str,
        len: usize,
    },
    Magic {
        what: &'static str,
        found: u32,
    },
    Version {
        what: &'static str,
        found: u8,
    },
    Crc {
        what: &'static str,
        stored: u32,
        computed: u32,
    },
    VolumeType {
        what: &'static str,
        code: u8,
    },
    /// A volume name of 0 or more than [`VOLUME_NAME_MAX`] bytes.
    NameLength(usize),
    NameNotUtf8,
    /// A field the kernel would refuse.
    Field {
        what: &'static str,
        field: &'static str,
        value: u64,
    },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { what, len } => write!(f, "{what} cut short at {len} bytes"),
            Self::Magic { what, found } => write!(f, "{what} magic {found:#010x}"),
            Self::Version { what, found } => write!(f, "{what} version {found}"),
            Self::Crc {
                what,
                stored,
                computed,
            } => write!(f, "{what} CRC {stored:#010x}, computed {computed:#010x}"),
            Self::VolumeType { what, code } => write!(f, "{what} volume type {code}"),
            Self::NameLength(len) => write!(f, "volume table record name length {len}"),
            Self::NameNotUtf8 => f.write_str("volume table record name isn't UTF-8"),
            Self::Field { what, field, value } => write!(f, "{what} {field} {value}"),
        }
    }
}

impl std::error::Error for HeaderError {}

const EC: &str = "EC header";
const VID: &str = "VID header";
const VTBL: &str = "volume table record";

fn field(what: &'static str, field: &'static str, value: impl Into<u64>) -> HeaderError {
    HeaderError::Field {
        what,
        field,
        value: value.into(),
    }
}

/// Erased flash: the header's every byte 0xff.
fn erased(bytes: &[u8]) -> bool {
    bytes.iter().take(HEADER_SIZE).all(|byte| *byte == ERASED)
}

fn check(bytes: &[u8], magic: u32, what: &'static str) -> Result<(), HeaderError> {
    if bytes.len() < HEADER_SIZE {
        return Err(HeaderError::TooShort {
            what,
            len: bytes.len(),
        });
    }
    let found = be32(bytes, 0);
    if found != magic {
        return Err(HeaderError::Magic { what, found });
    }
    if bytes[4] != VERSION {
        return Err(HeaderError::Version {
            what,
            found: bytes[4],
        });
    }
    let computed = ubi_crc(&bytes[..HEADER_CRC_SIZE]);
    let stored = be32(bytes, HEADER_CRC_SIZE);
    if computed != stored {
        return Err(HeaderError::Crc {
            what,
            stored,
            computed,
        });
    }
    Ok(())
}

fn seal(header: &mut [u8; HEADER_SIZE]) {
    let crc = ubi_crc(&header[..HEADER_CRC_SIZE]);
    header[HEADER_CRC_SIZE..].copy_from_slice(&crc.to_be_bytes());
}

/// The erase-counter header at offset 0 of every PEB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcHeader {
    /// Gaia writes 0, as `ubinize` does; a flasher writes each PEB's own.
    pub erase_counter: u64,
    pub vid_header_offset: u32,
    pub data_offset: u32,
    pub image_seq: u32,
}

impl EcHeader {
    /// `Ok(None)` for erased flash.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, HeaderError> {
        if bytes.len() >= HEADER_SIZE && erased(bytes) {
            return Ok(None);
        }
        check(bytes, EC_MAGIC, EC)?;
        Ok(Some(Self {
            erase_counter: be64(bytes, 8),
            vid_header_offset: be32(bytes, 16),
            data_offset: be32(bytes, 20),
            image_seq: be32(bytes, 24),
        }))
    }

    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&EC_MAGIC.to_be_bytes());
        header[4] = VERSION;
        header[8..16].copy_from_slice(&self.erase_counter.to_be_bytes());
        header[16..20].copy_from_slice(&self.vid_header_offset.to_be_bytes());
        header[20..24].copy_from_slice(&self.data_offset.to_be_bytes());
        header[24..28].copy_from_slice(&self.image_seq.to_be_bytes());
        seal(&mut header);
        header
    }

    pub fn validate(&self) -> Result<(), HeaderError> {
        if self.erase_counter > MAX_ERASE_COUNTER {
            return Err(field(EC, "erase counter", self.erase_counter));
        }
        if self.vid_header_offset < HEADER_SIZE as u32 || !self.vid_header_offset.is_multiple_of(8)
        {
            return Err(field(EC, "VID header offset", self.vid_header_offset));
        }
        if u64::from(self.data_offset) < u64::from(self.vid_header_offset) + HEADER_SIZE as u64 {
            return Err(field(EC, "data offset", self.data_offset));
        }
        Ok(())
    }
}

/// The volume-ID header of a PEB holding a LEB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VidHeader {
    pub kind: VolumeKind,
    /// 1 when the LEB was copied by wear levelling; Gaia writes 0.
    pub copy_flag: u8,
    /// The layout volume's is [`super::LAYOUT_VOLUME_COMPAT`]; others' 0.
    pub compat: u8,
    pub volume_id: u32,
    pub lnum: u32,
    /// Static volumes only: the bytes in this LEB, the volume's LEB count
    /// and the CRC of this LEB's bytes. 0 for dynamic ones.
    pub data_size: u32,
    pub used_ebs: u32,
    /// Gaia writes 0: every volume has alignment 1.
    pub data_pad: u32,
    pub data_crc: u32,
    /// Gaia writes 0, as `ubinize` does: the kernel numbers writes from the
    /// highest it finds.
    pub sqnum: u64,
}

impl VidHeader {
    /// A VID header with Gaia's defaults for the fields only the kernel
    /// changes: copy flag, data pad and sequence number 0.
    pub fn new(kind: VolumeKind, volume_id: u32, lnum: u32) -> Self {
        Self {
            kind,
            copy_flag: 0,
            compat: 0,
            volume_id,
            lnum,
            data_size: 0,
            used_ebs: 0,
            data_pad: 0,
            data_crc: 0,
            sqnum: 0,
        }
    }

    /// `Ok(None)` for erased flash: a PEB holding no LEB.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, HeaderError> {
        if bytes.len() >= HEADER_SIZE && erased(bytes) {
            return Ok(None);
        }
        check(bytes, VID_MAGIC, VID)?;
        let kind = VolumeKind::from_code(bytes[5]).ok_or(HeaderError::VolumeType {
            what: VID,
            code: bytes[5],
        })?;
        Ok(Some(Self {
            kind,
            copy_flag: bytes[6],
            compat: bytes[7],
            volume_id: be32(bytes, 8),
            lnum: be32(bytes, 12),
            data_size: be32(bytes, 20),
            used_ebs: be32(bytes, 24),
            data_pad: be32(bytes, 28),
            data_crc: be32(bytes, 32),
            sqnum: be64(bytes, 40),
        }))
    }

    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&VID_MAGIC.to_be_bytes());
        header[4] = VERSION;
        header[5] = self.kind.code();
        header[6] = self.copy_flag;
        header[7] = self.compat;
        header[8..12].copy_from_slice(&self.volume_id.to_be_bytes());
        header[12..16].copy_from_slice(&self.lnum.to_be_bytes());
        header[20..24].copy_from_slice(&self.data_size.to_be_bytes());
        header[24..28].copy_from_slice(&self.used_ebs.to_be_bytes());
        header[28..32].copy_from_slice(&self.data_pad.to_be_bytes());
        header[32..36].copy_from_slice(&self.data_crc.to_be_bytes());
        header[40..48].copy_from_slice(&self.sqnum.to_be_bytes());
        seal(&mut header);
        header
    }

    /// The kernel's `validate_vid_hdr`, less the checks that need the
    /// device's LEB size.
    pub fn validate(&self) -> Result<(), HeaderError> {
        if !matches!(self.compat, 0 | 1 | 2 | 4 | 5) {
            return Err(field(VID, "compat", self.compat));
        }
        if self.copy_flag > 1 {
            return Err(field(VID, "copy flag", self.copy_flag));
        }
        if self.volume_id >= MAX_VOLUMES && self.volume_id != LAYOUT_VOLUME_ID {
            return Err(field(VID, "volume id", self.volume_id));
        }
        if self.volume_id == LAYOUT_VOLUME_ID && self.compat != super::LAYOUT_VOLUME_COMPAT {
            return Err(field(VID, "layout volume compat", self.compat));
        }
        if self.volume_id < MAX_VOLUMES && self.compat != 0 {
            return Err(field(VID, "compat", self.compat));
        }
        if self.lnum > i32::MAX as u32 {
            return Err(field(VID, "LEB number", self.lnum));
        }
        match self.kind {
            VolumeKind::Static => {
                if self.used_ebs == 0 || self.lnum >= self.used_ebs {
                    return Err(field(VID, "used LEBs", self.used_ebs));
                }
                if self.data_size == 0 {
                    return Err(field(VID, "data size", self.data_size));
                }
            }
            VolumeKind::Dynamic => {
                if self.used_ebs != 0 {
                    return Err(field(VID, "used LEBs", self.used_ebs));
                }
                if self.copy_flag == 0 && self.data_crc != 0 {
                    return Err(field(VID, "data CRC", self.data_crc));
                }
                if self.copy_flag == 0 && self.data_size != 0 {
                    return Err(field(VID, "data size", self.data_size));
                }
            }
        }
        Ok(())
    }
}

/// One volume-table record that names a volume. A record whose
/// `reserved_pebs` is 0 is empty: no volume has its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtblRecord {
    pub reserved_pebs: u32,
    /// Gaia writes 1.
    pub alignment: u32,
    pub data_pad: u32,
    pub kind: VolumeKind,
    /// Set while `ubiupdatevol` writes the volume, cleared when it is done.
    pub upd_marker: bool,
    pub name: String,
    pub autoresize: bool,
}

impl VtblRecord {
    /// A record with Gaia's defaults: alignment 1, no data pad, no update.
    pub fn new(reserved_pebs: u32, kind: VolumeKind, name: &str, autoresize: bool) -> Self {
        Self {
            reserved_pebs,
            alignment: 1,
            data_pad: 0,
            kind,
            upd_marker: false,
            name: name.to_string(),
            autoresize,
        }
    }

    /// An empty record: zeros and their CRC (`0xf116c36b`).
    pub fn empty() -> [u8; VTBL_RECORD_SIZE] {
        let mut bytes = [0u8; VTBL_RECORD_SIZE];
        seal_record(&mut bytes);
        bytes
    }

    /// `Ok(None)` for an empty record.
    pub fn decode(record: &[u8]) -> Result<Option<Self>, HeaderError> {
        if record.len() < VTBL_RECORD_SIZE {
            return Err(HeaderError::TooShort {
                what: VTBL,
                len: record.len(),
            });
        }
        let computed = ubi_crc(&record[..VTBL_RECORD_CRC_SIZE]);
        let stored = be32(record, VTBL_RECORD_CRC_SIZE);
        if computed != stored {
            return Err(HeaderError::Crc {
                what: VTBL,
                stored,
                computed,
            });
        }
        let reserved_pebs = be32(record, 0);
        if reserved_pebs == 0 {
            return Ok(None);
        }
        let kind = VolumeKind::from_code(record[12]).ok_or(HeaderError::VolumeType {
            what: VTBL,
            code: record[12],
        })?;
        let name_len = usize::from(u16::from_be_bytes([record[14], record[15]]));
        if name_len == 0 || name_len > VOLUME_NAME_MAX {
            return Err(HeaderError::NameLength(name_len));
        }
        let name = std::str::from_utf8(&record[16..16 + name_len])
            .map_err(|_| HeaderError::NameNotUtf8)?
            .to_string();
        Ok(Some(Self {
            reserved_pebs,
            alignment: be32(record, 4),
            data_pad: be32(record, 8),
            kind,
            upd_marker: record[13] != 0,
            name,
            autoresize: record[144] & VTBL_AUTORESIZE != 0,
        }))
    }

    /// The record's bytes, refused when [`Self::validate`] fails.
    pub fn encode(&self) -> Result<[u8; VTBL_RECORD_SIZE], HeaderError> {
        self.validate()?;
        let mut bytes = [0u8; VTBL_RECORD_SIZE];
        bytes[0..4].copy_from_slice(&self.reserved_pebs.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.alignment.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.data_pad.to_be_bytes());
        bytes[12] = self.kind.code();
        bytes[13] = u8::from(self.upd_marker);
        bytes[14..16].copy_from_slice(&(self.name.len() as u16).to_be_bytes());
        bytes[16..16 + self.name.len()].copy_from_slice(self.name.as_bytes());
        bytes[144] = if self.autoresize { VTBL_AUTORESIZE } else { 0 };
        seal_record(&mut bytes);
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), HeaderError> {
        if self.name.is_empty() || self.name.len() > VOLUME_NAME_MAX {
            return Err(HeaderError::NameLength(self.name.len()));
        }
        if self.reserved_pebs == 0 {
            return Err(field(VTBL, "reserved PEBs", self.reserved_pebs));
        }
        if self.alignment == 0 {
            return Err(field(VTBL, "alignment", self.alignment));
        }
        if self.data_pad >= self.alignment {
            return Err(field(VTBL, "data pad", self.data_pad));
        }
        Ok(())
    }
}

fn seal_record(record: &mut [u8; VTBL_RECORD_SIZE]) {
    let crc = ubi_crc(&record[..VTBL_RECORD_CRC_SIZE]);
    record[VTBL_RECORD_CRC_SIZE..].copy_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip_and_name_damage() {
        let ec = EcHeader {
            erase_counter: 7,
            vid_header_offset: 2048,
            data_offset: 4096,
            image_seq: 0x1234_5678,
        };
        let mut bytes = ec.encode();
        assert_eq!(EcHeader::decode(&bytes), Ok(Some(ec)));
        assert_eq!(ec.validate(), Ok(()));
        bytes[9] ^= 1;
        assert!(matches!(
            EcHeader::decode(&bytes),
            Err(HeaderError::Crc { .. })
        ));
        assert_eq!(EcHeader::decode(&[0xff; 64]), Ok(None));
        assert!(matches!(
            EcHeader::decode(&[0; 10]),
            Err(HeaderError::TooShort { len: 10, .. })
        ));

        let vid = VidHeader {
            data_size: 100,
            used_ebs: 10,
            data_crc: 0xdead_beef,
            sqnum: 3,
            ..VidHeader::new(VolumeKind::Static, 3, 9)
        };
        assert_eq!(VidHeader::decode(&vid.encode()), Ok(Some(vid)));
        assert_eq!(vid.validate(), Ok(()));
        assert!(matches!(
            VidHeader::decode(&ec.encode()),
            Err(HeaderError::Magic {
                found: super::EC_MAGIC,
                ..
            })
        ));
        let mut bytes = vid.encode();
        bytes[5] = 9;
        let crc = ubi_crc(&bytes[..60]);
        bytes[60..].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(
            VidHeader::decode(&bytes),
            Err(HeaderError::VolumeType { code: 9, .. })
        ));
        let dynamic_with_crc = VidHeader {
            data_crc: 1,
            ..VidHeader::new(VolumeKind::Dynamic, 0, 0)
        };
        assert!(dynamic_with_crc.validate().is_err());

        let record = VtblRecord::new(40, VolumeKind::Dynamic, "rootfs_a", false);
        let bytes = record.encode().unwrap();
        assert_eq!(VtblRecord::decode(&bytes), Ok(Some(record)));
        assert_eq!(VtblRecord::decode(&VtblRecord::empty()), Ok(None));
        assert_eq!(be32(&VtblRecord::empty(), 168), 0xf116_c36b);
        let mut damaged = bytes;
        damaged[20] ^= 1;
        assert!(matches!(
            VtblRecord::decode(&damaged),
            Err(HeaderError::Crc { .. })
        ));
        let long = VtblRecord::new(1, VolumeKind::Dynamic, &"x".repeat(128), false);
        assert_eq!(long.encode(), Err(HeaderError::NameLength(128)));
    }
}
