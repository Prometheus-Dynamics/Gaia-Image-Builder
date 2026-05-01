use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{ProcessCancelCheck, ProcessLogSink, output_text, run_command_with_timeout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarArchiveValidationError {
    pub message: String,
}

pub fn validate_tar_archive_entries(
    archive_path: &Path,
    strip_components: usize,
    timeout: Duration,
    label: &str,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), TarArchiveValidationError> {
    let mut command = Command::new("tar");
    command
        .arg("-tvf")
        .arg(archive_path)
        .arg("--quoting-style=c");
    let result = run_command_with_timeout(&mut command, timeout, label, sink, cancel_check)
        .map_err(|error| TarArchiveValidationError {
            message: error.message,
        })?;
    if !result.output.status.success() {
        return Err(TarArchiveValidationError {
            message: format!("{label} failed: {}", output_text(&result.output)),
        });
    }
    for line in result.stdout_lines {
        validate_tar_listing_line(&line, strip_components)?;
    }
    Ok(())
}

fn validate_tar_listing_line(
    line: &str,
    strip_components: usize,
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
            validate_relative_link_target(&stripped_path, target)
        }
        'h' => Err(TarArchiveValidationError {
            message: format!("tar hardlink entry '{entry_path}' is not allowed"),
        }),
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
    let parent = entry_path.parent().unwrap_or_else(|| Path::new(""));
    let joined = parent.join(target_path);
    normalize_archive_path(&joined.display().to_string()).map(|_| ())
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn validate_tar_archive_entries_accepts_safe_archive() {
        let dir = unique_dir("safe-tar");
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

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_tar_archive_entries_accepts_safe_archive_with_strip_components() {
        let dir = unique_dir("safe-strip-tar");
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

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_tar_archive_entries_rejects_parent_traversal() {
        let dir = unique_dir("traversal-tar");
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
        let _ = fs::remove_dir_all(dir);
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
    fn validate_tar_archive_entries_rejects_absolute_paths() {
        let error =
            validate_tar_listing_line("-rw-r--r-- root/root 0 2026-01-01 \"/etc/passwd\"", 0)
                .expect_err("absolute archive paths should fail");

        assert!(error.message.contains("absolute or prefixed path"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_rejects_absolute_symlink() {
        use std::os::unix::fs::symlink;

        let dir = unique_dir("absolute-symlink-tar");
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
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_rejects_relative_symlink_escape() {
        use std::os::unix::fs::symlink;

        let dir = unique_dir("relative-symlink-tar");
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
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn validate_tar_archive_entries_rejects_hardlinks() {
        let dir = unique_dir("hardlink-tar");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("source dir");
        fs::write(src.join("file.txt"), "ok").expect("source file");
        fs::hard_link(src.join("file.txt"), src.join("hard.txt")).expect("hard link");
        let archive = dir.join("hardlink.tar");
        create_tar(&archive, &src, &["file.txt", "hard.txt"]);

        let error = validate_tar_archive_entries(
            &archive,
            0,
            Duration::from_secs(5),
            "hardlink tar validation",
            None,
            None,
        )
        .expect_err("hardlink archive should fail");

        assert!(error.message.contains("hardlink"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_tar_archive_entries_rejects_special_file_types() {
        let error = validate_tar_listing_line("prw-r--r-- root/root 0 2026-01-01 \"pipe\"", 0)
            .expect_err("fifo archive entries should fail");

        assert!(error.message.contains("unsupported type"));
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

    fn unique_dir(name: &str) -> PathBuf {
        let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join("gaia-tests").join(format!(
            "gaia-process-tar-{name}-{}-{counter}",
            std::process::id()
        ))
    }
}
