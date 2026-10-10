//! `gaia run --export <dir>`: copies a successful run's primary image
//! output into a directory under a versioned name.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// One exported image: where it landed, its digest and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

/// Disk image suffixes, compression included; longest first so that
/// `.img.xz` is split as one extension.
const DISK_IMAGE_SUFFIXES: [&str; 11] = [
    ".img.xz", ".img.gz", ".img.zst", ".raw.xz", ".raw.gz", ".raw.zst", ".wic.xz", ".wic.gz",
    ".img", ".raw", ".wic",
];

/// Archive suffixes that are also kept whole when a name is split.
const ARCHIVE_SUFFIXES: [&str; 4] = [".tar.xz", ".tar.gz", ".tar.zst", ".tar"];

/// Largest `-N` collision suffix tried before giving up on a name.
const MAX_COLLISION_SUFFIX: u32 = 999;

/// The files to export for a run's primary output: the file itself, or,
/// when the output is a collect directory, the disk images inside it.
pub(crate) fn primary_images(primary: &Path) -> io::Result<Vec<PathBuf>> {
    if primary.is_file() {
        return Ok(vec![primary.to_path_buf()]);
    }
    if !primary.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("primary output '{}' does not exist", primary.display()),
        ));
    }
    let mut images = fs::read_dir(primary)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| DISK_IMAGE_SUFFIXES.iter().any(|s| name.ends_with(s)))
        })
        .collect::<Vec<_>>();
    images.sort();
    Ok(images)
}

/// Splits a file name into stem and extension, keeping compound image and
/// archive suffixes (`.img.xz`, `.tar.zst`) whole.
pub(crate) fn split_image_extension(name: &str) -> (&str, &str) {
    for suffix in DISK_IMAGE_SUFFIXES.iter().chain(ARCHIVE_SUFFIXES.iter()) {
        if name.len() > suffix.len() && name.ends_with(suffix) {
            return name.split_at(name.len() - suffix.len());
        }
    }
    match name.rfind('.') {
        Some(index) if index > 0 => name.split_at(index),
        _ => (name, ""),
    }
}

/// True when a file stem already carries a version such as `v2027.0.0` or
/// `9.9.9` as one of its `-`/`_`-separated parts.
fn has_version_token(stem: &str) -> bool {
    stem.split(['-', '_']).any(is_version_part)
}

fn is_version_part(part: &str) -> bool {
    let digits = part.strip_prefix(['v', 'V']).unwrap_or(part);
    let groups = digits.split('.').collect::<Vec<_>>();
    groups.len() >= 2
        && groups
            .iter()
            .all(|group| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The name an exported image gets. A name that already carries a version
/// (see `has_version_token`) is kept; otherwise it becomes `<build>-<suffix><ext>`, where the
/// suffix is the build version, or the run's UTC stamp without one.
pub(crate) fn export_file_name(
    source_name: &str,
    build: &str,
    version: Option<&str>,
    run_stamp: &str,
) -> String {
    let (stem, ext) = split_image_extension(source_name);
    let version = version.map(str::trim).filter(|version| !version.is_empty());
    if has_version_token(stem) {
        return source_name.to_string();
    }
    let suffix = version
        .map(sanitize_component)
        .unwrap_or_else(|| run_stamp.to_string());
    format!("{}-{suffix}{ext}", sanitize_component(build))
}

fn sanitize_component(value: &str) -> String {
    value.replace(['/', '\\', ' '], "-")
}

/// Copies each source into `dir` under its export name, creating `dir` when
/// needed. A name already taken by different content is never overwritten:
/// the copy takes `-1`, `-2`, ... before the extension. A name already taken
/// by identical content is reused.
pub(crate) fn export_images(
    sources: &[PathBuf],
    dir: &Path,
    build: &str,
    version: Option<&str>,
    run_stamp: &str,
) -> io::Result<Vec<ExportedFile>> {
    fs::create_dir_all(dir)?;
    let dir = fs::canonicalize(dir)?;
    let mut exported = Vec::new();
    for source in sources {
        let source_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("'{}' has no usable file name", source.display()),
                )
            })?;
        let name = export_file_name(source_name, build, version, run_stamp);
        exported.push(place_unique(source, &dir, &name)?);
    }
    Ok(exported)
}

/// Copies `source` into `dir` as `name`, or as the first free `-N` variant.
fn place_unique(source: &Path, dir: &Path, name: &str) -> io::Result<ExportedFile> {
    let temp = dir.join(format!(".{name}.partial-{}", process::id()));
    let copied = copy_hashed(source, &temp);
    let (sha256, bytes) = match copied {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };
    let result = place_temp(&temp, dir, name, &sha256);
    let _ = fs::remove_file(&temp);
    result.map(|path| ExportedFile {
        path,
        sha256,
        bytes,
    })
}

fn place_temp(temp: &Path, dir: &Path, name: &str, sha256: &str) -> io::Result<PathBuf> {
    let (stem, ext) = split_image_extension(name);
    for attempt in 0..=MAX_COLLISION_SUFFIX {
        let candidate_name = if attempt == 0 {
            name.to_string()
        } else {
            format!("{stem}-{attempt}{ext}")
        };
        let candidate = dir.join(candidate_name);
        match fs::hard_link(temp, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(_) if candidate.exists() => {
                if sha256_file(&candidate)? == sha256 {
                    return Ok(candidate);
                }
            }
            // The directory cannot hold hard links: move the copy into place.
            Err(_) => {
                fs::rename(temp, &candidate)?;
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free name for '{name}' in '{}'", dir.display()),
    ))
}

/// Copies `source` to `target` (which must not exist), hashing as it goes.
fn copy_hashed(source: &Path, target: &Path) -> io::Result<(String, u64)> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(target)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut bytes = 0u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
        bytes += read as u64;
    }
    output.sync_all()?;
    Ok((hex(&hasher.finalize()), bytes))
}

pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The current time as a compact UTC stamp, such as `20261009T142530Z`.
pub(crate) fn run_stamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    utc_stamp(seconds)
}

fn utc_stamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let secs = unix_seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("gaia-app-export-{name}-{}-{nonce}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn versioned_names_are_kept_and_unversioned_ones_get_a_suffix() {
        let name = "photonvision-full-raze-dev-v2027.0.0-alpha-2-94-gc9548d91.img.xz";
        assert_eq!(
            export_file_name(name, "photonvision", Some("2027.0.0"), "stamp"),
            name
        );
        assert_eq!(
            export_file_name("default-9.9.9.tar", "default", Some("9.9.9"), "stamp"),
            "default-9.9.9.tar"
        );
        assert_eq!(
            export_file_name("rootfs.img.xz", "heli os", Some("2.1"), "stamp"),
            "heli-os-2.1.img.xz"
        );
        assert_eq!(
            export_file_name("disk.img", "heli", None, "20261009T142530Z"),
            "heli-20261009T142530Z.img"
        );
    }

    #[test]
    fn extension_split_keeps_compound_suffixes_whole() {
        assert_eq!(split_image_extension("a-1.0.img.xz"), ("a-1.0", ".img.xz"));
        assert_eq!(split_image_extension("a.b.tar.zst"), ("a.b", ".tar.zst"));
        assert_eq!(split_image_extension("a.b.txt"), ("a.b", ".txt"));
        assert_eq!(split_image_extension("README"), ("README", ""));
        assert_eq!(split_image_extension(".hidden"), (".hidden", ""));
    }

    #[test]
    fn utc_stamp_formats_known_instants() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_000_000_000), "20010909T014640Z");
        assert_eq!(utc_stamp(1_781_000_000), "20260609T101320Z");
    }

    #[test]
    fn export_copies_under_the_versioned_name_with_the_right_sha256() {
        let root = temp_dir("copy");
        let source = root.join("archive.img");
        fs::write(&source, b"gaia image bytes").expect("source");
        let dest = root.join("out");

        let exported = export_images(
            std::slice::from_ref(&source),
            &dest,
            "heli",
            Some("2.1.0"),
            "stamp",
        )
        .expect("export");

        assert_eq!(exported.len(), 1);
        let file = &exported[0];
        assert_eq!(
            file.path,
            fs::canonicalize(&dest)
                .expect("dest")
                .join("heli-2.1.0.img")
        );
        assert_eq!(file.bytes, 16);
        // sha256 of "gaia image bytes", computed independently.
        assert_eq!(
            file.sha256,
            "9e32268e4587fa5cf4458d573dd7fcfd99ccba9c76e40ec0dc2906253554e08b"
        );
        assert_eq!(fs::read(&file.path).expect("copy"), b"gaia image bytes");
        let leftovers = fs::read_dir(&dest)
            .expect("dest")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0, "no partial copy is left behind");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn collisions_never_overwrite_different_content_and_reuse_identical_content() {
        let root = temp_dir("collide");
        let dest = root.join("out");
        let first = root.join("first.img");
        fs::write(&first, b"first").expect("first");
        let second = root.join("second.img");
        fs::write(&second, b"second").expect("second");

        let a = export_images(std::slice::from_ref(&first), &dest, "heli", Some("1"), "s")
            .expect("first export");
        let b = export_images(std::slice::from_ref(&second), &dest, "heli", Some("1"), "s")
            .expect("second export");
        assert!(a[0].path.ends_with("heli-1.img"));
        assert!(b[0].path.ends_with("heli-1-1.img"));
        assert_eq!(fs::read(&a[0].path).expect("a"), b"first");
        assert_eq!(fs::read(&b[0].path).expect("b"), b"second");

        // The same content again: the existing copy is reused, nothing new.
        let c = export_images(std::slice::from_ref(&first), &dest, "heli", Some("1"), "s")
            .expect("third export");
        assert_eq!(c[0].path, a[0].path);
        assert_eq!(fs::read_dir(&dest).expect("dest").count(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn primary_images_of_a_directory_are_its_disk_images_only() {
        let root = temp_dir("dir");
        fs::write(root.join("b.img.xz"), "x").expect("b");
        fs::write(root.join("a.img"), "x").expect("a");
        fs::write(root.join("notes.txt"), "x").expect("notes");
        fs::write(root.join("rootfs.ext4"), "x").expect("rootfs");
        let names = primary_images(&root)
            .expect("images")
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["a.img", "b.img.xz"]);
        assert!(primary_images(&root.join("missing")).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
