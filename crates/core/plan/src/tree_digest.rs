//! The per-file digest map of a directory tree, as a
//! `${assembly.sha256_tree:...}` value renders it, and the content signature
//! a plan fingerprints it by. Content only: no mtimes or absolute paths, so
//! a tree rebuilt with the same bytes keeps the same signature.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

/// Each regular file under `root`: its path relative to `root` with `/`
/// separators, and its sha256 hex, sorted by path. Directories are walked;
/// symlinks are neither followed nor listed.
pub fn tree_digest_members(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut members = Vec::new();
    collect(root, root, &mut members)?;
    members.sort();
    Ok(members)
}

/// A digest of [`tree_digest_members`], for fingerprints: `missing` when
/// `root` is not a directory.
pub(crate) fn tree_digest_signature(root: &Path) -> String {
    if !root.is_dir() {
        return "missing".into();
    }
    match tree_digest_members(root) {
        Ok(members) => {
            let mut hasher = Sha256::new();
            for (path, sha256) in &members {
                hasher.update(format!("{path}\t{sha256}\n").as_bytes());
            }
            let digest = hasher.finalize();
            format!(
                "sha256:{}",
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        }
        Err(error) => format!("unreadable:{error}"),
    }
}

/// The digest a `src_dir` copy is fingerprinted by: every entry under `root`
/// in sorted order, with its kind, its relative path, a regular file's
/// permission bits and sha256, and a link's target. Directories and links
/// are included, so a renamed, added or retargeted entry changes it, as a
/// changed mode does. `missing` when `root` is not a directory.
pub(crate) fn tree_copy_signature(root: &Path) -> String {
    if !root.is_dir() {
        return "missing".into();
    }
    let mut lines = Vec::new();
    if let Err(error) = collect_copy_lines(root, root, &mut lines) {
        return format!("unreadable:{error}");
    }
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn collect_copy_lines(root: &Path, dir: &Path, lines: &mut Vec<String>) -> Result<(), String> {
    let read_error = |error: std::io::Error| {
        format!(
            "failed to read '{}' for a tree digest: {error}",
            dir.display()
        )
    };
    let mut paths = fs::read_dir(dir)
        .map_err(read_error)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    paths.sort();
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "failed to read '{}' for a tree digest: {error}",
                path.display()
            )
        })?;
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if metadata.is_dir() {
            lines.push(format!("dir\t{relative}"));
            collect_copy_lines(root, &path, lines)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|error| {
                format!(
                    "failed to read '{}' for a tree digest: {error}",
                    path.display()
                )
            })?;
            lines.push(format!("link\t{relative}\t{}", target.display()));
        } else if metadata.is_file() {
            lines.push(format!(
                "file\t{relative}\t{:o}\t{}",
                permission_bits(&metadata),
                file_sha256(&path)?
            ));
        } else {
            lines.push(format!("other\t{relative}"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn permission_bits(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permission_bits(_metadata: &fs::Metadata) -> u32 {
    0
}

fn collect(root: &Path, dir: &Path, members: &mut Vec<(String, String)>) -> Result<(), String> {
    let read_error = |error: std::io::Error| {
        format!(
            "failed to read '{}' for a tree digest: {error}",
            dir.display()
        )
    };
    let mut paths = fs::read_dir(dir)
        .map_err(read_error)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    paths.sort();
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "failed to read '{}' for a tree digest: {error}",
                path.display()
            )
        })?;
        if metadata.is_dir() {
            collect(root, &path, members)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            members.push((relative, file_sha256(&path)?));
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let read_error = |error: std::io::Error| {
        format!(
            "failed to read '{}' for a tree digest: {error}",
            path.display()
        )
    };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_relative_sorted_and_skip_symlinks_and_directories() {
        let scratch = gaia_testkit::TempDir::new("gaia-tree-digest");
        let dir = scratch.join("tree");
        fs::create_dir_all(dir.join("b/c")).expect("dirs");
        fs::write(dir.join("b/c/z.txt"), b"z").expect("file");
        fs::write(dir.join("a.txt"), b"a").expect("file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("a.txt"), dir.join("link")).expect("link");
        let members = tree_digest_members(&dir).expect("members");
        let names = members
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["a.txt", "b/c/z.txt"]);
        assert_eq!(
            members[0].1,
            "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
        );
        let signature = tree_digest_signature(&dir);
        fs::write(dir.join("a.txt"), b"a").expect("rewritten with same bytes");
        assert_eq!(tree_digest_signature(&dir), signature);
        assert_eq!(tree_digest_signature(&dir.join("missing")), "missing");
    }

    #[test]
    fn copy_signature_covers_names_modes_links_and_nested_content() {
        let scratch = gaia_testkit::TempDir::new("gaia-tree-copy-signature");
        let dir = scratch.join("tree");
        fs::create_dir_all(dir.join("_app/immutable")).expect("dirs");
        fs::write(dir.join("_app/immutable/a.js"), b"a").expect("file");
        let base = tree_copy_signature(&dir);
        assert!(base.starts_with("sha256:"), "{base}");
        assert_eq!(tree_copy_signature(&dir), base);
        fs::write(dir.join("_app/immutable/a.js"), b"b").expect("changed content");
        let changed = tree_copy_signature(&dir);
        assert_ne!(changed, base, "nested content");
        fs::write(dir.join("_app/immutable/a.js"), b"a").expect("same content again");
        assert_eq!(tree_copy_signature(&dir), base);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("_app/immutable/a.js");
            let mut permissions = fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("mode");
            assert_ne!(tree_copy_signature(&dir), base, "mode");
            std::os::unix::fs::symlink("a.js", dir.join("_app/alias.js")).expect("link");
            assert_ne!(tree_copy_signature(&dir), base, "added link");
        }
        assert_eq!(tree_copy_signature(&dir.join("missing")), "missing");
    }
}
