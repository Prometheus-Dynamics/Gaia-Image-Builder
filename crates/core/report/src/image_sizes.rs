//! Raw and content sizes of the disk images a run published. A raw image
//! can be shorter than its partition table (`truncate = "last-data"`) and
//! sparse, so the file length is not what was written: the content size is
//! the bytes the filesystem allocates for it.

use crate::model::ImageSizeRecord;
use std::path::{Path, PathBuf};

/// Records for `paths` that are files, in the given order. A path that is
/// missing or not a file is skipped rather than failing the report.
pub(crate) fn image_size_records(paths: &[PathBuf]) -> Vec<ImageSizeRecord> {
    paths
        .iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(path).ok()?;
            if !metadata.is_file() {
                return None;
            }
            let raw_bytes = metadata.len();
            Some(ImageSizeRecord {
                path: path.display().to_string(),
                raw_bytes,
                content_bytes: allocated_bytes(path, raw_bytes),
            })
        })
        .collect()
}

/// One human line per image, such as `sdcard.img: 1.36 GB raw, 412 MB written`.
pub(crate) fn image_size_note(record: &ImageSizeRecord) -> String {
    let name = Path::new(&record.path).file_name().map_or_else(
        || record.path.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    format!(
        "{name}: {} raw, {} written",
        human_bytes(record.raw_bytes),
        human_bytes(record.content_bytes)
    )
}

/// Bytes allocated for `path`, from the block count the filesystem reports
/// (512-byte units, holes excluded). Capped at the file length. Falls back to
/// the file length when the block count is not available.
fn allocated_bytes(path: &Path, raw_bytes: u64) -> u64 {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.blocks().saturating_mul(512).min(raw_bytes),
        Err(_) => raw_bytes,
    }
}

/// Decimal units (kB, MB, GB), matching how image sizes are usually quoted.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1_000_000_000), ("MB", 1_000_000), ("kB", 1_000)];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            return format!("{:.2} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} B")
}

/// Image paths for the summary: the primary image first, then each disk the
/// run published, without repeats.
pub(crate) fn summary_image_paths(
    primary: Option<&str>,
    disk_images: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in primary.map(PathBuf::from).into_iter().chain(disk_images) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gaia-image-sizes-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("disk.img")
    }

    #[test]
    fn content_size_counts_written_data_not_holes() {
        let path = temp_file("holes");
        let mut file = std::fs::File::create(&path).expect("create");
        // One 4 KiB written block, then a 1 MiB hole, then a 4 KiB block.
        file.write_all(&[0xab; 4096]).expect("first block");
        file.seek(SeekFrom::Current(1024 * 1024)).expect("hole");
        file.write_all(&[0xcd; 4096]).expect("second block");
        file.set_len(1024 * 1024 + 8192).expect("length");
        drop(file);

        let records = image_size_records(std::slice::from_ref(&path));
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.raw_bytes, 1024 * 1024 + 8192);
        assert!(record.content_bytes >= 8192, "{record:?}");
        assert!(
            record.content_bytes < record.raw_bytes,
            "a sparse hole must not count as content: {record:?}"
        );
        assert!(image_size_note(record).starts_with("disk.img: "));
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn content_size_is_never_more_than_the_file_length() {
        let path = temp_file("short");
        std::fs::write(&path, [1u8; 100]).expect("write");
        let records = image_size_records(std::slice::from_ref(&path));
        assert!(records[0].content_bytes <= records[0].raw_bytes);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn missing_paths_and_directories_are_skipped() {
        let missing = temp_file("missing");
        let dir = missing.parent().expect("dir").to_path_buf();
        assert!(image_size_records(&[missing, dir.clone()]).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn summary_paths_put_primary_first_and_skip_repeats() {
        let paths = summary_image_paths(
            Some("/out/a.img"),
            [PathBuf::from("/out/a.img"), PathBuf::from("/out/b.img")],
        );
        assert_eq!(
            paths,
            vec![PathBuf::from("/out/a.img"), PathBuf::from("/out/b.img")]
        );
    }

    #[test]
    fn human_bytes_uses_decimal_units() {
        assert_eq!(human_bytes(1_360_000_000), "1.36 GB");
        assert_eq!(human_bytes(412_000_000), "412.00 MB");
        assert_eq!(human_bytes(512), "512 B");
    }
}
