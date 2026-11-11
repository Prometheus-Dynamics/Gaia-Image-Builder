//! `gaia.disk`: one disk image from a partition table, the partitions in it
//! and raw blobs at fixed offsets, plus `gaia.uboot.env`: a U-Boot
//! environment, for a partition.
//!
//! The table is GPT or MBR, as the board says. Partitions go in order from
//! `first`, each on an `alignment` boundary (or at its own `offset`), each
//! sized by its `size` or its content. Blobs (bootloaders, the first stage
//! of a boot ROM) are written at their `offset` bytes from the disk's start.
//! The layout is checked before anything is written, and a refusal says
//! which rule broke ([`layout`]).
//!
//! The disk is written sparse: the zero runs are holes, so an empty
//! partition costs no space and the file is the same bytes on every host.
//! The GUIDs and the MBR disk signature are derived from the disk's `id` and
//! the partition names, so a `root=PARTUUID=` on a kernel command line stays
//! valid across rebuilds; a partition's GUID is not the table's, so no
//! description has to name one.
//!
//! The step's key is the disk's `id`, its table, its partitions and blobs
//! (every content by digest), so an edit to any of them remakes the disk and
//! nothing else.
//!
//! Not built here: MBR logical partitions (more than four partitions on an
//! MBR). A description that needs them is refused.

pub mod crc;
pub mod env;
pub mod layout;
pub mod table;

pub use env::{UBOOT_ENV_NODE, uboot_env, uboot_env_artifact, uboot_env_image};
pub use layout::{BlobInput, Layout, PartitionInput, Placed, layout};

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::Digest;
use gaia_types::{Image, ImageKind, ObjectKind, Partition, PartitionRole, Size, StoreRef};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.disk";
/// The domains the derived GUIDs and the MBR signature hash under.
const DISK_GUID_DOMAIN: &str = "gaia gpt disk guid\n";
const PARTITION_GUID_DOMAIN: &str = "gaia gpt partition guid\n";
const MBR_SIGNATURE_DOMAIN: &str = "gaia mbr disk signature\n";

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

/// One U-Boot environment variable.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:uboot-var")]
pub struct UbootVar {
    pub name: String,
    pub value: String,
}

/// The disk image of `partitions` and `blobs` on `table`: the node.
#[node(
    id = "gaia.disk",
    inputs(store, id, table, partitions, blobs),
    outputs(image)
)]
pub fn disk(
    store: &str,
    id: &str,
    table: DiskTable,
    partitions: Vec<DiskPartition>,
    blobs: Vec<RawBlob>,
    cx: &Ctx,
) -> Result<Image, StepError> {
    disk_image(store, id, &table, &partitions, &blobs, cx)
}

/// A content in the store: its path, its length and its digest.
struct Content {
    path: PathBuf,
    len: u64,
    digest: Digest,
}

fn content(store: &Store, what: &str, reference: &StoreRef) -> Result<Content, String> {
    if reference.kind != ObjectKind::File {
        return Err(format!(
            "{what} is a tree; a partition or blob takes a file"
        ));
    }
    let object = reference
        .object_ref()
        .ok_or_else(|| format!("{what} has a malformed digest '{}'", reference.digest))?;
    let path = store.path(&object);
    let len = std::fs::metadata(&path)
        .map_err(|e| format!("{what} is not in the store: {e}"))?
        .len();
    Ok(Content {
        path,
        len,
        digest: object.digest,
    })
}

/// The disk image, as [`disk`] makes it.
pub fn disk_image(
    store: &str,
    id: &str,
    table: &DiskTable,
    partitions: &[DiskPartition],
    blobs: &[RawBlob],
    cx: &Ctx,
) -> Result<Image, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, format!("disk '{id}': {e}"));

    let mut part_contents = Vec::with_capacity(partitions.len());
    for partition in partitions {
        let what = format!("partition '{}' content", partition.name);
        part_contents.push(match &partition.content {
            Some(reference) => Some(content(&store, &what, reference).map_err(fail)?),
            None => None,
        });
    }
    let mut blob_contents = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let what = format!("blob '{}' content", blob.name);
        blob_contents.push(content(&store, &what, &blob.content).map_err(fail)?);
    }

    let inputs: Vec<layout::PartitionInput> = partitions
        .iter()
        .zip(&part_contents)
        .map(|(partition, content)| layout::PartitionInput {
            name: partition.name.clone(),
            role: partition.role.clone(),
            table_type: partition.table_type.clone(),
            bootable: partition.bootable,
            offset: partition.offset.map(|s| s.0),
            size: partition.size.map(|s| s.0),
            content_len: content.as_ref().map(|c| c.len),
        })
        .collect();
    let blob_inputs: Vec<layout::BlobInput> = blobs
        .iter()
        .zip(&blob_contents)
        .map(|(blob, content)| layout::BlobInput {
            name: blob.name.clone(),
            offset: blob.offset.0,
            len: content.len,
        })
        .collect();
    let planned = layout(
        table.scheme,
        table.first.map(|s| s.0),
        table.alignment.map(|s| s.0),
        table.size.map(|s| s.0),
        &inputs,
        &blob_inputs,
    )
    .map_err(fail)?;
    let writes = table_writes(id, table.scheme, &planned, partitions).map_err(fail)?;

    let mut key = KeyBuilder::new(NAME, 1)
        .config("id", id)
        .config(
            "scheme",
            match table.scheme {
                TableScheme::Gpt => "gpt",
                TableScheme::Mbr => "mbr",
            },
        )
        .config("first", planned.first.to_string())
        .config("alignment", planned.alignment.to_string())
        .config("size", planned.size.to_string());
    for (index, (partition, placed)) in partitions.iter().zip(&planned.partitions).enumerate() {
        key = key.config(
            &format!("partition {index}"),
            format!(
                "{}|{}|{}|{}|{}|{}",
                partition.name,
                partition.role,
                partition.table_type,
                partition.bootable,
                placed.start,
                placed.size
            ),
        );
        if let Some(content) = &part_contents[index] {
            key = key.input(&format!("partition {index} content"), content.digest);
        }
    }
    for (index, (blob, placed)) in blobs.iter().zip(&planned.blobs).enumerate() {
        key = key
            .config(
                &format!("blob {index}"),
                format!("{}|{}|{}", blob.name, placed.start, placed.size),
            )
            .input(
                &format!("blob {index} content"),
                blob_contents[index].digest,
            );
    }

    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("image", Path::new("image.disk"))],
        |work| {
            let image = work.dir().join("image.disk");
            let mut file = File::create(&image).map_err(|e| fail(e.to_string()))?;
            file.set_len(planned.size)
                .map_err(|e| fail(e.to_string()))?;
            for (content, placed) in part_contents.iter().zip(&planned.partitions) {
                if let Some(content) = content {
                    copy_at(&mut file, &content.path, placed.start).map_err(fail)?;
                }
            }
            for (content, placed) in blob_contents.iter().zip(&planned.blobs) {
                copy_at(&mut file, &content.path, placed.start).map_err(fail)?;
            }
            // The table last: its sectors are kept clear of the content above.
            for (offset, bytes) in &writes {
                file.seek(SeekFrom::Start(*offset))
                    .and_then(|_| file.write_all(bytes))
                    .map_err(|e| fail(format!("write the partition table: {e}")))?;
            }
            file.sync_all().map_err(|e| fail(e.to_string()))
        },
    )?;

    let object = made.outputs["image"];
    let out = partitions
        .iter()
        .zip(&planned.partitions)
        .map(|(partition, placed)| Partition {
            name: partition.name.clone(),
            role: partition.role.clone(),
            table_type: partition.table_type.clone(),
            start: Size(placed.start),
            size: Size(placed.size),
            content: partition.content.clone(),
        })
        .collect();
    Ok(Image {
        kind: ImageKind::Disk,
        object: object.into(),
        size: Size(planned.size),
        partitions: out,
    })
}

/// The partition table's sectors for `planned`, as `(offset, bytes)`.
fn table_writes(
    id: &str,
    scheme: TableScheme,
    planned: &Layout,
    partitions: &[DiskPartition],
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    match scheme {
        TableScheme::Gpt => {
            let disk_guid = table::derived_guid(DISK_GUID_DOMAIN, &[id.as_bytes()]);
            let mut entries = Vec::with_capacity(partitions.len());
            for (partition, placed) in partitions.iter().zip(&planned.partitions) {
                let type_guid = table::guid_bytes(&partition.table_type)
                    .map_err(|e| format!("partition '{}' table_type: {e}", partition.name))?;
                entries.push(table::GptEntry {
                    name: partition.name.clone(),
                    type_guid,
                    guid: table::derived_guid(
                        PARTITION_GUID_DOMAIN,
                        &[&disk_guid[..], partition.name.as_bytes()],
                    ),
                    start: placed.start,
                    size: placed.size,
                    bootable: partition.bootable,
                });
            }
            table::gpt_writes(planned.size, disk_guid, table::GPT_HEAD_SECTORS, &entries)
        }
        TableScheme::Mbr => {
            let mut entries = Vec::with_capacity(partitions.len());
            for (partition, placed) in partitions.iter().zip(&planned.partitions) {
                let hex = partition
                    .table_type
                    .trim_start_matches("0x")
                    .trim_start_matches("0X");
                let type_byte = u8::from_str_radix(hex, 16).map_err(|_| {
                    format!(
                        "partition '{}' table_type '{}' is not an MBR type byte in hex",
                        partition.name, partition.table_type
                    )
                })?;
                entries.push(table::MbrEntry {
                    start: placed.start,
                    size: placed.size,
                    type_byte,
                    bootable: partition.bootable,
                });
            }
            let signature = table::derived_guid(MBR_SIGNATURE_DOMAIN, &[id.as_bytes()]);
            let mut sig = [0u8; 4];
            sig.copy_from_slice(&signature[..4]);
            table::mbr_writes(sig, &entries)
        }
    }
}

/// Copies `source` into `dest` at `offset`.
fn copy_at(dest: &mut File, source: &Path, offset: u64) -> Result<(), String> {
    let mut from = File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
    dest.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("seek to {offset}: {e}"))?;
    std::io::copy(&mut from, dest)
        .map(drop)
        .map_err(|e| format!("copy {} at {offset}: {e}", source.display()))
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 2] = [&DISK_NODE, &UBOOT_ENV_NODE];
