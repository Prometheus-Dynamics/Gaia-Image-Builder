use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{
    ProcessCancelCheck, ProcessLogSink, ProcessOutputRetention, output_text,
    run_command_stdout_to_file_with_timeout_and_retention,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarArchiveValidationError {
    pub message: String,
}

/// Validates every entry of the archive. The listing is streamed to a file
/// and read back line by line, so no entry is skipped however large the
/// archive is: the process runner keeps only the last few stdout lines.
pub fn validate_tar_archive_entries(
    archive_path: &Path,
    strip_components: usize,
    timeout: Duration,
    label: &str,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), TarArchiveValidationError> {
    validate_tar_archive_entries_with(
        archive_path,
        strip_components,
        false,
        timeout,
        label,
        sink,
        cancel_check,
    )
}

/// [`validate_tar_archive_entries`], optionally accepting symlinks with an
/// absolute target. A root filesystem or an SDK's sysroot holds them
/// (`usr/bin/iptables-xml -> /usr/sbin/xtables-legacy-multi`), meant
/// relative to that root; the link is created inside the destination, and
/// GNU tar does not write through a link it extracted.
pub fn validate_tar_archive_entries_with(
    archive_path: &Path,
    strip_components: usize,
    allow_absolute_symlinks: bool,
    timeout: Duration,
    label: &str,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), TarArchiveValidationError> {
    let listing = ListingFile::beside(archive_path);
    let mut command = Command::new("tar");
    command
        .arg("-tvf")
        .arg(archive_path)
        .arg("--quoting-style=c");
    let result = run_command_stdout_to_file_with_timeout_and_retention(
        &mut command,
        &listing.path,
        timeout,
        label,
        ProcessOutputRetention::default(),
        sink,
        cancel_check,
    )
    .map_err(|error| TarArchiveValidationError {
        message: error.message,
    })?;
    if !result.output.status.success() {
        return Err(TarArchiveValidationError {
            message: format!("{label} failed: {}", output_text(&result.output)),
        });
    }
    let file = File::open(&listing.path).map_err(|error| TarArchiveValidationError {
        message: format!(
            "failed to read {label} listing '{}': {error}",
            listing.path.display()
        ),
    })?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| TarArchiveValidationError {
            message: format!("failed to read {label} listing: {error}"),
        })?;
        validate_tar_listing_line_with(&line, strip_components, allow_absolute_symlinks)?;
    }
    Ok(())
}

/// A scratch file for a tar listing, removed when dropped. It sits beside the
/// archive rather than in the temp dir, which may be RAM-backed; when the
/// archive's directory is not writable it falls back to the temp dir.
struct ListingFile {
    path: PathBuf,
}

impl ListingFile {
    fn beside(archive_path: &Path) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = archive_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = format!(".{name}.listing-{}-{unique}", std::process::id());
        let beside = archive_path.with_file_name(&file_name);
        let path = if fs::File::create(&beside).is_ok() {
            beside
        } else {
            std::env::temp_dir().join(file_name)
        };
        Self { path }
    }
}

impl Drop for ListingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
fn validate_tar_listing_line(
    line: &str,
    strip_components: usize,
) -> Result<(), TarArchiveValidationError> {
    validate_tar_listing_line_with(line, strip_components, false)
}

fn validate_tar_listing_line_with(
    line: &str,
    strip_components: usize,
    allow_absolute_symlinks: bool,
) -> Result<(), TarArchiveValidationError> {
    let entry_kind = line.chars().next().unwrap_or('-');
    let quoted = parse_c_quoted_strings(line).ok_or_else(|| TarArchiveValidationError {
        message: format!("failed to parse tar listing line: {line}"),
    })?;
    let Some(entry_path) = quoted.first() else {
        return Err(TarArchiveValidationError {
            message: format!("tar listing entry did not include a path: {line}"),
        });
    };
    let stripped_path = strip_tar_components(entry_path, strip_components)?;
    match entry_kind {
        '-' | 'd' => Ok(()),
        'l' => {
            let Some(target) = quoted.get(1) else {
                return Err(TarArchiveValidationError {
                    message: format!("tar symlink entry '{entry_path}' did not include a target"),
                });
            };
            if allow_absolute_symlinks && Path::new(target).is_absolute() {
                return Ok(());
            }
            validate_relative_link_target(&stripped_path, target)
        }
        'h' => {
            // A hard link names another member of the archive by its archive
            // path; it must resolve inside the archive and survive the strip.
            let Some(target) = quoted.get(1) else {
                return Err(TarArchiveValidationError {
                    message: format!("tar hardlink entry '{entry_path}' did not include a target"),
                });
            };
            let resolved = resolve_inside_archive(Path::new(target)).ok_or_else(|| {
                TarArchiveValidationError {
                    message: format!(
                        "tar hardlink '{entry_path}' points to '{target}', which escapes the extraction destination"
                    ),
                }
            })?;
            if resolved.components().count() <= strip_components {
                return Err(TarArchiveValidationError {
                    message: format!(
                        "tar hardlink '{entry_path}' points to '{target}', which strip_components removes"
                    ),
                });
            }
            Ok(())
        }
        other => Err(TarArchiveValidationError {
            message: format!("tar entry '{entry_path}' has unsupported type '{other}'"),
        }),
    }
}

fn strip_tar_components(
    path: &str,
    strip_components: usize,
) -> Result<PathBuf, TarArchiveValidationError> {
    let normalized = normalize_archive_path(path)?;
    let stripped = normalized
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .skip(strip_components)
        .collect::<PathBuf>();
    if stripped.as_os_str().is_empty() {
        return Ok(stripped);
    }
    normalize_archive_path(&stripped.display().to_string())
}

fn normalize_archive_path(path: &str) -> Result<PathBuf, TarArchiveValidationError> {
    let path = Path::new(path);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(TarArchiveValidationError {
                    message: format!(
                        "tar entry '{}' escapes the extraction destination",
                        path.display()
                    ),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(TarArchiveValidationError {
                    message: format!(
                        "tar entry '{}' uses an absolute or prefixed path",
                        path.display()
                    ),
                });
            }
        }
    }
    Ok(out)
}

fn validate_relative_link_target(
    entry_path: &Path,
    target: &str,
) -> Result<(), TarArchiveValidationError> {
    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return Err(TarArchiveValidationError {
            message: format!(
                "tar symlink '{}' points to absolute target '{}'",
                entry_path.display(),
                target
            ),
        });
    }
    // `..` in a link target is resolved against the link's directory; only
    // a target that leaves the extraction destination is refused (headers
    // link `include/dt-bindings/input/../../uapi/...`, which stays inside).
    let parent = entry_path.parent().unwrap_or_else(|| Path::new(""));
    let joined = parent.join(target_path);
    resolve_inside_archive(&joined)
        .map(|_| ())
        .ok_or_else(|| TarArchiveValidationError {
            message: format!(
                "tar symlink '{}' points to '{}', which escapes the extraction destination",
                entry_path.display(),
                target
            ),
        })
}

/// `path` with `.` and `..` resolved lexically, or `None` when it is absolute
/// or a `..` would climb above the archive root.
fn resolve_inside_archive(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn parse_c_quoted_strings(line: &str) -> Option<Vec<String>> {
    let mut values = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut value = String::new();
        while let Some((_, ch)) = chars.next() {
            match ch {
                '"' => {
                    values.push(value);
                    break;
                }
                '\\' => value.push(parse_c_escape(&mut chars)?),
                other => value.push(other),
            }
        }
    }
    Some(values)
}

fn parse_c_escape<I>(chars: &mut std::iter::Peekable<I>) -> Option<char>
where
    I: Iterator<Item = (usize, char)>,
{
    let (_, ch) = chars.next()?;
    Some(match ch {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        '\\' => '\\',
        '"' => '"',
        '0'..='7' => {
            let mut value = ch.to_digit(8)?;
            for _ in 0..2 {
                let Some((_, next)) = chars.peek().copied() else {
                    break;
                };
                let Some(digit) = next.to_digit(8) else {
                    break;
                };
                let _ = chars.next();
                value = value * 8 + digit;
            }
            char::from_u32(value)?
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::fs;

    #[test]
    fn validate_tar_archive_entries_accepts_safe_archive() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("safe-tar");
        let src = dir.join("src");
        fs::create_dir_all(src.join("dir")).expect("source dir");
        fs::write(src.join("dir/file.txt"), "ok").expect("source file");
        let archive = dir.join("safe.tar");
        create_tar(&archive, &src, &["dir/file.txt"]);

        validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "safe tar validation",
            None,
            None,
        )
        .expect("safe archive should validate");
    }

    #[test]
    fn validate_tar_archive_entries_accepts_safe_archive_with_strip_components() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("safe-strip-tar");
        let src = dir.join("src");
        fs::create_dir_all(src.join("outer/dir")).expect("source dir");
        fs::write(src.join("outer/dir/file.txt"), "ok").expect("source file");
        let archive = dir.join("safe-strip.tar");
        create_tar(&archive, &src, &["outer/dir/file.txt"]);

        validate_tar_archive_entries(
            &archive,
            1,
            Duration::from_secs(5),
            "safe stripped tar validation",
            None,
            None,
        )
        .expect("safe stripped archive should validate");
    }

    #[test]
    fn validate_tar_archive_entries_rejects_parent_traversal() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("traversal-tar");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("source dir");
        fs::write(src.join("file.txt"), "ok").expect("source file");
        let archive = dir.join("bad.tar");
        let status = Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("--transform=s#file.txt#../escape.txt#")
            .arg("file.txt")
            .status()
            .expect("create traversal tar");
        assert!(status.success());

        let error = validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "traversal tar validation",
            None,
            None,
        )
        .expect_err("traversal archive should fail");

        assert!(error.message.contains("escapes the extraction destination"));
    }

    #[test]
    fn validate_tar_archive_entries_rejects_parent_traversal_before_strip_components() {
        let error = validate_tar_listing_line(
            "-rw-r--r-- root/root 0 2026-01-01 \"outer/../escape.txt\"",
            1,
        )
        .expect_err("strip components must not hide traversal");

        assert!(error.message.contains("escapes the extraction destination"));
    }

    #[test]
    fn absolute_symlinks_are_refused_unless_allowed() {
        let line = "lrwxrwxrwx root/root 0 2026-01-01 \"sysroot/usr/bin/iptables-xml\" -> \"/usr/sbin/xtables-legacy-multi\"";
        assert!(validate_tar_listing_line_with(line, 0, false).is_err());
        assert!(validate_tar_listing_line_with(line, 0, true).is_ok());
        // Entry names stay strict either way.
        assert!(
            validate_tar_listing_line_with(
                "-rw-r--r-- root/root 0 2026-01-01 \"/etc/passwd\"",
                0,
                true
            )
            .is_err()
        );
    }

    #[test]
    fn links_resolving_inside_the_archive_are_allowed_and_escaping_ones_refused() {
        // A kernel-header symlink that climbs and comes back down.
        assert!(validate_tar_listing_line(
            "lrwxrwxrwx root/root 0 2026-01-01 \"sys/include/dt-bindings/input/linux-event-codes.h\" -> \"../../uapi/linux/input-event-codes.h\"",
            0,
        )
        .is_ok());
        assert!(
            validate_tar_listing_line(
                "lrwxrwxrwx root/root 0 2026-01-01 \"sys/a\" -> \"../../../etc/passwd\"",
                0,
            )
            .is_err()
        );
        // Hard links between members (toolchains link gcc to gcc-14.x).
        assert!(
            validate_tar_listing_line(
                "hrwxr-xr-x root/root 0 2026-01-01 \"sdk/bin/gcc\" link to \"sdk/bin/gcc-14.2.0\"",
                1,
            )
            .is_ok()
        );
        assert!(
            validate_tar_listing_line(
                "hrwxr-xr-x root/root 0 2026-01-01 \"sdk/bin/gcc\" link to \"../outside\"",
                0,
            )
            .is_err()
        );
        assert!(
            validate_tar_listing_line(
                "hrwxr-xr-x root/root 0 2026-01-01 \"sdk/bin/gcc\" link to \"sdk\"",
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn validate_tar_archive_entries_rejects_absolute_paths() {
        let error =
            validate_tar_listing_line("-rw-r--r-- root/root 0 2026-01-01 \"/etc/passwd\"", 0)
                .expect_err("absolute archive paths should fail");

        assert!(error.message.contains("absolute or prefixed path"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_rejects_absolute_symlink() {
        let scratch = TempDir::new("gaia-process-test");
        use std::os::unix::fs::symlink;

        let dir = scratch.join("absolute-symlink-tar");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("source dir");
        symlink("/tmp", src.join("link")).expect("symlink");
        let archive = dir.join("bad-link.tar");
        create_tar(&archive, &src, &["link"]);

        let error = validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "symlink tar validation",
            None,
            None,
        )
        .expect_err("absolute symlink archive should fail");

        assert!(error.message.contains("absolute target"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_rejects_relative_symlink_escape() {
        let scratch = TempDir::new("gaia-process-test");
        use std::os::unix::fs::symlink;

        let dir = scratch.join("relative-symlink-tar");
        let src = dir.join("src");
        fs::create_dir_all(src.join("dir")).expect("source dir");
        symlink("../../escape", src.join("dir/link")).expect("symlink");
        let archive = dir.join("bad-relative-link.tar");
        create_tar(&archive, &src, &["dir/link"]);

        let error = validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "relative symlink tar validation",
            None,
            None,
        )
        .expect_err("relative symlink escape should fail");

        assert!(error.message.contains("escapes the extraction destination"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_accept_hardlinks_inside_the_archive() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("hardlink-tar");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("source dir");
        fs::write(src.join("file.txt"), "ok").expect("source file");
        fs::hard_link(src.join("file.txt"), src.join("hard.txt")).expect("hard link");
        let archive = dir.join("hardlink.tar");
        create_tar(&archive, &src, &["file.txt", "hard.txt"]);

        // GNU tar lists the second name as `h... "hard.txt" link to "file.txt"`.
        validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "hardlink tar validation",
            None,
            None,
        )
        .expect("a hard link to another member is allowed");
    }

    #[test]
    fn validate_tar_archive_entries_rejects_special_file_types() {
        let error = validate_tar_listing_line("prw-r--r-- root/root 0 2026-01-01 \"pipe\"", 0)
            .expect_err("fifo archive entries should fail");

        assert!(error.message.contains("unsupported type"));
    }

    /// The bad entry is first, so it is outside the last lines the process
    /// runner retains. Every entry must still be checked.
    #[test]
    fn validate_tar_archive_entries_rejects_bad_entry_before_many_benign_entries() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("early-bad-tar");
        let src = dir.join("src");
        fs::create_dir_all(src.join("benign")).expect("source dir");
        fs::write(src.join("bad.txt"), "bad").expect("bad source");
        for index in 0..1_500 {
            fs::write(src.join(format!("benign/file-{index:04}.txt")), "ok").expect("benign");
        }
        let archive = dir.join("early-bad.tar");
        let status = Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("--transform=s#^bad.txt$#../escape.txt#")
            .arg("bad.txt")
            .status()
            .expect("create bad tar");
        assert!(status.success());
        let status = Command::new("tar")
            .arg("-rf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("benign")
            .status()
            .expect("append benign entries");
        assert!(status.success());

        let error = validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(60),
            "early bad entry tar validation",
            None,
            None,
        )
        .expect_err("a bad entry before 1,500 benign ones must still be rejected");

        assert!(error.message.contains("escapes the extraction destination"));
        let leftover = fs::read_dir(&dir)
            .expect("list dir")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".listing-"));
        assert!(!leftover, "the listing scratch file must be removed");
    }

    fn create_tar(archive: &Path, source_dir: &Path, entries: &[&str]) {
        let status = Command::new("tar")
            .arg("-cf")
            .arg(archive)
            .arg("-C")
            .arg(source_dir)
            .args(entries)
            .status()
            .expect("create tar");
        assert!(status.success());
    }
}
