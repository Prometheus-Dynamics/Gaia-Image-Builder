//! Content-based output signatures for the image build and image assembly.
//!
//! The image provider rewrites `<collect_dir>/.gaia-image-state.txt` on every
//! run, and that file records mtime digests and absolute paths. Assembly
//! state is similarly path- and time-bearing. These signatures therefore hash
//! the files the operations produce, never the state files, so identical
//! outputs keep identical signatures and dependents are reused.

use crate::reuse::{content_state_signature, resolve_workspace_path};
use gaia_spec::{AssemblyPathTemplate, AssemblyRoots, ResolvedBuildSpec};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// Provider bookkeeping in a collect dir that changes on every run and does
/// not describe the collected images.
const VOLATILE_COLLECT_FILE: &str = ".gaia-image-state.txt";
/// Suffix of temporary files the providers write before renaming them into
/// place; a leftover one is not an output.
const TEMP_FILE_SUFFIX: &str = ".gaia.tmp";

/// Bounds the digest cache; clearing it only costs re-hashing.
const MAX_CACHED_DIGESTS: usize = 4096;

/// A file's identity for the digest cache: path, size and modification time.
type DigestKey = (PathBuf, u64, u128);

static DIGEST_CACHE: OnceLock<Mutex<HashMap<DigestKey, String>>> = OnceLock::new();

/// Content signature of the image build: the digests of every file in the
/// collect dir (collected images, the provider marker, and any archive
/// kept there), plus the archive digest. `None` without a collect dir.
pub(crate) fn image_build_content_signature(spec: &ResolvedBuildSpec) -> Option<String> {
    let collect_dir = resolve_workspace_path(spec, spec.image.output.collect_dir.as_deref()?);
    let mut parts = vec![format!("collect:{}", collect_dir_digest(&collect_dir))];
    if let Some(archive_name) = spec.image.output.archive_name.as_deref() {
        // Also hashed by the walk when it lives in the collect dir; the
        // repeat is harmless and keeps this correct for archives elsewhere.
        parts.push(format!(
            "archive:{}",
            cached_content_signature(&collect_dir.join(archive_name))
        ));
    }
    Some(parts.join("|"))
}

/// Content signature of image assembly: the digests of the files it writes
/// (transform, filesystem, disk and archive outputs), keyed by the spec's
/// output templates so no absolute path or timestamp enters the signature.
pub(crate) fn image_assembly_content_signature(spec: &ResolvedBuildSpec) -> String {
    let Some(assembly) = &spec.image.assembly else {
        return "assembly:none".into();
    };
    let Ok(roots) = AssemblyRoots::new(spec, assembly) else {
        return "assembly:root_resolution_failed".into();
    };
    let outputs = assembly
        .transforms
        .iter()
        .map(|transform| ("transform", &transform.dest))
        .chain(
            assembly
                .filesystems
                .iter()
                .map(|filesystem| ("filesystem", &filesystem.output)),
        )
        .chain(assembly.disks.iter().map(|disk| ("disk", &disk.output)))
        .chain(
            assembly
                .archives
                .iter()
                .map(|archive| ("archive", &archive.output)),
        );
    outputs
        .map(|(kind, template)| output_part(spec, &roots, kind, template))
        .collect::<Vec<_>>()
        .join("|")
}

fn output_part(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    kind: &str,
    template: &AssemblyPathTemplate,
) -> String {
    match roots.resolve_path(spec, template) {
        Ok(path) => format!(
            "{kind}:{}={}",
            template.as_str(),
            cached_content_signature(&path)
        ),
        Err(_) => format!("{kind}:{}=unresolved", template.as_str()),
    }
}

/// Digest of a collect dir's files, keyed by their relative paths. Symlinked
/// files are followed; symlinked directories are not, so a link cycle cannot
/// stall the walk.
fn collect_dir_digest(dir: &Path) -> String {
    if !dir.is_dir() {
        return "missing".into();
    }
    let mut lines = Vec::new();
    collect_file_lines(dir, dir, &mut lines);
    lines.sort();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex(hasher.finalize().as_slice()))
}

fn collect_file_lines(root: &Path, dir: &Path, lines: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == VOLATILE_COLLECT_FILE || name.ends_with(TEMP_FILE_SUFFIX) {
            continue;
        }
        let is_real_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if is_real_dir {
            collect_file_lines(root, &path, lines);
        } else if fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            lines.push(format!("{relative}\t{}", cached_content_signature(&path)));
        }
    }
}

/// `content_state_signature`, memoized by (path, size, mtime) so the several
/// signature computations within one plan hash each large image once.
fn cached_content_signature(path: &Path) -> String {
    let Ok(metadata) = fs::metadata(path) else {
        return content_state_signature(path);
    };
    let Some(mtime) = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
    else {
        return content_state_signature(path);
    };
    let key = (path.to_path_buf(), metadata.len(), mtime);
    let cache = DIGEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(digest) = cache.lock().ok().and_then(|map| map.get(&key).cloned()) {
        return digest;
    }
    let digest = content_state_signature(path);
    if let Ok(mut map) = cache.lock() {
        if map.len() >= MAX_CACHED_DIGESTS {
            map.clear();
        }
        map.insert(key, digest.clone());
    }
    digest
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
