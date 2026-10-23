//! Entry archives: a tar of the entry's members, compressed with zstd, made and
//! read by the host's `tar` and `zstd`.
//!
//! Unpacking trusts nothing in the archive. Every member must sit under the
//! entry's own name, and links and devices are refused, so an entry cannot
//! write outside its directory or through a symlink it carries.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

use super::RegistryError;

/// Packs `members` (paths relative to `dir`) into `out` as tar.zst. The member
/// order, owner and group are fixed, so the same tree packs to the same bytes.
pub fn pack_entry(dir: &Path, members: &[&str], out: &Path) -> Result<(), RegistryError> {
    if members.is_empty() {
        return Err(RegistryError::Protocol(
            "an entry needs at least one member".into(),
        ));
    }
    for member in members {
        check_member_path(member)?;
    }
    let mut tar = Command::new("tar")
        .arg("-C")
        .arg(dir)
        .args([
            "--sort=name",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "-cf",
            "-",
        ])
        .args(members)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io("cannot run tar", error))?;
    let tar_stdout = tar
        .stdout
        .take()
        .ok_or_else(|| RegistryError::Io("tar gave no output pipe".into()))?;
    let file = File::create(out).map_err(|error| io(&out.display().to_string(), error))?;
    let mut zstd = Command::new("zstd")
        .args(["-q", "-c"])
        .stdin(Stdio::from(tar_stdout))
        .stdout(Stdio::from(file))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io("cannot run zstd", error))?;
    let zstd_status = zstd.wait().map_err(|error| io("zstd", error))?;
    let tar_status = tar.wait().map_err(|error| io("tar", error))?;
    if !tar_status.success() || !zstd_status.success() {
        let _ = fs::remove_file(out);
        return Err(RegistryError::Io(format!(
            "packing {} failed (tar {tar_status}, zstd {zstd_status})",
            out.display()
        )));
    }
    Ok(())
}

/// Unpacks the tar.zst `blob` into `into`, which is created if needed. Every
/// member must be `<expect_prefix>.json` or sit under `<expect_prefix>/`, with
/// no absolute path or `..`, and must be a regular file or directory. Returns
/// the member names.
pub fn unpack_entry(
    blob: &Path,
    into: &Path,
    expect_prefix: &str,
) -> Result<Vec<String>, RegistryError> {
    if expect_prefix.is_empty() || expect_prefix.contains('/') || expect_prefix == ".." {
        return Err(RegistryError::Protocol(format!(
            "'{expect_prefix}' is not an entry name"
        )));
    }
    let names = list(blob, "-tf")?;
    let kinds = list(blob, "-tvf")?;
    let listed: Vec<&str> = names.lines().collect();
    if listed.len() != kinds.lines().count() {
        return Err(RegistryError::Protocol("archive listings disagree".into()));
    }
    for (name, line) in names.lines().zip(kinds.lines()) {
        if !matches!(line.chars().next(), Some('-' | 'd')) {
            return Err(RegistryError::Protocol(format!(
                "archive member '{name}' is a link or device, which entries do not carry"
            )));
        }
        if !member_allowed(name, expect_prefix) {
            return Err(RegistryError::Protocol(format!(
                "archive member '{name}' is outside '{expect_prefix}'"
            )));
        }
    }
    fs::create_dir_all(into).map_err(|error| io(&into.display().to_string(), error))?;
    extract(blob, into)?;
    Ok(listed.into_iter().map(str::to_string).collect())
}

/// Whether `name` is the entry's manifest-sized file or sits under its directory.
fn member_allowed(name: &str, prefix: &str) -> bool {
    if name.starts_with('/') || name.split('/').any(|part| part == "..") {
        return false;
    }
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest == ".json" || rest.starts_with('/'))
}

/// A member path given to `pack_entry`: relative, no `..`, not a flag.
fn check_member_path(member: &str) -> Result<(), RegistryError> {
    if member.is_empty()
        || member.starts_with('-')
        || member.starts_with('/')
        || member.split('/').any(|part| part == "..")
    {
        return Err(RegistryError::Protocol(format!(
            "'{member}' is not a relative member path"
        )));
    }
    Ok(())
}

/// The output of `tar <flag> -` over the decompressed archive.
fn list(blob: &Path, flag: &str) -> Result<String, RegistryError> {
    let mut zstd = decompress(blob)?;
    let zstd_stdout = zstd
        .stdout
        .take()
        .ok_or_else(|| RegistryError::Io("zstd gave no output pipe".into()))?;
    let tar = Command::new("tar")
        .arg(flag)
        .arg("-")
        .stdin(Stdio::from(zstd_stdout))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| io("cannot run tar", error))?;
    let zstd_status = zstd.wait().map_err(|error| io("zstd", error))?;
    if !zstd_status.success() || !tar.status.success() {
        return Err(not_an_archive(blob));
    }
    Ok(String::from_utf8_lossy(&tar.stdout).into_owned())
}

/// Extracts the archive into `into`, without taking the archive's owners.
fn extract(blob: &Path, into: &Path) -> Result<(), RegistryError> {
    let mut zstd = decompress(blob)?;
    let zstd_stdout = zstd
        .stdout
        .take()
        .ok_or_else(|| RegistryError::Io("zstd gave no output pipe".into()))?;
    let tar = Command::new("tar")
        .args(["-x", "--no-same-owner", "-f", "-", "-C"])
        .arg(into)
        .stdin(Stdio::from(zstd_stdout))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| io("cannot run tar", error))?;
    let zstd_status = zstd.wait().map_err(|error| io("zstd", error))?;
    if !zstd_status.success() || !tar.success() {
        return Err(not_an_archive(blob));
    }
    Ok(())
}

fn decompress(blob: &Path) -> Result<std::process::Child, RegistryError> {
    Command::new("zstd")
        .arg("-dc")
        .arg(blob)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io("cannot run zstd", error))
}

fn not_an_archive(blob: &Path) -> RegistryError {
    RegistryError::Protocol(format!(
        "{} is not a readable entry archive",
        blob.display()
    ))
}

fn io(context: &str, error: std::io::Error) -> RegistryError {
    RegistryError::Io(format!("{context}: {error}"))
}
