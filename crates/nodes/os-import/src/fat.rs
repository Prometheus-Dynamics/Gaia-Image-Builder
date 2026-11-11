//! A FAT filesystem (a boot or EFI system partition), read in place.

use crate::disk::Slice;
use crate::out::{Sink, component, join};
use std::io::{self, Read};

/// Writes every file of the FAT filesystem in `slice` into `sink`: files
/// 0644, directories 0755, owned by root.
pub(crate) fn copy(slice: Slice, sink: &mut Sink<'_>) -> io::Result<()> {
    let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new())?;
    walk(&fs.root_dir(), "", sink)
}

fn walk<T: fatfs::ReadWriteSeek>(
    dir: &fatfs::Dir<'_, T>,
    rel: &str,
    sink: &mut Sink<'_>,
) -> io::Result<()> {
    let mut entries: Vec<_> = dir.iter().collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        if name == "."
            || name == ".."
            || entry
                .attributes()
                .contains(fatfs::FileAttributes::VOLUME_ID)
        {
            continue;
        }
        let rel = join(rel, component(&name)?);
        if entry.is_dir() {
            sink.dir(&rel, 0o755, 0, 0)?;
            walk(&entry.to_dir(), &rel, sink)?;
        } else {
            let mut data = Vec::new();
            entry.to_file().read_to_end(&mut data)?;
            sink.file(&rel, &data, 0o644, 0, 0)?;
        }
    }
    Ok(())
}
