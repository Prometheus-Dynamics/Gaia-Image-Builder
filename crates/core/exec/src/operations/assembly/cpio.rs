//! The newc cpio archive of the `cpio*` filesystems. An archive is planned
//! first (every record's name, mode, inode, link count, device numbers and
//! data size, from metadata only), then streamed into a writer. The plan
//! gives the exact archive size up front, which `zstd --stream-size` needs
//! to write the same frame a file input gives.
//!
//! Hard links: the links of one inode share a newc inode number and link
//! count, and the data goes with the last link in archive order, as cpio and
//! the kernel's initramfs unpacker expect. Device nodes and FIFOs come from
//! the tree (when the build can see them there) or from the `devices` list
//! of the assembly.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufWriter, PipeReader, Write};
use std::path::{Path, PathBuf};

use std::process::Command;

use gaia_process::ProcessCancelCheck;
use gaia_spec::{AssemblyDeviceKindSpec, AssemblyDeviceSpec, ResolvedBuildSpec};

use super::filesystems::collect_archive_entries;
use super::{AssemblyError, CommandFileOutput, assembly_command_timeout, process_output_retention};

/// The newc header: a 6-byte magic and 13 eight-digit hex fields.
const NEWC_HEADER_BYTES: usize = 110;
const TRAILER_NAME: &str = "TRAILER!!!";
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;
const S_IFCHR: u32 = 0o020000;
const S_IFBLK: u32 = 0o060000;
const S_IFIFO: u32 = 0o010000;

/// What a record's data is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// No data: directories, device nodes, FIFOs, and every hard link but
    /// the last of its inode.
    Empty,
    /// A symlink's target.
    Link(String),
    /// A regular file's contents, `size` bytes as planned.
    File { path: PathBuf, size: u64 },
}

/// One newc record, ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    name: String,
    /// File type and permission bits.
    mode: u32,
    ino: u32,
    nlink: u32,
    /// Device numbers of a char or block node; zero otherwise.
    rdev: (u32, u32),
    body: Body,
}

/// Where a planned name's record comes from before its metadata is read.
enum Source {
    Path(PathBuf),
    /// A directory the device list needs and the tree lacks.
    ImplicitDir,
    Device(AssemblyDeviceSpec),
}

/// A planned archive: its records in order, and the exact size it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CpioPlan {
    records: Vec<Record>,
    bytes: u64,
}

impl CpioPlan {
    /// The archive's size in bytes, trailer included.
    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Plans the archive of `source_tree` with the devices `devices` (all for
/// this tree) added.
pub(super) fn plan_newc(
    source_tree: &Path,
    devices: &[&AssemblyDeviceSpec],
) -> Result<CpioPlan, String> {
    let mut sources: BTreeMap<String, Source> = BTreeMap::new();
    for (name, path) in collect_archive_entries(source_tree)? {
        sources.insert(name, Source::Path(path));
    }
    for device in devices {
        let name = device.path.trim_start_matches('/').to_string();
        if sources.contains_key(&name) {
            return Err(format!(
                "assembly device '{}' is already in tree '{}'",
                device.path,
                source_tree.display()
            ));
        }
        // The device's parent directories are added when the tree lacks
        // them: an unpacker creates a node only where its parent exists.
        let parts: Vec<&str> = name.split('/').collect();
        let mut parent = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(part);
            sources.entry(parent.clone()).or_insert(Source::ImplicitDir);
        }
        sources.insert(name, Source::Device((*device).clone()));
    }

    let mut records = Vec::with_capacity(sources.len());
    // Inode of each hard-linked file, with the indices of its records.
    let mut links: HashMap<(u64, u64), (u32, Vec<usize>)> = HashMap::new();
    let mut next_ino = 1u32;
    for (name, source) in sources {
        let (mode, body, rdev, link_key) = match source {
            Source::ImplicitDir => (S_IFDIR | 0o755, Body::Empty, (0, 0), None),
            Source::Device(device) => {
                let type_bits = match device.kind {
                    AssemblyDeviceKindSpec::Char => S_IFCHR,
                    AssemblyDeviceKindSpec::Block => S_IFBLK,
                    AssemblyDeviceKindSpec::Fifo => S_IFIFO,
                };
                let rdev = match device.kind {
                    AssemblyDeviceKindSpec::Fifo => (0, 0),
                    _ => (device.major.unwrap_or(0), device.minor.unwrap_or(0)),
                };
                (
                    type_bits | device.permission_bits(),
                    Body::Empty,
                    rdev,
                    None,
                )
            }
            Source::Path(path) => plan_path(&path, source_tree)?,
        };
        let index = records.len();
        let ino = match link_key {
            Some(key) => match links.get_mut(&key) {
                Some((ino, members)) => {
                    members.push(index);
                    *ino
                }
                None => {
                    let ino = next_ino;
                    next_ino = next_ino.saturating_add(1);
                    links.insert(key, (ino, vec![index]));
                    ino
                }
            },
            None => {
                let ino = next_ino;
                next_ino = next_ino.saturating_add(1);
                ino
            }
        };
        records.push(Record {
            name,
            mode,
            ino,
            nlink: 1,
            rdev,
            body,
        });
    }
    // A linked inode's records all carry the number of links the archive
    // holds, and only the last one has the data.
    for (_, members) in links.values() {
        let count = u32::try_from(members.len()).unwrap_or(u32::MAX);
        let (last, earlier) = members.split_last().expect("a link group has a member");
        for index in earlier {
            records[*index].nlink = count;
            records[*index].body = Body::Empty;
        }
        records[*last].nlink = count;
    }

    let mut bytes = record_bytes(TRAILER_NAME, 0);
    for record in &records {
        bytes += record_bytes(&record.name, body_len(&record.body));
    }
    records.push(Record {
        name: TRAILER_NAME.to_string(),
        mode: 0o100644,
        ino: 0,
        nlink: 1,
        rdev: (0, 0),
        body: Body::Empty,
    });
    Ok(CpioPlan { records, bytes })
}

/// A tree entry's mode, body, device numbers and hard-link key (device and
/// inode, for a file with several links).
type PlannedPath = (u32, Body, (u32, u32), Option<(u64, u64)>);

/// The mode, body, device numbers and hard-link key of a tree entry, from
/// its metadata.
fn plan_path(path: &Path, source_tree: &Path) -> Result<PlannedPath, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect assembly archive entry '{}': {error}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    let (permissions, device, inode, links) = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        (
            metadata.permissions().mode() & 0o7777,
            metadata.rdev(),
            metadata.ino(),
            metadata.nlink(),
        )
    };
    #[cfg(not(unix))]
    let (permissions, device, inode, links) = (0o644, 0, 0, 1);
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        return Ok((S_IFDIR | permissions.max(0o755), Body::Empty, (0, 0), None));
    }
    if file_type.is_symlink() {
        let target = std::fs::read_link(path).map_err(|error| {
            format!(
                "failed to read assembly archive symlink '{}': {error}",
                path.display()
            )
        })?;
        return Ok((
            S_IFLNK | 0o777,
            Body::Link(target.to_string_lossy().into_owned()),
            (0, 0),
            None,
        ));
    }
    if file_type.is_file() {
        let size = metadata.len();
        if u32::try_from(size).is_err() {
            return Err(format!(
                "assembly archive file '{}' exceeds the cpio newc size limit",
                path.display()
            ));
        }
        let key = (links > 1).then_some((device, inode));
        return Ok((
            S_IFREG | permissions.max(0o644),
            Body::File {
                path: path.to_path_buf(),
                size,
            },
            (0, 0),
            key,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_char_device() {
            return Ok((S_IFCHR | permissions, Body::Empty, glibc_dev(device), None));
        }
        if file_type.is_block_device() {
            return Ok((S_IFBLK | permissions, Body::Empty, glibc_dev(device), None));
        }
        if file_type.is_fifo() {
            return Ok((S_IFIFO | permissions, Body::Empty, (0, 0), None));
        }
    }
    Err(format!(
        "assembly archive entry '{}' under '{}' is not supported",
        path.display(),
        source_tree.display()
    ))
}

/// The major and minor numbers of a device, as glibc splits them.
fn glibc_dev(device: u64) -> (u32, u32) {
    let major = ((device >> 8) & 0xfff) | ((device >> 32) & !0xfff);
    let minor = (device & 0xff) | ((device >> 12) & !0xff);
    (major as u32, minor as u32)
}

fn body_len(body: &Body) -> u64 {
    match body {
        Body::Empty => 0,
        Body::Link(target) => target.len() as u64,
        Body::File { size, .. } => *size,
    }
}

/// The bytes one record takes: its header and name, then its data, each
/// padded to four bytes.
fn record_bytes(name: &str, data: u64) -> u64 {
    let header = (NEWC_HEADER_BYTES + name.len() + 1) as u64;
    pad4(header) + pad4(data)
}

fn pad4(length: u64) -> u64 {
    length.div_ceil(4) * 4
}

/// Streams a planned archive into `out`.
pub(super) fn write_newc<W: Write>(plan: &CpioPlan, out: &mut W) -> Result<(), String> {
    for record in &plan.records {
        write_record(record, out)?;
    }
    Ok(())
}

fn write_record<W: Write>(record: &Record, out: &mut W) -> Result<(), String> {
    let (data_size, rdev) = (body_len(&record.body), record.rdev);
    let data_size = u32::try_from(data_size).map_err(|_| {
        format!(
            "assembly archive entry '{}' exceeds the cpio newc size limit",
            record.name
        )
    })?;
    let name_size = record.name.len() + 1;
    write!(
        out,
        "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        record.ino,
        record.mode,
        0u32, // uid
        0u32, // gid
        record.nlink,
        0u32, // mtime
        data_size,
        0u32, // dev major
        0u32, // dev minor
        rdev.0,
        rdev.1,
        name_size,
        0u32, // check
    )
    .map_err(|error| format!("failed to write cpio header: {error}"))?;
    out.write_all(record.name.as_bytes())
        .and_then(|_| out.write_all(&[0]))
        .map_err(|error| format!("failed to write cpio name: {error}"))?;
    pad_to_4(out, NEWC_HEADER_BYTES + name_size)?;
    match &record.body {
        Body::Empty => {}
        Body::Link(target) => {
            out.write_all(target.as_bytes())
                .map_err(|error| format!("failed to write cpio data: {error}"))?;
            pad_to_4(out, target.len())?;
        }
        Body::File { path, size } => {
            let mut source = std::fs::File::open(path).map_err(|error| {
                format!(
                    "failed to open assembly archive file '{}': {error}",
                    path.display()
                )
            })?;
            let copied = std::io::copy(&mut source, out).map_err(|error| {
                format!(
                    "failed to stream assembly archive file '{}': {error}",
                    path.display()
                )
            })?;
            if copied != *size {
                return Err(format!(
                    "assembly archive file '{}' changed while it was being archived",
                    path.display()
                ));
            }
            pad_to_4(out, *size as usize)?;
        }
    }
    Ok(())
}

fn pad_to_4<W: Write>(out: &mut W, written: usize) -> Result<(), String> {
    const PADDING: [u8; 3] = [0; 3];
    let padding = (4 - (written % 4)) % 4;
    if padding > 0 {
        out.write_all(&PADDING[..padding])
            .map_err(|error| format!("failed to write cpio padding: {error}"))?;
    }
    Ok(())
}

/// Runs `command`, which reads the archive on its standard input, with its
/// stdout published as `temp`. The archive is streamed to the command as it
/// is planned, so no cpio copy is written.
pub(super) fn run_compressor_from_plan(
    spec: &ResolvedBuildSpec,
    command: &mut Command,
    plan: &CpioPlan,
    temp: &Path,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<CommandFileOutput, AssemblyError> {
    pipe_newc(plan, |stdin| {
        let result = gaia_process::run_command_stdout_to_file_with_stdin(
            command,
            Some(stdin),
            temp,
            assembly_command_timeout(spec),
            "assembly command",
            process_output_retention(spec),
            None,
            cancel_check,
        )
        .map_err(|error| AssemblyError::process(command, error))?;
        Ok(CommandFileOutput {
            status: result.output.status,
            stderr: result.output.stderr,
        })
    })
}

/// Writes a planned archive to `path`, buffered.
pub(super) fn write_newc_file(plan: &CpioPlan, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|error| {
        format!(
            "failed to create cpio archive '{}': {error}",
            path.display()
        )
    })?;
    let mut out = BufWriter::new(file);
    write_newc(plan, &mut out)?;
    out.flush()
        .map_err(|error| format!("failed to write cpio archive '{}': {error}", path.display()))
}

/// Runs `consume` with the read end of a pipe that carries the archive: a
/// thread writes the plan into the other end while `consume` reads it (as a
/// command's standard input). The consumer's error is reported first: a
/// writer that fails because the consumer exited early says little.
pub(super) fn pipe_newc<T>(
    plan: &CpioPlan,
    consume: impl FnOnce(PipeReader) -> Result<T, AssemblyError>,
) -> Result<T, AssemblyError> {
    let (reader, writer) =
        std::io::pipe().map_err(|error| format!("failed to create the cpio pipe: {error}"))?;
    std::thread::scope(|scope| {
        let writing = scope.spawn(move || -> Result<(), String> {
            let mut out = BufWriter::new(writer);
            write_newc(plan, &mut out)?;
            out.flush()
                .map_err(|error| format!("failed to write cpio pipe: {error}"))
        });
        let consumed = consume(reader);
        let written = writing
            .join()
            .unwrap_or_else(|_| Err("the cpio writer thread panicked".to_string()));
        match (consumed, written) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.into()),
            (Err(error), _) => Err(error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glibc_dev_splits_major_and_minor() {
        // makedev(8, 1) is 0x801 (major 8, minor 1).
        assert_eq!(glibc_dev(0x801), (8, 1));
        // glibc's makedev for a major and minor past their low bytes.
        let (major, minor) = (259u64, 300u64);
        let dev = ((major & !0xfff) << 32)
            | ((major & 0xfff) << 8)
            | ((minor & !0xff) << 12)
            | (minor & 0xff);
        assert_eq!(glibc_dev(dev), (259, 300));
    }

    #[test]
    fn record_bytes_pads_header_and_data_to_four() {
        // 110 + "a" + NUL = 112 header bytes; no data.
        assert_eq!(record_bytes("a", 0), 112);
        // "abc" + NUL: 114 header bytes padded to 116; 5 data bytes to 8.
        assert_eq!(record_bytes("abc", 5), 116 + 8);
    }

    /// The archive the writer produced before devices, hard links and the
    /// plan: sequential inodes, one link each, data for files and link
    /// targets. A tree without hard links or devices must still match it.
    fn reference_newc(tree: &Path) -> Vec<u8> {
        let mut entries = collect_archive_entries(tree).expect("walk");
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut out = Vec::new();
        let record = |out: &mut Vec<u8>, name: &str, ino: u32, mode: u32, data: &[u8]| {
            out.extend_from_slice(
                format!(
                    "070701{ino:08x}{mode:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                    0, 0, 1, 0, data.len(), 0, 0, 0, 0, name.len() + 1, 0
                )
                .as_bytes(),
            );
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            pad_to_4(out, 110 + name.len() + 1).expect("pad");
            out.extend_from_slice(data);
            pad_to_4(out, data.len()).expect("pad");
        };
        for (index, (name, path)) in entries.into_iter().enumerate() {
            let ino = u32::try_from(index).expect("entry count") + 1;
            let meta = std::fs::symlink_metadata(&path).expect("meta");
            use std::os::unix::fs::PermissionsExt;
            let perm = meta.permissions().mode() & 0o7777;
            if meta.file_type().is_dir() {
                record(&mut out, &name, ino, 0o040000 | perm.max(0o755), &[]);
            } else if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).expect("link");
                record(
                    &mut out,
                    &name,
                    ino,
                    0o120000 | 0o777,
                    target.to_string_lossy().as_bytes(),
                );
            } else {
                let data = std::fs::read(&path).expect("read");
                record(&mut out, &name, ino, 0o100000 | perm.max(0o644), &data);
            }
        }
        record(&mut out, TRAILER_NAME, 0, 0o100644, &[]);
        out
    }

    /// One record of an archive: its inode, mode, link count, device
    /// numbers, name and data.
    #[derive(Debug, PartialEq, Eq)]
    struct Parsed {
        name: String,
        ino: u32,
        mode: u32,
        nlink: u32,
        rdev: (u32, u32),
        data: Vec<u8>,
    }

    fn parse_newc(bytes: &[u8]) -> Vec<Parsed> {
        let field = |at: usize| -> u32 {
            u32::from_str_radix(std::str::from_utf8(&bytes[at..at + 8]).expect("hex"), 16)
                .expect("hex number")
        };
        let mut records = Vec::new();
        let mut at = 0;
        loop {
            assert_eq!(&bytes[at..at + 6], b"070701", "magic at {at}");
            let ino = field(at + 6);
            let mode = field(at + 14);
            let nlink = field(at + 38);
            let filesize = field(at + 54) as usize;
            let rdev = (field(at + 78), field(at + 86));
            let namesize = field(at + 94) as usize;
            let name_at = at + 110;
            let name = String::from_utf8(bytes[name_at..name_at + namesize - 1].to_vec())
                .expect("utf8 name");
            let data_at = (name_at + namesize).div_ceil(4) * 4;
            let data = bytes[data_at..data_at + filesize].to_vec();
            at = (data_at + filesize).div_ceil(4) * 4;
            let done = name == TRAILER_NAME;
            records.push(Parsed {
                name,
                ino,
                mode,
                nlink,
                rdev,
                data,
            });
            if done {
                assert_eq!(at, bytes.len(), "no bytes after the trailer");
                return records;
            }
        }
    }

    fn sample_tree(root: &Path) {
        std::fs::create_dir_all(root.join("bin")).expect("dir");
        std::fs::create_dir_all(root.join("etc/init.d")).expect("dir");
        std::fs::write(root.join("bin/tool"), b"#!/bin/sh\necho hi\n").expect("file");
        std::fs::write(root.join("etc/init.d/rc"), b"rc").expect("file");
        std::fs::write(root.join("etc/odd name"), b"x").expect("file");
        std::os::unix::fs::symlink("../bin/tool", root.join("etc/tool")).expect("link");
    }

    #[test]
    fn tree_without_links_or_devices_keeps_the_reference_bytes() {
        let dir = gaia_testkit::TempDir::new("cpio-reference");
        let tree = dir.path().join("tree");
        sample_tree(&tree);
        let plan = plan_newc(&tree, &[]).expect("plan");
        let mut bytes = Vec::new();
        write_newc(&plan, &mut bytes).expect("write");
        assert_eq!(bytes, reference_newc(&tree));
        assert_eq!(
            plan.bytes(),
            bytes.len() as u64,
            "the plan sizes the archive"
        );
    }

    #[test]
    fn hard_links_share_an_inode_and_the_last_link_carries_the_data() {
        let dir = gaia_testkit::TempDir::new("cpio-hardlink");
        let tree = dir.path().join("tree");
        sample_tree(&tree);
        std::fs::hard_link(tree.join("bin/tool"), tree.join("bin/sh")).expect("hard link");
        std::fs::hard_link(tree.join("bin/tool"), tree.join("bin/ls")).expect("hard link");
        let plan = plan_newc(&tree, &[]).expect("plan");
        let mut bytes = Vec::new();
        write_newc(&plan, &mut bytes).expect("write");
        let records = parse_newc(&bytes);
        let links: Vec<&Parsed> = records
            .iter()
            .filter(|record| record.name.starts_with("bin/") && record.name != "bin")
            .collect();
        assert_eq!(
            links.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["bin/ls", "bin/sh", "bin/tool"]
        );
        let inode = links[0].ino;
        assert!(
            links
                .iter()
                .all(|record| record.ino == inode && record.nlink == 3)
        );
        assert!(links[0].data.is_empty() && links[1].data.is_empty());
        assert_eq!(links[2].data, b"#!/bin/sh\necho hi\n");
        // Every other record has an inode of its own.
        let others: Vec<u32> = records
            .iter()
            .filter(|record| !record.name.starts_with("bin/") && record.name != TRAILER_NAME)
            .map(|record| record.ino)
            .collect();
        assert!(others.iter().all(|ino| *ino != inode), "{others:?}");
        let mut unique = others.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), others.len(), "{others:?}");
    }

    #[test]
    fn devices_from_the_list_are_added_with_their_parents() {
        let dir = gaia_testkit::TempDir::new("cpio-devices");
        let tree = dir.path().join("tree");
        sample_tree(&tree);
        let console = gaia_spec::AssemblyDeviceSpec {
            tree: "rootfs".into(),
            path: "/dev/console".into(),
            kind: AssemblyDeviceKindSpec::Char,
            major: Some(5),
            minor: Some(1),
            mode: Some("0600".into()),
        };
        let fifo = gaia_spec::AssemblyDeviceSpec {
            tree: "rootfs".into(),
            path: "run/fifo".into(),
            kind: AssemblyDeviceKindSpec::Fifo,
            major: None,
            minor: None,
            mode: None,
        };
        let plan = plan_newc(&tree, &[&console, &fifo]).expect("plan");
        let mut bytes = Vec::new();
        write_newc(&plan, &mut bytes).expect("write");
        let records = parse_newc(&bytes);
        let dev = records.iter().find(|r| r.name == "dev").expect("dev dir");
        assert_eq!(dev.mode, 0o040755);
        let console = records
            .iter()
            .find(|r| r.name == "dev/console")
            .expect("console");
        assert_eq!(console.mode, 0o020600);
        assert_eq!(console.rdev, (5, 1));
        let run = records.iter().find(|r| r.name == "run").expect("run dir");
        assert_eq!(run.mode, 0o040755);
        let fifo = records.iter().find(|r| r.name == "run/fifo").expect("fifo");
        assert_eq!(fifo.mode, 0o010600);
        assert_eq!(fifo.rdev, (0, 0));
    }

    #[test]
    fn a_device_over_a_tree_entry_is_refused() {
        let dir = gaia_testkit::TempDir::new("cpio-device-clash");
        let tree = dir.path().join("tree");
        sample_tree(&tree);
        let clash = gaia_spec::AssemblyDeviceSpec {
            tree: "rootfs".into(),
            path: "bin/tool".into(),
            kind: AssemblyDeviceKindSpec::Fifo,
            major: None,
            minor: None,
            mode: None,
        };
        let error = plan_newc(&tree, &[&clash]).expect_err("clash");
        assert!(error.contains("already in tree"), "{error}");
    }

    /// Compressing the archive through a pipe gives the bytes a file input
    /// gives, for gzip and for zstd with the planned `--stream-size`.
    #[test]
    fn piped_archive_compresses_to_the_file_input_bytes() {
        let dir = gaia_testkit::TempDir::new("cpio-pipe");
        let tree = dir.path().join("tree");
        sample_tree(&tree);
        std::fs::write(tree.join("etc/big"), vec![7u8; 300_000]).expect("big");
        let plan = plan_newc(&tree, &[]).expect("plan");
        let archive = dir.path().join("archive.cpio");
        write_newc_file(&plan, &archive).expect("file");
        assert_eq!(
            std::fs::metadata(&archive).expect("meta").len(),
            plan.bytes()
        );

        // A file input, as the cpio filesystems used to give it.
        let host = |tool: &str, args: &[String], input: &Path| -> Vec<u8> {
            let output = std::process::Command::new(tool)
                .args(args)
                .arg(input)
                .output()
                .expect("run");
            assert!(output.status.success(), "{tool} failed");
            output.stdout
        };
        let gzip_args = vec!["-n".to_string(), "-c".to_string()];
        let zstd_args = vec![
            "-q".to_string(),
            "-c".to_string(),
            "--no-progress".to_string(),
            "-T0".to_string(),
            "-19".to_string(),
        ];
        let mut zstd_piped = zstd_args.clone();
        zstd_piped.push(format!("--stream-size={}", plan.bytes()));

        let file_gzip = host("gzip", &gzip_args, &archive);
        let file_zstd = host("zstd", &zstd_args, &archive);
        for (tool, args, expected) in [
            ("gzip", &gzip_args, &file_gzip),
            ("zstd", &zstd_piped, &file_zstd),
        ] {
            let (reader, writer) = std::io::pipe().expect("pipe");
            let plan_ref = &plan;
            let bytes = std::thread::scope(|scope| {
                let writing = scope.spawn(move || {
                    let mut out = BufWriter::new(writer);
                    write_newc(plan_ref, &mut out).expect("write");
                    out.flush().expect("flush");
                });
                let mut command = std::process::Command::new(tool);
                command
                    .args(args)
                    .stdin(std::process::Stdio::from(reader))
                    .stdout(std::process::Stdio::piped());
                let output = command.output().expect("run");
                writing.join().expect("writer");
                assert!(output.status.success(), "{tool} failed");
                output.stdout
            });
            assert_eq!(&bytes, expected, "{tool} output through the pipe");
        }
    }
}
