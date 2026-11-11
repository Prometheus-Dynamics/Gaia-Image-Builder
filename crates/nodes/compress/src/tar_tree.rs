//! `gaia.compress.tar`: a stored tree as a reproducible tar.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, FileEntry, Tree};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.compress.tar";

/// The tree as a tar, keyed on its digest and its entries.
#[node(id = "gaia.compress.tar", inputs(store, tree), outputs(archive))]
pub fn tar_tree(store: &str, tree: Tree, cx: &Ctx) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(NAME, "the tree is not in the store"))?;
    let root = store.path(&object);
    let mut key = KeyBuilder::new(NAME, 1).input("tree", object.digest);
    let mut entries: BTreeMap<&str, &FileEntry> = BTreeMap::new();
    for entry in &tree.entries {
        let path = entry.path.trim_start_matches('/').trim_end_matches('/');
        key = key.config(
            &format!("entry {path}"),
            format!("{:o} {} {}", entry.mode & 0o7777, entry.uid, entry.gid),
        );
        entries.insert(path, entry);
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("archive", Path::new("archive.tar"))],
        |work| {
            let out = work.dir().join("archive.tar");
            write_tar(&root, &entries, &out).map_err(|e| StepError::body(NAME, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::Archive,
        object: made.outputs["archive"].into(),
    })
}

/// Every path under `root`, relative and `/`-separated, sorted by bytes.
fn walk(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for item in std::fs::read_dir(&dir)? {
            let path = item?.path();
            let relative = path
                .strip_prefix(root)
                .expect("under root")
                .to_str()
                .ok_or_else(|| std::io::Error::other(format!("{}: not UTF-8", path.display())))?
                .to_string();
            if path.symlink_metadata()?.is_dir() {
                pending.push(path.clone());
            }
            found.push((relative, path));
        }
    }
    found.sort();
    Ok(found)
}

fn write_tar(root: &Path, entries: &BTreeMap<&str, &FileEntry>, out: &Path) -> Result<(), String> {
    let paths = walk(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for path in entries.keys() {
        if !path.is_empty()
            && paths
                .binary_search_by(|(p, _)| p.as_str().cmp(path))
                .is_err()
        {
            return Err(format!("an entry names {path}, which is not in the tree"));
        }
    }
    let file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut builder = tar::Builder::new(BufWriter::new(file));
    builder.follow_symlinks(false);
    for (relative, path) in &paths {
        let meta = path
            .symlink_metadata()
            .map_err(|e| format!("{relative}: {e}"))?;
        let entry = entries.get(relative.as_str());
        let mut header = tar::Header::new_gnu();
        header.set_mtime(0);
        header.set_uid(entry.map_or(0, |e| e.uid.into()));
        header.set_gid(entry.map_or(0, |e| e.gid.into()));
        header.set_mode(entry.map_or(meta.permissions().mode(), |e| e.mode) & 0o7777);
        let fail = |e: std::io::Error| format!("{relative}: {e}");
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(path).map_err(fail)?;
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder
                .append_link(&mut header, relative, &target)
                .map_err(fail)?;
        } else if meta.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            builder
                .append_data(&mut header, format!("{relative}/"), std::io::empty())
                .map_err(fail)?;
        } else if meta.is_file() {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            let data = File::open(path).map_err(fail)?;
            builder
                .append_data(&mut header, relative, data)
                .map_err(fail)?;
        } else {
            return Err(format!("{relative}: not a file, directory or symlink"));
        }
    }
    let mut writer = builder.into_inner().map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}
