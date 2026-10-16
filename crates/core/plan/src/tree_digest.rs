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
pub fn tree_digest_signature(root: &Path) -> String {
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
}
