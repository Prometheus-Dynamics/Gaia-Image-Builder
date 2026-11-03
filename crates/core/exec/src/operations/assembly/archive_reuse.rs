//! A raw disk archive that an `xz` transform already made: when a transform
//! compresses the archive's disk with the archive's own xz flags (no level),
//! its output is the archive byte for byte, so it is moved to the archive
//! name instead of compressing the disk a second time, and the run publishes
//! one file instead of two.

use super::*;

/// An `xz` transform with the default level: its source and output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompressedDisk {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
}

/// Whether a transform writes the bytes a `.img.xz` archive of its source
/// would hold.
pub(super) fn is_archive_xz(transform: &gaia_spec::AssemblyTransformSpec) -> bool {
    transform.kind == gaia_spec::AssemblyTransformKindSpec::Xz && transform.level.is_none()
}

/// Moves the transform output of the archive's disk to the archive name and
/// drops it from `published_outputs`. `None` when no transform matches, so
/// the archive is compressed as usual.
pub(super) fn reuse_compressed_disk(
    spec: &ResolvedBuildSpec,
    built_disks: &[(String, PathBuf)],
    compressed: &[CompressedDisk],
    published_outputs: &mut Vec<PathBuf>,
    messages: &mut Vec<String>,
) -> Result<Option<AssemblyArchiveSummary>, AssemblyError> {
    let Some(archive_path) = assembly_archive_path(spec) else {
        return Ok(None);
    };
    let is_xz = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(gaia_spec::raw_disk_archive)
        == Some(gaia_spec::RawDiskArchive::Xz);
    if !is_xz || built_disks.is_empty() {
        return Ok(None);
    }
    let source = archive_source_disk(spec, &archive_path, built_disks)?;
    let Some(found) = compressed
        .iter()
        .find(|entry| &entry.src == source && entry.dest != archive_path && entry.dest.is_file())
    else {
        return Ok(None);
    };
    if let Some(parent) = archive_path.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create archive dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temp = temporary_assembly_output_path(&archive_path);
    // A rename when both are on one filesystem; a RAM run's transform output
    // is copied out instead.
    if std_fs::rename(&found.dest, &temp).is_err() {
        std_fs::copy(&found.dest, &temp)
            .and_then(|_| std_fs::remove_file(&found.dest))
            .map_err(|error| {
                let _ = std_fs::remove_file(&temp);
                format!(
                    "failed to move '{}' to '{}': {error}",
                    found.dest.display(),
                    archive_path.display()
                )
            })?;
    }
    let sha256 = publish_assembly_output(&temp, &archive_path)?;
    published_outputs.retain(|path| path != &found.dest);
    messages.push(format!(
        "archive '{}' is the xz transform output '{}' (same disk and flags), moved instead of compressed again",
        archive_path.display(),
        found.dest.display()
    ));
    Ok(Some(AssemblyArchiveSummary {
        output: archive_path.clone(),
        source: source.clone(),
        bytes: file_len(&archive_path)?,
        sha256,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_spec;
    use super::*;
    use gaia_testkit::TempDir;
    use std::fs;

    #[test]
    fn an_xz_archive_of_a_compressed_disk_moves_the_transform_output() {
        let scratch = TempDir::new("gaia-exec-test");
        let root = scratch.join("gaia-archive-reuse");
        let mut spec = test_spec(&root);
        spec.image.output.archive_name = Some("acme.img.xz".into());
        let images = root.join("out/images");
        fs::create_dir_all(&images).expect("images dir");
        let disk = root.join("work/slots.img");
        fs::create_dir_all(disk.parent().expect("work")).expect("work dir");
        fs::write(&disk, "raw disk").expect("disk");
        let transform_out = images.join("slot-id.img.xz");
        fs::write(&transform_out, "xz bytes").expect("transform output");
        let other = images.join("other.img.xz");
        let mut published = vec![transform_out.clone(), other.clone()];
        let compressed = [CompressedDisk {
            src: disk.clone(),
            dest: transform_out.clone(),
        }];
        let built = [("slots".to_string(), disk.clone())];
        let mut messages = Vec::new();

        let summary =
            reuse_compressed_disk(&spec, &built, &compressed, &mut published, &mut messages)
                .expect("reuse")
                .expect("summary");

        assert_eq!(summary.output, images.join("acme.img.xz"));
        assert_eq!(
            fs::read(&summary.output).expect("archive"),
            b"xz bytes".to_vec()
        );
        assert!(!transform_out.exists());
        assert_eq!(published, vec![other]);

        // Another disk, or a zstd archive, is compressed as usual.
        let elsewhere = [("other".to_string(), root.join("work/other.img"))];
        assert!(
            reuse_compressed_disk(
                &spec,
                &elsewhere,
                &compressed,
                &mut published,
                &mut messages
            )
            .expect("no reuse")
            .is_none()
        );
        spec.image.output.archive_name = Some("acme.img.zst".into());
        assert!(
            reuse_compressed_disk(&spec, &built, &compressed, &mut published, &mut messages)
                .expect("no reuse")
                .is_none()
        );
    }
}
