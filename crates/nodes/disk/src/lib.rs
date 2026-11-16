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
pub mod env_place;
pub mod layout;
pub mod table;

pub use env::{UBOOT_ENV_NODE, parse_env_text, uboot_env, uboot_env_artifact, uboot_env_image};
pub use layout::{BlobInput, Layout, PartitionInput, Placed, layout};

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::Digest;
use gaia_types::{
    Artifact, Image, ImageKind, ObjectKind, Partition, PartitionRole, Size, StoreRef,
};
pub use gaia_types::{DiskLayout, DiskPartition, DiskTable, RawBlob, TableScheme};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.disk";
/// The domains the derived GUIDs and the MBR signature hash under.
const DISK_GUID_DOMAIN: &str = "gaia gpt disk guid\n";
const PARTITION_GUID_DOMAIN: &str = "gaia gpt partition guid\n";
const MBR_SIGNATURE_DOMAIN: &str = "gaia mbr disk signature\n";

/// One U-Boot environment variable.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:uboot-var")]
pub struct UbootVar {
    pub name: String,
    pub value: String,
}

/// The disk image of `layout` (the table and the board's partitions), then
/// the partitions fed in, in port order, and the raw `blobs`, when there are
/// any: the node.
///
/// A CPU node takes one fan-in, and the partitions have it, so the blobs
/// come as one list, from `gaia.disk.blobs`.
///
/// `bootloader` is the OS's bootloader output, when the disk has one. When
/// it holds U-Boot's final configuration (`u-boot.config`), the places
/// U-Boot would save its environment to are checked against the layout
/// ([`env_place`]) and a disk where a `saveenv` would write over the table,
/// a bootloader area or a partition is refused.
#[node(
    id = "gaia.disk",
    inputs(store, id, layout, partitions, blobs, bootloader),
    outputs(image)
)]
pub fn disk(
    store: &str,
    id: &str,
    layout: &DiskLayout,
    partitions: FanIn<'_, DiskPartition>,
    blobs: Option<&Vec<RawBlob>>,
    bootloader: Option<&Artifact>,
    cx: &Ctx,
) -> Result<Image, StepError> {
    let fail = |e: String| StepError::body(NAME, format!("disk '{id}': {e}"));
    let fed: Vec<DiskPartition> = partitions.iter().map(|p| (*p).clone()).collect();
    let all = fill(&layout.partitions, &fed).map_err(fail)?;
    let blobs = blobs.map(Vec::as_slice).unwrap_or_default();
    let uboot_config = match bootloader {
        Some(from) => bootloader_config(store, from).map_err(fail)?,
        None => None,
    };
    disk_image_checked(
        store,
        id,
        &layout.table,
        &all,
        blobs,
        uboot_config.as_deref(),
        cx,
    )
}

/// U-Boot's final configuration (`u-boot.config`) in the bootloader output,
/// when it is there.
///
/// # Errors
///
/// An output not in the store, or a config that cannot be read.
pub fn bootloader_config(store: &str, from: &Artifact) -> Result<Option<String>, String> {
    if from.object.kind != ObjectKind::Tree {
        return Ok(None);
    }
    let store = Store::open(store).map_err(|e| e.to_string())?;
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| "the bootloader output is not in the store".to_string())?;
    let file = store.path(&object).join("u-boot.config");
    if !file.is_file() {
        return Ok(None);
    }
    std::fs::read_to_string(&file)
        .map(Some)
        .map_err(|e| format!("U-Boot's config: {e}"))
}

/// The board's partitions with the recipe's: a recipe partition named as a
/// board partition is that partition, and gives only its content. The
/// board's place, size, type, role and flag stand, so a recipe can fill a
/// layout without restating it; what a recipe partition states that the
/// board says otherwise is refused, by name. Any other recipe partition
/// follows the board's, in order.
///
/// # Errors
///
/// A recipe partition that differs from the board's of its name, or a
/// board partition filled twice.
pub fn fill(board: &[DiskPartition], fed: &[DiskPartition]) -> Result<Vec<DiskPartition>, String> {
    let mut all = board.to_vec();
    for partition in fed {
        let Some(at) = all
            .iter()
            .take(board.len())
            .position(|have| have.name == partition.name)
        else {
            all.push(partition.clone());
            continue;
        };
        let have = &mut all[at];
        let name = &partition.name;
        let differs = |what: &str, board: String, recipe: String| {
            format!(
                "partition '{name}': the board's layout gives its {what} as {board}, and the recipe as {recipe}; the board's layout is the one to change"
            )
        };
        if !partition.table_type.is_empty() && partition.table_type != have.table_type {
            return Err(differs(
                "type",
                have.table_type.clone(),
                partition.table_type.clone(),
            ));
        }
        if let Some(size) = partition.size
            && have.size != Some(size)
        {
            return Err(differs(
                "size",
                have.size
                    .map_or("its content's".into(), |s| s.0.to_string()),
                size.0.to_string(),
            ));
        }
        if let Some(offset) = partition.offset
            && have.offset != Some(offset)
        {
            return Err(differs(
                "offset",
                have.offset
                    .map_or("the next boundary".into(), |s| s.0.to_string()),
                offset.0.to_string(),
            ));
        }
        if partition.bootable && !have.bootable {
            return Err(differs("active flag", "off".into(), "on".into()));
        }
        if have.content.is_some() {
            return Err(format!("partition '{name}' is filled twice"));
        }
        have.content.clone_from(&partition.content);
    }
    Ok(all)
}

/// One raw blob: the file `path` of the tree `from` (an OS's bootloader
/// output, say), written `offset` bytes from the disk's start: the node.
/// The file is stored on its own, so the disk keys on its bytes and not on
/// the rest of the tree.
#[node(
    id = "gaia.disk.blob",
    inputs(store, from, path, name, offset),
    outputs(blob)
)]
pub fn disk_blob(
    store: &str,
    from: &Artifact,
    path: &str,
    name: &str,
    offset: u64,
    cx: &Ctx,
) -> Result<RawBlob, StepError> {
    const BLOB: &str = "gaia.disk.blob";
    let fail = |why: String| StepError::body(BLOB, format!("blob '{name}': {why}"));
    let store = Store::open(store).map_err(StepError::store(BLOB))?;
    if from.object.kind != ObjectKind::Tree {
        return Err(fail(
            "its source is a file, not a tree to pick a file from".into(),
        ));
    }
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| fail("its source is not in the store".into()))?;
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !plain {
        return Err(fail(format!(
            "'{path}' is not a path inside the tree: no leading /, no . or .."
        )));
    }
    let file = store.path(&object).join(relative);
    if !file.is_file() {
        let mut names: Vec<String> = std::fs::read_dir(store.path(&object))
            .map(|dir| {
                dir.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        return Err(fail(format!(
            "the tree has no file '{path}'; it has {}",
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            }
        )));
    }
    let key = KeyBuilder::new(BLOB, 1)
        .config("path", path)
        .input("from", object.digest)
        .finish();
    let made = Step::new(&store, cx, BLOB, key).look_up_or_make(
        &[("blob", Path::new("blob.bin"))],
        |work| {
            gaia_store::copy_file(&file, work.dir().join("blob.bin"))
                .map(|_| ())
                .map_err(|e| fail(e.to_string()))
        },
    )?;
    Ok(RawBlob {
        name: name.to_string(),
        offset: Size(offset),
        content: made.outputs["blob"].into(),
    })
}

/// Any number of raw blobs, on ports `blobs0`, `blobs1` and so on, as the
/// one list `gaia.disk` takes: the node.
#[node(id = "gaia.disk.blobs", inputs(blobs), outputs(list))]
pub fn disk_blobs(blobs: FanIn<'_, RawBlob>) -> Vec<RawBlob> {
    blobs.iter().map(|blob| (*blob).clone()).collect()
}

/// One partition of a disk, holding the image `content` when there is one,
/// or the file `file` (a partition's image another program wrote): the
/// node. A recipe gives one or the other; given both, `content` is taken.
/// `role` is a partition role's name; `offset` and `size` of 0 leave the
/// place and size to the layout (the next boundary, the content's size).
#[node(
    id = "gaia.disk.partition",
    inputs(name, role, table_type, bootable, offset, size, content, file),
    outputs(partition)
)]
#[allow(clippy::too_many_arguments)]
pub fn disk_partition(
    name: &str,
    role: &str,
    table_type: &str,
    bootable: bool,
    offset: u64,
    size: u64,
    content: Option<&Image>,
    file: Option<&Artifact>,
) -> DiskPartition {
    DiskPartition {
        name: name.to_string(),
        role: PartitionRole::parse(role),
        table_type: table_type.to_string(),
        bootable,
        offset: (offset != 0).then_some(Size(offset)),
        size: (size != 0).then_some(Size(size)),
        content: content
            .map(|image| image.object.clone())
            .or_else(|| file.map(|artifact| artifact.object.clone())),
    }
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

/// The disk image, as [`disk`] makes it with no bootloader output.
pub fn disk_image(
    store: &str,
    id: &str,
    table: &DiskTable,
    partitions: &[DiskPartition],
    blobs: &[RawBlob],
    cx: &Ctx,
) -> Result<Image, StepError> {
    disk_image_checked(store, id, table, partitions, blobs, None, cx)
}

/// The disk image, as [`disk`] makes it: with `uboot_config`, U-Boot's final
/// configuration, its environment's places are checked against the layout.
pub fn disk_image_checked(
    store: &str,
    id: &str,
    table: &DiskTable,
    partitions: &[DiskPartition],
    blobs: &[RawBlob],
    uboot_config: Option<&str>,
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
    if let Some(config) = uboot_config {
        let names: Vec<String> = blobs.iter().map(|b| b.name.clone()).collect();
        let found = env_place::problems(config, &planned, &inputs, &names);
        if !found.is_empty() {
            return Err(fail(format!(
                "U-Boot's environment would be saved over the disk's own data: {}",
                found.join("; ")
            )));
        }
    }
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
pub const NODES: [&NodeRecord; 5] = [
    &DISK_NODE,
    &DISK_PARTITION_NODE,
    &DISK_BLOB_NODE,
    &DISK_BLOBS_NODE,
    &UBOOT_ENV_NODE,
];

#[cfg(test)]
mod fill_tests {
    use super::*;
    use gaia_types::{ObjectKind, PartitionRole, Size, StoreRef};

    fn part(name: &str, table_type: &str, size: Option<u64>) -> DiskPartition {
        DiskPartition {
            name: name.into(),
            role: PartitionRole::parse(name),
            table_type: table_type.into(),
            bootable: false,
            offset: None,
            size: size.map(Size),
            content: None,
        }
    }

    fn content(digest: &str) -> Option<StoreRef> {
        Some(StoreRef {
            kind: ObjectKind::File,
            digest: format!("sha256:{digest}"),
        })
    }

    #[test]
    fn a_recipe_partition_fills_the_boards_of_its_name() {
        let board = [
            part("boot", "0x0c", Some(82_837_504)),
            part("data", "0x83", None),
        ];
        let mut boot = part("boot", "", None);
        boot.content = content("aa");
        let mut data = part("data", "", None);
        data.content = content("bb");
        let all = fill(&board, &[boot, data]).expect("fills");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].table_type, "0x0c");
        assert_eq!(all[0].size, Some(Size(82_837_504)));
        assert_eq!(all[0].content, content("aa"));
        assert_eq!(all[1].content, content("bb"));
        // A partition the board does not name follows the board's.
        let all = fill(&board, &[part("extra", "0x83", None)]).expect("appends");
        assert_eq!(all[2].name, "extra");
    }

    #[test]
    fn what_a_recipe_states_otherwise_is_refused() {
        let board = [part("boot", "0x0c", Some(4096))];
        let why = fill(&board, &[part("boot", "0x83", None)]).expect_err("type");
        assert_eq!(
            why,
            "partition 'boot': the board's layout gives its type as 0x0c, and the recipe as 0x83; the board's layout is the one to change"
        );
        let why = fill(&board, &[part("boot", "", Some(8192))]).expect_err("size");
        assert!(
            why.contains("its size as 4096, and the recipe as 8192"),
            "{why}"
        );
        let mut active = part("boot", "", None);
        active.bootable = true;
        assert!(fill(&board, &[active]).is_err());
        // The same type and size, restated: no difference.
        assert!(fill(&board, &[part("boot", "0x0c", Some(4096))]).is_ok());
        let mut twice = part("boot", "", None);
        twice.content = content("aa");
        let why = fill(&board, &[twice.clone(), twice]).expect_err("twice");
        assert_eq!(why, "partition 'boot' is filled twice");
    }
}
