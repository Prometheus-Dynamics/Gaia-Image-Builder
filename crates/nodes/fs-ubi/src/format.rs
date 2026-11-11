//! UBI's on-media format (`drivers/mtd/ubi/ubi-media.h`), as Gaia writes it.
//!
//! Each physical eraseblock (PEB) starts with an erase-counter (EC) header.
//! A PEB holding data has a volume-ID (VID) header at `vid_header_offset` and
//! its logical eraseblock (LEB) at `data_offset`. The layout volume's two
//! LEBs each hold a copy of the volume table. Every header field is
//! big-endian, and every header and record ends in a CRC of the bytes before
//! it ([`ubi_crc`]).
//!
//! This is the writer's half of `gaia_formats::ubi` on dev, with its decoders
//! for the checks in this crate's tests. The reader of a finished image stays
//! on dev. The device side reads the same bytes, so a change here changes every image.

use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};
use std::fmt;

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
/// levelling and one for atomic LEB changes.
pub const RUNTIME_RESERVED_PEBS: u64 = 2;
/// Linux's default `CONFIG_MTD_UBI_BEB_LIMIT`: PEBs UBI keeps for bad blocks
/// per 1024 PEBs of the whole flash device.
pub const DEFAULT_BAD_PEBS_PER_1024: u32 = 20;
/// An erased flash byte.
pub const ERASED: u8 = 0xff;
/// Erase counters above this are corrupt, not worn (`UBI_MAX_ERASECOUNTER`).
pub const MAX_ERASE_COUNTER: u64 = 0x7fff_ffff;

const EC: &str = "EC header";
const VID: &str = "VID header";
const VTBL: &str = "volume table record";

/// A volume's type, in VID headers and volume-table records.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Type, Serialize, Deserialize,
)]
#[type_key("gaia:ubi-volume-kind")]
#[serde(rename_all = "lowercase")]
pub enum VolumeKind {
    /// `UBI_VID_DYNAMIC`: read-write, no per-LEB data size or CRC.
    Dynamic,
    /// `UBI_VID_STATIC`: read-only, each LEB carries its size and CRC.
    Static,
}

impl VolumeKind {
    /// The code in a VID header and a volume-table record.
    pub fn code(self) -> u8 {
        match self {
            Self::Dynamic => 1,
            Self::Static => 2,
        }
    }

    /// The kind a code names, when it names one.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Dynamic),
            2 => Some(Self::Static),
            _ => None,
        }
    }
}

/// Why bytes are not a usable header or record, or a header's fields are not
/// ones the kernel accepts.
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

fn field(what: &'static str, field: &'static str, value: impl Into<u64>) -> HeaderError {
    HeaderError::Field {
        what,
        field,
        value: value.into(),
    }
}

/// The CRC-32 of UBI's headers (`crc32_le(UBI_CRC32_INIT, ...)`): the
/// standard reflected CRC-32 polynomial, started at all ones and without the
/// final inversion.
pub fn ubi_crc(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 != 0 {
                    0xedb8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    bytes.iter().fold(!0u32, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn be64(bytes: &[u8], at: usize) -> u64 {
    (u64::from(be32(bytes, at)) << 32) | u64::from(be32(bytes, at + 4))
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
    /// Gaia writes 0, as `ubinize` does.
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

    /// The kernel's `validate_ec_hdr`, less the checks that need the device.
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
    /// The layout volume's is [`LAYOUT_VOLUME_COMPAT`]; others' 0.
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
    /// changes: copy flag, data pad, CRC and sequence number 0.
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
        if self.volume_id == LAYOUT_VOLUME_ID && self.compat != LAYOUT_VOLUME_COMPAT {
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

/// One whole PEB: the EC header, the VID header and the LEB's bytes, with
/// erased bytes between them and after the data. `data` is at most a LEB;
/// the offsets come from `geometry`.
pub fn encode_peb(
    geometry: &crate::geometry::Geometry,
    ec: &EcHeader,
    vid: &VidHeader,
    data: &[u8],
) -> Vec<u8> {
    let mut peb = vec![ERASED; geometry.peb_size as usize];
    peb[..HEADER_SIZE].copy_from_slice(&ec.encode());
    let vid_at = geometry.vid_header_offset as usize;
    peb[vid_at..vid_at + HEADER_SIZE].copy_from_slice(&vid.encode());
    let data_at = geometry.data_offset as usize;
    peb[data_at..data_at + data.len()].copy_from_slice(data);
    peb
}

/// The volume table a layout-volume LEB holds: one record per possible
/// volume id, each volume's at its id and every other one empty.
pub fn encode_volume_table(
    geometry: &crate::geometry::Geometry,
    volumes: &[(u32, VtblRecord)],
) -> Result<Vec<u8>, HeaderError> {
    let records = geometry.vtbl_records() as usize;
    let empty = VtblRecord::empty();
    let mut table = Vec::with_capacity(records * VTBL_RECORD_SIZE);
    for _ in 0..records {
        table.extend_from_slice(&empty);
    }
    for (id, record) in volumes {
        let at = *id as usize;
        if at >= records {
            return Err(HeaderError::Field {
                what: "volume table",
                field: "volume id",
                value: u64::from(*id),
            });
        }
        table[at * VTBL_RECORD_SIZE..][..VTBL_RECORD_SIZE].copy_from_slice(&record.encode()?);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_is_crc32_without_the_final_inversion() {
        assert_eq!(ubi_crc(b"123456789"), 0x340b_c6d9);
        // An empty volume-table record's CRC, as every layout volume has it.
        assert_eq!(ubi_crc(&[0u8; VTBL_RECORD_CRC_SIZE]), 0xf116_c36b);
    }

    #[test]
    fn headers_round_trip_and_damage_is_named() {
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
        let dynamic_with_crc = VidHeader {
            data_crc: 1,
            ..VidHeader::new(VolumeKind::Dynamic, 0, 0)
        };
        assert!(dynamic_with_crc.validate().is_err());

        let record = VtblRecord::new(40, VolumeKind::Dynamic, "rootfs_a", false);
        let bytes = record.encode().expect("encodes");
        assert_eq!(VtblRecord::decode(&bytes), Ok(Some(record)));
        assert_eq!(VtblRecord::decode(&VtblRecord::empty()), Ok(None));
        let long = VtblRecord::new(1, VolumeKind::Dynamic, &"x".repeat(128), false);
        assert_eq!(long.encode(), Err(HeaderError::NameLength(128)));
    }
}
