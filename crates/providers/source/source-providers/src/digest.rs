use super::*;
use gaia_spec::fingerprint::Fingerprint;

/// Compares an already computed digest against the expected one.
pub(crate) fn check_sha256(
    path: &Path,
    expected_sha: &str,
    actual: &str,
) -> Result<(), SourceProviderError> {
    if actual.eq_ignore_ascii_case(expected_sha) {
        return Ok(());
    }
    Err(SourceProviderError::new(
        SourceProviderErrorKind::OutputMissing,
        format!(
            "sha256 mismatch for '{}': expected {}, got {}",
            path.display(),
            expected_sha,
            actual
        ),
    ))
}

/// Hex SHA-256 of a file, hashed in-process (same output as `sha256sum`).
/// Tree digests hash every file, so spawning a process per file was the
/// dominant cost on large source trees.
pub(crate) fn sha256_or_placeholder(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return format!("sha256-error:{}:{error}", path.display()),
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) => return format!("sha256-error:{}:{error}", path.display()),
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Directories that never define a path source's identity: VCS metadata,
/// Gaia's own state, and build or dependency output that builds write into
/// the source tree. Hashing them cost minutes and made every build look like
/// a source change.
pub(crate) const DEFAULT_PATH_SOURCE_IGNORES: &[&str] = &[
    ".git",
    ".gaia",
    "target",
    "node_modules",
    "__pycache__",
    ".gaia-pack",
    ".gaia-wheelhouse",
];

pub(crate) fn tree_digest(path: &Path, ignored_names: &[&str]) -> String {
    let mut fingerprint = Fingerprint::new("source-tree");
    hash_tree(path, ignored_names, &mut fingerprint);
    fingerprint.finish().short_hex()
}

/// Content identity of a materialized git checkout, recorded as
/// `materialized_tree_digest`. A clean checkout is identified by its tree id
/// (`git-tree:<id>`): the tree names every file's content and mode, and a
/// submodule's commit, so no file has to be read. A checkout with local
/// changes, or one git cannot read, is hashed file by file as before.
pub(crate) fn checkout_content_digest(dir: &Path) -> String {
    match clean_checkout_tree(dir) {
        Some(tree) => format!("git-tree:{tree}"),
        None => tree_digest(
            dir,
            &[".git", ".gaia", "source.txt", ".gaia-source-state.txt"],
        ),
    }
}

/// The tree id of `dir`'s HEAD when the checkout has no changes other than
/// Gaia's own markers.
fn clean_checkout_tree(dir: &Path) -> Option<String> {
    let tree = git_command()
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD^{tree}"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|tree| !tree.is_empty())?;
    let status = git_status_porcelain(dir)?;
    status
        .lines()
        .all(|line| {
            line.strip_prefix("?? ")
                .is_some_and(is_checkout_marker_path)
        })
        .then_some(tree)
}

/// Fingerprint of a path source's tree, shared by the planner (reuse) and the
/// provider (materialized state) so both agree on what counts as a change.
///
/// It hashes the set of names and the content of files (plus their mode and
/// length), and the target of symlinks. Directory timestamps and sizes are
/// never hashed, and Gaia's own state is skipped: `.gaia`, `.gaia-trash`,
/// `.gaia-run*` run files, and the workspace build and out directories, even
/// when they sit inside the source root. Creating them after a run therefore
/// does not change the source's fingerprint.
pub fn path_source_fingerprint(
    workspace: &WorkspaceSpec,
    root: &Path,
    identity_ignore: &[String],
) -> String {
    let owned = [&workspace.build_dir, &workspace.out_dir]
        .into_iter()
        .map(|value| {
            let resolved = gaia_spec::resolve_workspace_path(workspace, value)
                .unwrap_or_else(|_| PathBuf::from(&workspace.root_dir).join(value));
            normalize_existing_path(&resolved)
        })
        .collect::<Vec<_>>();
    let ignores = PathSourceIgnores {
        names: identity_ignore
            .iter()
            .cloned()
            .chain(
                DEFAULT_PATH_SOURCE_IGNORES
                    .iter()
                    .map(|name| name.to_string()),
            )
            .collect(),
        owned,
    };
    let root = normalize_existing_path(root);
    let mut fingerprint = Fingerprint::new("path-source");
    fingerprint.str(&root.display().to_string());
    match git_listed_files(&root) {
        // Inside a git work tree, git already knows which files matter: the
        // tracked and untracked-but-not-ignored ones. Ignored build output
        // (target/, node_modules/, ...) is never read.
        Some(files) => hash_git_listed_files(&root, &files, &ignores, &mut fingerprint),
        None => hash_path_source_entry(&root, true, &ignores, &mut fingerprint),
    }
    fingerprint.finish().short_hex()
}

/// Relative paths of the files git lists under `root`: tracked files plus
/// untracked files that are not ignored, sorted. `None` when `root` is not
/// inside a git work tree or git fails, so the caller walks the tree.
fn git_listed_files(root: &Path) -> Option<Vec<PathBuf>> {
    use std::process::Stdio;

    let inside = crate::git_helpers::git_command()
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return None;
    }
    let listed = crate::git_helpers::git_command()
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !listed.status.success() {
        return None;
    }
    let mut files = listed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(path_from_git_bytes)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Some(files)
}

fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Hashes the git-listed files with the same rules as the walk: an entry is
/// skipped when any of its path components is ignored, and a listed path that
/// is missing from the work tree (deleted but not staged) is skipped, as the
/// walk never sees it. A listed directory (a submodule or nested repository)
/// is hashed as the walk hashes a directory.
fn hash_git_listed_files(
    root: &Path,
    files: &[PathBuf],
    ignores: &PathSourceIgnores,
    hasher: &mut Fingerprint,
) {
    for relative in files {
        if ignores.skips_relative(root, relative) {
            continue;
        }
        let path = root.join(relative);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        // The relative path is hashed so a rename changes the digest.
        hasher.str(&relative.display().to_string());
        if metadata.is_dir() {
            hash_path_source_entry(&path, true, ignores, hasher);
        } else {
            hash_path_source_leaf(&path, &metadata, hasher);
        }
    }
}

struct PathSourceIgnores {
    names: Vec<String>,
    owned: Vec<PathBuf>,
}

impl PathSourceIgnores {
    fn skips(&self, path: &Path) -> bool {
        if self.owned.iter().any(|owned| owned == path) {
            return true;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        self.names.iter().any(|ignored| ignored == name) || is_gaia_owned_name(name)
    }

    /// Whether `root.join(relative)` is skipped, checking every component so
    /// an ignored directory prunes everything below it, as the walk does.
    fn skips_relative(&self, root: &Path, relative: &Path) -> bool {
        let mut current = root.to_path_buf();
        for component in relative.components() {
            current.push(component);
            if self.skips(&current) {
                return true;
            }
        }
        false
    }
}

fn is_gaia_owned_name(name: &str) -> bool {
    name == ".gaia"
        || name == ".gaia-trash"
        || name == ".gaia-run-interrupted"
        || name.starts_with(".gaia-run.")
}

fn hash_path_source_entry(
    path: &Path,
    is_root: bool,
    ignores: &PathSourceIgnores,
    hasher: &mut Fingerprint,
) {
    if !is_root {
        if ignores.skips(path) {
            return;
        }
        hasher.str(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        );
    }
    // The root may be a symlink to the tree, so it is followed; entries are not.
    let metadata = match if is_root {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    } {
        Ok(metadata) => metadata,
        Err(_) => {
            hasher.str("missing");
            return;
        }
    };
    if metadata.is_dir() {
        hasher.str("dir");
        match fs::read_dir(path) {
            Ok(entries) => {
                let mut entries = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                entries.sort();
                for entry in entries {
                    hash_path_source_entry(&entry, false, ignores, hasher);
                }
            }
            Err(_) => {
                hasher.str("unreadable");
            }
        }
    } else {
        hash_path_source_leaf(path, &metadata, hasher);
    }
}

/// Hashes a non-directory entry: a symlink's target, or a file's length, mode
/// and contents.
fn hash_path_source_leaf(path: &Path, metadata: &fs::Metadata, hasher: &mut Fingerprint) {
    if metadata.file_type().is_symlink() {
        hasher.str("symlink");
        hasher.str(
            &fs::read_link(path)
                .map(|target| target.display().to_string())
                .unwrap_or_default(),
        );
    } else if metadata.is_file() {
        hasher.str("file");
        hasher.u64(metadata.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            hasher.u64(u64::from(metadata.permissions().mode()));
        }
        hasher.str(&sha256_or_placeholder(path));
    } else {
        hasher.str("other");
    }
}

/// Canonicalizes the longest existing ancestor of `path` and re-appends the
/// rest, so a path that does not exist yet compares equal to its later form.
fn normalize_existing_path(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = fs::canonicalize(current) {
            let mut normalized = canonical;
            for part in missing.iter().rev() {
                normalized.push(part);
            }
            return normalized;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub(crate) fn hash_tree(path: &Path, ignored_names: &[&str], hasher: &mut Fingerprint) {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if ignored_names.iter().any(|ignored| ignored == &file_name) {
        return;
    }
    hasher.str(&path.display().to_string());
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            hasher.str("missing");
            return;
        }
    };
    hasher
        .bool(metadata.is_dir())
        .bool(metadata.is_file())
        .bool(metadata.file_type().is_symlink())
        .u64(metadata.len());
    if metadata.is_dir() {
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort();
        for entry in entries {
            hash_tree(&entry, ignored_names, hasher);
        }
    } else if metadata.is_file() {
        hasher.str(&sha256_or_placeholder(path));
    }
}
