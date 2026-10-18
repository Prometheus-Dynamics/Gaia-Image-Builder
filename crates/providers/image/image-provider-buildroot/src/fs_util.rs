use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, Write};

/// How a collected file was placed in the collect dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectedPlacement {
    /// The copy already there has the source's size and mtime: nothing written.
    Unchanged,
    /// A reflink (FICLONE): the filesystem shares the source's extents.
    Reflinked,
    /// A byte copy in which zero runs become holes, so it stays sparse.
    Sparse,
}

impl CollectedPlacement {
    /// The word a step message gives the placement.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Unchanged => "kept: size and mtime unchanged",
            Self::Reflinked => "reflinked",
            Self::Sparse => "sparse copy",
        }
    }
}

pub(crate) struct CollectedCopy {
    /// The sha256 of the file's bytes (the source's, which the copy equals).
    pub(crate) digest: String,
    pub(crate) placement: CollectedPlacement,
}

/// Places `from` at `to` in `collect_dir`. A copy whose size and mtime match
/// the source is kept (its digest comes from the content manifest). Otherwise
/// the file is reflinked where the filesystem supports it, or else copied
/// sparsely, and its mtime is set to the source's so the next build can keep
/// it. Hard links are not used: later image steps rewrite the Buildroot
/// images in place, which would change the collected copy too.
pub(crate) fn copy_collected_file(
    from: &Path,
    to: &Path,
    collect_dir: &Path,
) -> std::io::Result<CollectedCopy> {
    let source = fs::metadata(from)?;
    if let Ok(current) = fs::metadata(to)
        && current.is_file()
        && current.len() == source.len()
        && current.modified().ok() == source.modified().ok()
    {
        let digest = match gaia_image_providers::recorded_sha256(collect_dir, to) {
            Some(digest) => digest,
            None => gaia_image_providers::sha256_hex(to)?,
        };
        return Ok(CollectedCopy {
            digest,
            placement: CollectedPlacement::Unchanged,
        });
    }
    // A fresh inode: an earlier copy may be shared with something else.
    match fs::remove_file(to) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut reader = fs::File::open(from)?;
    let mut writer = fs::File::create_new(to)?;
    let placement = if reflink_file(&reader, &writer) {
        CollectedPlacement::Reflinked
    } else {
        CollectedPlacement::Sparse
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        let chunk = &buffer[..read];
        hasher.update(chunk);
        if placement == CollectedPlacement::Sparse && chunk.iter().all(|byte| *byte == 0) {
            writer.seek(std::io::SeekFrom::Current(read as i64))?;
        } else if placement == CollectedPlacement::Sparse {
            writer.write_all(chunk)?;
        }
    }
    if placement == CollectedPlacement::Sparse {
        // A trailing hole does not extend the file: set its length.
        writer.set_len(total)?;
    }
    writer.set_permissions(source.permissions())?;
    if let Ok(modified) = source.modified() {
        writer.set_modified(modified)?;
    }
    Ok(CollectedCopy {
        digest: hex(&hasher.finalize()),
        placement,
    })
}

/// Asks the filesystem to share `source`'s extents with `dest` (FICLONE).
/// `false` when it cannot (other filesystems, another device): the caller
/// then copies the bytes.
#[cfg(target_os = "linux")]
fn reflink_file(source: &fs::File, dest: &fs::File) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: both descriptors are open for the duration of the call, and
    // FICLONE takes the source descriptor as its only argument.
    unsafe { libc::ioctl(dest.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn reflink_file(_source: &fs::File, _dest: &fs::File) -> bool {
    false
}

/// Records the digest of a file just copied into a collect dir, so the
/// content manifest does not hash it again. Best effort: a missing entry
/// only costs a later hash.
pub(crate) fn record_copied_digest(collect_dir: &Path, copied: &Path, digest: &str) {
    let _ = gaia_image_providers::record_content_digests_known(
        collect_dir,
        vec![(copied.to_path_buf(), Some(digest.to_string()))],
    );
}

pub(crate) fn rootfs_path(rootfs_dir: &Path, image_path: &str) -> PathBuf {
    let trimmed = image_path.trim_start_matches('/');
    rootfs_dir.join(trimmed)
}

pub(crate) fn resolve_workspace_path(
    spec: &ResolvedBuildSpec,
    raw: &str,
) -> Result<PathBuf, ImageProviderError> {
    gaia_spec::resolve_workspace_path(&spec.workspace, raw).map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::RuntimeState, error.to_string())
    })
}

pub(crate) fn copy_path(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to create destination dir '{}': {error}",
                    parent.display()
                ),
            )
        })?;
    }
    let metadata = fs::symlink_metadata(src).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to stat source '{}': {error}", src.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read symlink '{}': {error}", src.display()),
            )
        })?;
        if let Ok(dest_meta) = fs::symlink_metadata(dest) {
            if dest_meta.is_dir() {
                fs::remove_dir_all(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination dir '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            } else {
                fs::remove_file(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination file '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            }
        } else if dest.exists() {
            let dest_meta = fs::symlink_metadata(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to stat existing destination '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
            if dest_meta.is_dir() {
                fs::remove_dir_all(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination dir '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            } else {
                fs::remove_file(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination file '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy symlink '{}' to '{}': {error}",
                    src.display(),
                    dest.display()
                ),
            )
        })?;
        #[cfg(not(unix))]
        {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy symlink '{}' to '{}': symlink copy is unsupported on this platform",
                    src.display(),
                    dest.display()
                ),
            ));
        }
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to create directory '{}': {error}", dest.display()),
            )
        })?;
        for entry in fs::read_dir(src).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read directory '{}': {error}", src.display()),
            )
        })? {
            let entry = entry.map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read directory entry in '{}': {error}",
                        src.display()
                    ),
                )
            })?;
            copy_path(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Ok(dest_meta) = fs::symlink_metadata(dest) {
        if dest_meta.is_dir() {
            fs::remove_dir_all(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to remove existing destination dir '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        } else {
            fs::remove_file(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to remove existing destination file '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }
    fs::copy(src, dest).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to copy source '{}' to '{}': {error}",
                src.display(),
                dest.display()
            ),
        )
    })?;
    Ok(())
}

pub(crate) fn merge_tree_contents(
    src_dir: &Path,
    dest_dir: &Path,
) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create destination dir '{}': {error}",
                dest_dir.display()
            ),
        )
    })?;

    let entries = fs::read_dir(src_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to read source dir '{}': {error}", src_dir.display()),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read entry under '{}': {error}",
                    src_dir.display()
                ),
            )
        })?;
        copy_path(&entry.path(), &dest_dir.join(entry.file_name()))?;
    }

    Ok(())
}

#[cfg(test)]
mod collected_copy_tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::os::unix::fs::MetadataExt;

    /// A sparse file: 64 MiB long, with data only at both ends.
    fn sparse_image(path: &Path, bytes: u64) {
        let mut file = fs::File::create(path).expect("image");
        file.set_len(bytes).expect("length");
        file.write_all(b"head").expect("head");
        file.seek(std::io::SeekFrom::Start(bytes - 4))
            .expect("seek");
        file.write_all(b"tail").expect("tail");
    }

    #[test]
    fn a_collected_copy_keeps_its_holes_its_bytes_and_its_digest() {
        let scratch = TempDir::new("gaia-collected-copy");
        let source = scratch.join("sdcard.img");
        let collect = scratch.join("collect");
        fs::create_dir_all(&collect).expect("collect dir");
        let bytes = 64 * 1024 * 1024;
        sparse_image(&source, bytes);
        let dest = collect.join("sdcard.img");

        let copied = copy_collected_file(&source, &dest, &collect).expect("copy");

        assert_ne!(copied.placement, CollectedPlacement::Unchanged);
        assert_eq!(
            fs::read(&source).expect("source"),
            fs::read(&dest).expect("copy")
        );
        assert_eq!(
            copied.digest,
            gaia_image_providers::sha256_hex(&dest).expect("digest")
        );
        let metadata = fs::metadata(&dest).expect("dest metadata");
        assert_eq!(metadata.len(), bytes);
        assert!(
            metadata.blocks() * 512 < bytes / 4,
            "the copy allocated {} bytes of a {bytes}-byte sparse file",
            metadata.blocks() * 512
        );
    }

    #[test]
    fn an_unchanged_collected_copy_is_not_written_again() {
        let scratch = TempDir::new("gaia-collected-copy-unchanged");
        let source = scratch.join("rootfs.erofs");
        let collect = scratch.join("collect");
        fs::create_dir_all(&collect).expect("collect dir");
        fs::write(&source, "first image").expect("source");
        let dest = collect.join("rootfs.erofs");
        let first = copy_collected_file(&source, &dest, &collect).expect("first copy");
        gaia_image_providers::record_content_digests_known(
            &collect,
            vec![(dest.clone(), Some(first.digest.clone()))],
        )
        .expect("record");

        // Same size and mtime as the source, different bytes: a skipped copy keeps them.
        let modified = fs::metadata(&source)
            .expect("metadata")
            .modified()
            .expect("mtime");
        fs::write(&dest, "FIRST IMAGE").expect("same-size edit");
        fs::File::options()
            .write(true)
            .open(&dest)
            .and_then(|file| file.set_modified(modified))
            .expect("mtime");
        let again = copy_collected_file(&source, &dest, &collect).expect("second copy");

        assert_eq!(again.placement, CollectedPlacement::Unchanged);
        assert_eq!(fs::read_to_string(&dest).expect("kept"), "FIRST IMAGE");
        assert_eq!(
            again.digest, first.digest,
            "the recorded digest of the kept copy"
        );
    }

    #[test]
    fn a_changed_source_is_copied_again() {
        let scratch = TempDir::new("gaia-collected-copy-changed");
        let source = scratch.join("Image");
        let collect = scratch.join("collect");
        fs::create_dir_all(&collect).expect("collect dir");
        fs::write(&source, "kernel v1").expect("source");
        let dest = collect.join("Image");
        copy_collected_file(&source, &dest, &collect).expect("first copy");

        fs::write(&source, "kernel v2, longer").expect("rebuilt source");
        let copied = copy_collected_file(&source, &dest, &collect).expect("second copy");

        assert_ne!(copied.placement, CollectedPlacement::Unchanged);
        assert_eq!(
            fs::read_to_string(&dest).expect("copy"),
            "kernel v2, longer"
        );
        assert_eq!(
            copied.digest,
            gaia_image_providers::sha256_hex(&dest).expect("digest")
        );
    }
}
