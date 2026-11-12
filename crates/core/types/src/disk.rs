//! A disk's layout: its partition table, its partitions and raw blobs, as
//! `gaia.disk` takes them.

use crate::{PartitionRole, Size, StoreRef};
use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};

/// The partition table's scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Type, Serialize, Deserialize)]
#[type_key("gaia:table-scheme")]
#[serde(rename_all = "lowercase")]
pub enum TableScheme {
    Gpt,
    Mbr,
}

/// The partition table of a disk.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:disk-table")]
pub struct DiskTable {
    pub scheme: TableScheme,
    /// The first byte a partition may use. Unset: 1 MiB.
    pub first: Option<Size>,
    /// Partitions start on multiples of this. Unset: 1 MiB.
    pub alignment: Option<Size>,
    /// The disk's size. Unset: as large as its partitions, and for GPT its
    /// backup table.
    pub size: Option<Size>,
}

/// One partition of a disk.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:disk-partition")]
pub struct DiskPartition {
    pub name: String,
    pub role: PartitionRole,
    /// GPT: a type GUID. MBR: a type byte in hex, such as `0x83`.
    pub table_type: String,
    /// The MBR active flag; on GPT, the legacy BIOS bootable attribute.
    pub bootable: bool,
    /// Unset: the next boundary after the partition before it.
    pub offset: Option<Size>,
    /// Unset: the content's size, rounded up to a sector.
    pub size: Option<Size>,
    /// The file written at the partition's start; unset leaves it zero.
    pub content: Option<StoreRef>,
}

/// A raw blob (a bootloader, say) written at a fixed offset.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:raw-blob")]
pub struct RawBlob {
    pub name: String,
    /// Bytes from the disk's start.
    pub offset: Size,
    pub content: StoreRef,
}

/// The part of a disk the board gives, whatever the image: the table and
/// the partitions it needs with no content (a bootloader's environment, say).
/// A recipe's partitions follow them.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:disk-layout")]
pub struct DiskLayout {
    pub table: DiskTable,
    pub partitions: Vec<DiskPartition>,
}
