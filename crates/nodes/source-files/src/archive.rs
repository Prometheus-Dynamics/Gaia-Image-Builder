//! `gaia.source.archive`: a stored tar archive unpacked to a tree.

use crate::files::normalize_modes;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, Tree};
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use tar::EntryType;

const NAME: &str = "gaia.source.archive";

/// Unpacks `archive`, dropping `strip` leading path components of each
/// member; a member with no more than that is left out.
#[node(
    id = "gaia.source.archive",
    inputs(store, archive, strip),
    outputs(tree)
)]
pub fn source_archive(
    store: &str,
    archive: &Artifact,
    strip: i64,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let object = archive
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(NAME, format!("bad object {}", archive.object.digest)))?;
    let strip = usize::try_from(strip)
        .map_err(|_| StepError::body(NAME, format!("strip {strip} is negative")))?;
    let file = store.path(&object);
    let key = KeyBuilder::new(NAME, 1)
        .config("strip", strip.to_string())
        .input("archive", object.digest)
        .libs(crate::LIBS)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let tree = work.dir().join("tree");
            fs::create_dir(&tree).map_err(|e| StepError::body(NAME, e))?;
            unpack(&file, &tree, strip, || work.cancelled())
                .map_err(|e| StepError::body(NAME, format!("{}: {e}", object.digest)))?;
            normalize_modes(&tree).map_err(|e| StepError::body(NAME, e))
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}

/// The archive's bytes, decompressed by what its first bytes say it is.
fn open(path: &Path) -> io::Result<Box<dyn Read>> {
    let mut magic = [0_u8; 6];
    let read = fs::File::open(path)?.read(&mut magic)?;
    let file = BufReader::new(fs::File::open(path)?);
    Ok(match &magic[..read] {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::MultiGzDecoder::new(file)),
        [0xfd, b'7', b'z', b'X', b'Z', 0x00] => {
            Box::new(xz2::read::XzDecoder::new_multi_decoder(file))
        }
        [0x28, 0xb5, 0x2f, 0xfd, ..] => Box::new(zstd::stream::read::Decoder::new(file)?),
        [b'B', b'Z', b'h', ..] => {
            return Err(io::Error::other(
                "a bzip2 archive; gzip, xz, zstd or plain tar is read",
            ));
        }
        _ => Box::new(file),
    })
}

fn unpack(
    archive: &Path,
    dir: &Path,
    strip: usize,
    cancelled: impl Fn() -> bool,
) -> io::Result<()> {
    let mut tar = tar::Archive::new(open(archive)?);
    let mut count = 0_usize;
    for entry in tar.entries()? {
        if cancelled() {
            return Err(io::Error::other("cancelled"));
        }
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
        let Some(relative) = stripped(&name, strip)? else {
            continue;
        };
        let target = dir.join(&relative);
        make_parents(dir, &relative)?;
        match kind {
            EntryType::Directory => {
                if !target.is_dir() {
                    replace(&target)?;
                    fs::create_dir(&target)?;
                }
            }
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                replace(&target)?;
                let mut out = fs::File::create(&target)?;
                io::copy(&mut entry, &mut out)?;
                use std::os::unix::fs::PermissionsExt;
                let mode = entry.header().mode()?;
                fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))?;
            }
            EntryType::Symlink => {
                let link = entry.link_name()?.ok_or_else(|| {
                    io::Error::other(format!("{}: a symlink with no target", name.display()))
                })?;
                replace(&target)?;
                std::os::unix::fs::symlink(link.as_ref(), &target)?;
            }
            EntryType::Link => {
                let link = entry.link_name()?.ok_or_else(|| {
                    io::Error::other(format!("{}: a hard link with no target", name.display()))
                })?;
                let Some(source) = stripped(&link, strip)? else {
                    return Err(io::Error::other(format!(
                        "{}: links to a stripped path",
                        name.display()
                    )));
                };
                check_no_symlink(dir, &source)?;
                replace(&target)?;
                fs::hard_link(dir.join(source), &target)?;
            }
            other => {
                return Err(io::Error::other(format!(
                    "{}: a {other:?} member is not unpacked",
                    name.display()
                )));
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(io::Error::other("nothing to unpack"));
    }
    Ok(())
}

/// `name` without its first `strip` components; `None` when nothing is
/// left. An absolute path or a `..` is an error.
fn stripped(name: &Path, strip: usize) -> io::Result<Option<PathBuf>> {
    let mut parts = Vec::new();
    for part in name.components() {
        match part {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            _ => {
                return Err(io::Error::other(format!(
                    "{}: an absolute path or `..` is not unpacked",
                    name.display()
                )));
            }
        }
    }
    if parts.len() <= strip {
        return Ok(None);
    }
    Ok(Some(parts[strip..].iter().collect()))
}

/// Creates the directories above `relative` in `dir`; a symlink among them
/// is an error, so nothing is written through one.
fn make_parents(dir: &Path, relative: &Path) -> io::Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut at = dir.to_path_buf();
    for part in parent.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "{}: a path through a symlink is not unpacked",
                    relative.display()
                )));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "{}: a file is in the way",
                    relative.display()
                )));
            }
            Err(_) => fs::create_dir(&at)?,
        }
    }
    Ok(())
}

fn check_no_symlink(dir: &Path, relative: &Path) -> io::Result<()> {
    let mut at = dir.to_path_buf();
    for part in relative.components() {
        at.push(part);
        if fs::symlink_metadata(&at)?.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "{}: a link through a symlink",
                relative.display()
            )));
        }
    }
    Ok(())
}

/// Removes what is at `path`, unless it is a directory: a later member
/// replaces an earlier one, and a symlink is never written through.
fn replace(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Err(io::Error::other(format!(
            "{}: a directory is in the way",
            path.display()
        ))),
        Ok(_) => fs::remove_file(path),
        Err(_) => Ok(()),
    }
}
