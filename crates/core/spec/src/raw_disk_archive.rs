//! Raw disk image archive names (`image.output.archive_name`): the disk
//! itself (`.img`, `.raw`) or compressed with xz (`.img.xz`, `.raw.xz`) or
//! zstd (`.img.zst`, `.raw.zst`). zstd compresses and decompresses several
//! times faster, for development images; xz is smaller, for releases.

/// How a raw disk archive is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawDiskArchive {
    Plain,
    Xz,
    Zstd,
}

impl RawDiskArchive {
    /// The compressor and its arguments for `jobs` threads (0 = all
    /// cores); it reads the disk given after them and writes to stdout.
    /// Both produce the same bytes for any thread count, so archives stay
    /// reproducible across machines.
    pub fn compressor(self, jobs: u32) -> Option<(&'static str, Vec<String>)> {
        match self {
            Self::Plain => None,
            Self::Xz => Some((
                "xz",
                vec![
                    format!("-T{jobs}"),
                    "--block-size=24MiB".to_string(),
                    "-c".to_string(),
                ],
            )),
            Self::Zstd => Some((
                "zstd",
                vec![format!("-T{jobs}"), "-q".to_string(), "-c".to_string()],
            )),
        }
    }
}

/// The kind of raw disk archive a file name asks for, or `None` when it is
/// not one (a tar archive, for example).
pub fn raw_disk_archive(name: &str) -> Option<RawDiskArchive> {
    let lowered = name.to_ascii_lowercase();
    let is_disk = |suffix: &str| {
        [".img", ".raw"]
            .iter()
            .any(|disk| lowered.ends_with(&format!("{disk}{suffix}")))
    };
    if is_disk("") {
        Some(RawDiskArchive::Plain)
    } else if is_disk(".xz") {
        Some(RawDiskArchive::Xz)
    } else if is_disk(".zst") {
        Some(RawDiskArchive::Zstd)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_disk_archive_names_select_the_compression() {
        assert_eq!(raw_disk_archive("raze.img"), Some(RawDiskArchive::Plain));
        assert_eq!(raw_disk_archive("raze.RAW"), Some(RawDiskArchive::Plain));
        assert_eq!(
            raw_disk_archive("raze-1.2.img.xz"),
            Some(RawDiskArchive::Xz)
        );
        assert_eq!(raw_disk_archive("raze.img.zst"), Some(RawDiskArchive::Zstd));
        assert_eq!(raw_disk_archive("raze.raw.zst"), Some(RawDiskArchive::Zstd));
        assert_eq!(raw_disk_archive("rootfs.tar"), None);
        assert_eq!(raw_disk_archive("rootfs.tar.zst"), None);
        assert_eq!(raw_disk_archive("notes.xz"), None);
        assert_eq!(
            RawDiskArchive::Zstd.compressor(0),
            Some(("zstd", vec!["-T0".into(), "-q".into(), "-c".into()]))
        );
    }
}
