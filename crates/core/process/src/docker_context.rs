//! SHA-256 of a docker build context, cached per process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directory names skipped when hashing a Docker build context: VCS
/// metadata and build output that never belong in an execution image.
const DOCKER_CONTEXT_IGNORES: &[&str] = &[".git", ".gaia", "target", "node_modules"];

/// Read buffer for hashing file contents in place.
const CONTEXT_READ_BUFFER_BYTES: usize = 64 * 1024;

/// What the context hash can see of one file without reading it: when the
/// hash of a context is still valid, every stamp is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    executable: bool,
    link_target: Option<PathBuf>,
}

/// The stamps of a Dockerfile and of every file of its context, in hash order.
type ContextStamp = (FileStamp, Vec<(PathBuf, FileStamp)>);

/// Last hash computed per (Dockerfile, context), with the stamps it was
/// computed from. A process hashes the same context again and again (once per
/// wrapped command), so an unchanged context is not read again.
type ContextHashCache = std::sync::Mutex<HashMap<(PathBuf, PathBuf), (ContextStamp, String)>>;

static CONTEXT_HASH_CACHE: std::sync::OnceLock<ContextHashCache> = std::sync::OnceLock::new();

/// SHA-256 over a Dockerfile and every file in its build context (relative
/// path, type, executable bit, contents and symlink targets), in sorted
/// order. Stable across machines, so it can name a locally built image.
///
/// Files are read in chunks, and a context whose files have the same lengths,
/// modification times and modes as at the last call returns the hash computed
/// then without reading them again.
pub fn docker_build_context_hash(dockerfile: &Path, context: &Path) -> std::io::Result<String> {
    let dockerfile_metadata = std::fs::metadata(dockerfile)?;
    if !context.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "docker build context '{}' is not a directory",
                context.display()
            ),
        ));
    }
    let files = context_files(context)?;
    let mut entries = Vec::with_capacity(files.len());
    for (path, metadata) in &files {
        let relative = path.strip_prefix(context).unwrap_or(path).to_path_buf();
        entries.push((relative, file_stamp(path, metadata)?));
    }
    let stamp: ContextStamp = (file_stamp(dockerfile, &dockerfile_metadata)?, entries);
    let key = (dockerfile.to_path_buf(), context.to_path_buf());
    let cache = CONTEXT_HASH_CACHE.get_or_init(Default::default);
    let cached = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .filter(|(cached_stamp, _)| *cached_stamp == stamp)
        .map(|(_, hash)| hash.clone());
    if let Some(hash) = cached {
        return Ok(hash);
    }
    let hash = hash_build_context(dockerfile, context, &files)?;
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, (stamp, hash.clone()));
    Ok(hash)
}

/// Every file of a context that the hash covers, sorted by path.
fn context_files(context: &Path) -> std::io::Result<Vec<(PathBuf, std::fs::Metadata)>> {
    let mut stack = vec![context.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if DOCKER_CONTEXT_IGNORES.contains(&name) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
            } else {
                files.push((path, metadata));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn file_stamp(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<FileStamp> {
    let link_target = if metadata.file_type().is_symlink() {
        Some(std::fs::read_link(path)?)
    } else {
        None
    };
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        executable,
        link_target,
    })
}

/// Hashes the Dockerfile and the sorted context files, streaming each file's
/// contents. The byte stream is the one earlier releases hashed, so image tags
/// built from it do not change.
fn hash_build_context(
    dockerfile: &Path,
    context: &Path,
    files: &[(PathBuf, std::fs::Metadata)],
) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"dockerfile\0");
    hash_file_contents(&mut hasher, dockerfile)?;
    for (path, metadata) in files {
        let relative = path.strip_prefix(context).unwrap_or(path);
        hasher.update(b"\0entry\0");
        hasher.update(relative.to_string_lossy().as_bytes());
        if metadata.file_type().is_symlink() {
            hasher.update(b"\0symlink\0");
            hasher.update(std::fs::read_link(path)?.to_string_lossy().as_bytes());
            continue;
        }
        let executable = file_stamp(path, metadata)?.executable;
        hasher.update(if executable { b"\0x\0" } else { b"\0f\0" });
        hash_file_contents(&mut hasher, path)?;
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hash_file_contents(hasher: &mut sha2::Sha256, path: &Path) -> std::io::Result<()> {
    use sha2::Digest;
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0_u8; CONTEXT_READ_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, bytes).expect("write");
    }

    #[test]
    fn context_hash_is_the_byte_stream_earlier_releases_hashed() {
        let scratch = TempDir::new("docker-context");
        let dockerfile = scratch.join("Dockerfile");
        let context = scratch.join("ctx");
        write(&dockerfile, b"FROM scratch\n");
        write(&context.join("a.txt"), b"a");
        // Larger than one read buffer, so the contents are hashed in chunks.
        let big = vec![7_u8; 3 * CONTEXT_READ_BUFFER_BYTES + 5];
        write(&context.join("sub/b.txt"), &big);

        let mut expected = Sha256::new();
        expected.update(b"dockerfile\0");
        expected.update(b"FROM scratch\n");
        expected.update(b"\0entry\0a.txt\0f\0a");
        expected.update(b"\0entry\0sub/b.txt\0f\0");
        expected.update(&big);
        let expected: String = expected
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(
            docker_build_context_hash(&dockerfile, &context).expect("hash"),
            expected
        );
    }

    #[test]
    fn an_edit_of_the_same_size_is_hashed_again_and_a_repeat_call_is_stable() {
        let scratch = TempDir::new("docker-context");
        let dockerfile = scratch.join("Dockerfile");
        let context = scratch.join("ctx");
        write(&dockerfile, b"FROM scratch\n");
        let file = context.join("a.txt");
        write(&file, b"aaaa");

        let first = docker_build_context_hash(&dockerfile, &context).expect("hash");
        assert_eq!(
            docker_build_context_hash(&dockerfile, &context).expect("hash"),
            first,
            "an unchanged context keeps its hash"
        );

        // Same length; only the modification time tells the edit apart.
        fs::write(&file, b"bbbb").expect("edit");
        let edited = fs::File::options().write(true).open(&file).expect("open");
        edited
            .set_modified(SystemTime::now() + Duration::from_secs(3600))
            .expect("mtime");
        let second = docker_build_context_hash(&dockerfile, &context).expect("hash");
        assert_ne!(second, first);

        // A copy of the Dockerfile is a new cache key, so it is hashed from scratch.
        let copy = scratch.join("Dockerfile.copy");
        write(&copy, b"FROM scratch\n");
        assert_eq!(
            docker_build_context_hash(&copy, &context).expect("hash"),
            second
        );
    }
}
