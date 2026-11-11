//! A root filesystem archive: tar, plain or gzip, xz or zstd compressed.
//! Nothing is written outside the tree: an absolute member path, a `..`,
//! or a path through a symlink is refused.

use crate::out::{Sink, component};
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path};
use tar::EntryType;

/// The archive's bytes, decompressed by what its first bytes say, or
/// `None` when it is not a tar archive.
pub(crate) fn open(path: &Path) -> io::Result<Option<Box<dyn Read>>> {
    let mut magic = [0_u8; 6];
    let read = fs::File::open(path)?.read(&mut magic)?;
    let file = BufReader::new(fs::File::open(path)?);
    let mut stream: Box<dyn Read> = match &magic[..read] {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::MultiGzDecoder::new(file)),
        [0xfd, b'7', b'z', b'X', b'Z', 0x00] => {
            Box::new(xz2::read::XzDecoder::new_multi_decoder(file))
        }
        [0x28, 0xb5, 0x2f, 0xfd, ..] => Box::new(zstd::stream::read::Decoder::new(file)?),
        _ => Box::new(file),
    };
    // A tar header's magic is at 257.
    let mut head = vec![0_u8; 512];
    let mut got = 0;
    while got < 512 {
        let n = stream.read(&mut head[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    if got < 512 || &head[257..262] != b"ustar" {
        return Ok(None);
    }
    Ok(Some(Box::new(io::Cursor::new(head).chain(stream))))
}

/// Unpacks `stream` into `sink`, keeping modes and recording owners.
pub(crate) fn unpack(stream: Box<dyn Read>, sink: &mut Sink<'_>) -> io::Result<()> {
    let mut tar = tar::Archive::new(stream);
    let mut count = 0;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if matches!(
            kind,
            EntryType::XGlobalHeader
                | EntryType::XHeader
                | EntryType::GNULongName
                | EntryType::GNULongLink
        ) {
            continue;
        }
        let name = entry.path()?.into_owned();
        let Some(rel) = relative(&name)? else {
            continue;
        };
        let header = entry.header();
        let (mode, uid, gid) = (
            header.mode()? & 0o7777,
            header.uid()? as u32,
            header.gid()? as u32,
        );
        parents(sink.root, &rel)?;
        let target = sink.root.join(&rel);
        if kind != EntryType::Directory && fs::symlink_metadata(&target).is_ok() {
            return Err(io::Error::other(format!("/{rel}: in the archive twice")));
        }
        match kind {
            EntryType::Directory => sink.dir(&rel, mode, uid, gid)?,
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                let mut data = Vec::new();
                entry.read_to_end(&mut data)?;
                sink.file(&rel, &data, mode, uid, gid)?;
            }
            EntryType::Symlink => {
                let link = entry
                    .link_name()?
                    .ok_or_else(|| io::Error::other(format!("/{rel}: a symlink with no target")))?;
                sink.link(&rel, &link, uid, gid)?;
            }
            EntryType::Link => {
                let link = entry.link_name()?.ok_or_else(|| {
                    io::Error::other(format!("/{rel}: a hard link with no target"))
                })?;
                let source = relative(&link)?
                    .ok_or_else(|| io::Error::other(format!("/{rel}: a hard link to the root")))?;
                parents(sink.root, &source)?;
                let data = fs::read(sink.root.join(&source))
                    .map_err(|e| io::Error::other(format!("/{rel}: links to /{source}: {e}")))?;
                sink.file(&rel, &data, mode, uid, gid)?;
            }
            EntryType::Char => sink.special(&rel, "char", mode, uid, gid),
            EntryType::Block => sink.special(&rel, "block", mode, uid, gid),
            EntryType::Fifo => sink.special(&rel, "fifo", mode, uid, gid),
            other => return Err(io::Error::other(format!("/{rel}: a {other:?} member"))),
        }
        count += 1;
    }
    if count == 0 {
        return Err(io::Error::other("an empty archive"));
    }
    Ok(())
}

/// The member's path inside the tree; `None` for the root itself.
fn relative(name: &Path) -> io::Result<Option<String>> {
    let mut parts = Vec::new();
    for part in name.components() {
        match part {
            Component::Normal(p) => {
                parts.push(
                    component(p.to_str().ok_or_else(|| {
                        io::Error::other(format!("{}: not UTF-8", name.display()))
                    })?)?
                    .to_string(),
                )
            }
            Component::CurDir => {}
            _ => {
                return Err(io::Error::other(format!(
                    "{}: an absolute path or `..` is not unpacked",
                    name.display()
                )));
            }
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

/// Makes the directories above `rel` (0755); a symlink or a file among
/// them is an error, so nothing is written through one.
fn parents(root: &Path, rel: &str) -> io::Result<()> {
    let Some((parent, _)) = rel.rsplit_once('/') else {
        return Ok(());
    };
    let mut at = root.to_path_buf();
    for part in parent.split('/') {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "/{rel}: a path through a symlink or file is not unpacked"
                )));
            }
            Err(_) => {
                fs::create_dir(&at)?;
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&at, fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    Ok(())
}
