//! The canonical RAM tree (`[execution] canonical_tree = true`): a RAM
//! Buildroot tree at a path named by the build's identity instead of by the
//! checkout. Checkouts of one build then share the tree path, so a pinned
//! package-cache entry (a package whose binaries hold the tree's path, see
//! `package_cache.rs`) built in one checkout restores into another.
//!
//! The path is `<ram base>/gaia/<hash>/buildroot-output`, with `<hash>` the
//! first 12 hex digits of `sha256(material)`. The material is, one item per
//! line:
//! - `gaia-canonical-tree-v1`, the version of this formula;
//! - `build=<id>`, the build's id (`spec.identity.id`);
//! - the Buildroot config identity of the shared output trees
//!   ([`shared_output_key_material`]: the source identity, defconfig and
//!   fragment contents, config overrides, external tree, package overrides,
//!   ccache and parallel-packages settings, Docker image), with the workspace
//!   root replaced by `<workspace>`, so the same build in two checkouts at
//!   two paths gives the same material.
//!
//! The identity only chooses where the tree is. The tree's contents are
//! still checked as they are today: config steps re-run when their inputs'
//! contents change, and packages are rebuilt when their keys change.
//!
//! Two live builds never share a canonical tree. The claim is an owner record
//! `owner.json` in `<hash>`, written under an exclusive `flock` of that dir.
//! A record is live when its pid still runs with the start time it recorded
//! (`/proc/<pid>/stat`, field 22), which tells a reused pid apart. A live
//! record of another run makes the build fall back to its per-checkout tree
//! ([`super::ram_tree_dir::tree_dir`]) with a message. A stale record (dead
//! pid, reused pid, unreadable file) is taken over. The record is removed
//! when the build's [`WorkDir`](super::ram_tree::WorkDir) is dropped, and the
//! `<hash>` dir with it when empty, so a kept tree keeps its dir.
//!
//! Only a RAM tree is canonical. A disk tree lives in the build dir, and
//! moving it under a shared path would discard it and change what Docker
//! builds mount; `work_dir = "disk"` (and a RAM build that falls back to
//! disk) keep the per-checkout placement.
use super::*;
use crate::ram_tree_dir::create_private_tree_parent;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::io::AsRawFd;

/// The directory under the RAM base that holds the canonical trees.
const CANONICAL_DIR: &str = "gaia";
/// Bumped when the material or the path formula changes.
const CANONICAL_VERSION: &str = "gaia-canonical-tree-v1";
/// The owner record in a canonical `<hash>` dir.
const OWNER_FILE: &str = "owner.json";

/// What a build asks of the canonical tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalRequest {
    /// The build's id (`spec.identity.id`).
    pub(crate) build_id: String,
    /// The Buildroot config identity, with the workspace root as
    /// `<workspace>`.
    pub(crate) identity: String,
}

impl CanonicalRequest {
    /// The request for a Buildroot image build of `spec`.
    pub(crate) fn new(
        spec: &ResolvedBuildSpec,
        image: &ImageSpec,
        buildroot_dir: &Path,
        policy: &ImageExecutionPolicy,
        execution: &ImageExecutionContext,
    ) -> Result<Self, ImageProviderError> {
        let identity = shared_output_key_material(spec, image, buildroot_dir, policy, execution)?;
        Ok(Self {
            build_id: spec.identity.id.to_string(),
            identity: workspace_neutral(identity, &spec.workspace.root_dir),
        })
    }

    /// The request of a build with `[execution] canonical_tree` on; `None`
    /// when it is off.
    pub(crate) fn for_build(
        spec: &ResolvedBuildSpec,
        image: &ImageSpec,
        buildroot_dir: &Path,
        policy: &ImageExecutionPolicy,
        execution: &ImageExecutionContext,
    ) -> Result<Option<Self>, ImageProviderError> {
        policy
            .canonical_tree
            .then(|| Self::new(spec, image, buildroot_dir, policy, execution))
            .transpose()
    }

    /// What the path is a digest of.
    fn material(&self) -> String {
        format!(
            "{CANONICAL_VERSION}\nbuild={}\n{}",
            self.build_id, self.identity
        )
    }

    /// The `<hash>` dir name: 12 hex digits of the material's sha256.
    fn hash_name(&self) -> String {
        let digest = Sha256::digest(self.material().as_bytes());
        hex(&digest[..6])
    }
}

/// `identity` with the workspace root written as `<workspace>`: the external
/// tree and patch dir values hold absolute paths, which differ between
/// checkouts of one build.
fn workspace_neutral(identity: String, root: &str) -> String {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        identity
    } else {
        identity.replace(root, "<workspace>")
    }
}

/// The canonical `<hash>` dir under `base` (the RAM base).
pub(crate) fn canonical_hash_dir(base: &Path, request: &CanonicalRequest) -> PathBuf {
    base.join(CANONICAL_DIR).join(request.hash_name())
}

/// `<base>/gaia/<hash>/buildroot-output`: the canonical tree under `base`.
pub(crate) fn canonical_tree_dir(base: &Path, request: &CanonicalRequest) -> PathBuf {
    canonical_hash_dir(base, request).join("buildroot-output")
}

/// The owner record of a canonical tree: which run holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    pid: u32,
    /// Field 22 of `/proc/<pid>/stat`: the process start time in clock ticks.
    start: u64,
    /// The checkout's per-checkout output dir (`<build>/image/buildroot-output`).
    output_dir: String,
    build_id: String,
    /// Unix seconds when the record was written.
    claimed: u64,
}

impl Owner {
    /// The record of this process for a build of `output_dir`, or `None`
    /// when its start time cannot be read (then no claim is made).
    fn this_run(output_dir: &Path, build_id: &str) -> Option<Self> {
        let pid = std::process::id();
        Some(Self {
            pid,
            start: process_start(pid)?,
            output_dir: output_dir.display().to_string(),
            build_id: build_id.to_string(),
            claimed: unix_now(),
        })
    }

    /// The same run and checkout (the pid and start time name the process).
    fn same_run(&self, other: &Self) -> bool {
        self.pid == other.pid && self.start == other.start && self.output_dir == other.output_dir
    }

    /// Whether the process that wrote this record still runs as it did.
    fn is_live(&self) -> bool {
        process_start(self.pid) == Some(self.start)
    }

    fn describe(&self) -> String {
        format!(
            "build '{}' of '{}' (pid {})",
            self.build_id, self.output_dir, self.pid
        )
    }

    fn to_json(&self) -> String {
        serde_json::json!({
            "pid": self.pid,
            "start": self.start,
            "output_dir": self.output_dir,
            "build_id": self.build_id,
            "claimed": self.claimed,
        })
        .to_string()
    }

    fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Self {
            pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
            start: value.get("start")?.as_u64()?,
            output_dir: value.get("output_dir")?.as_str()?.to_string(),
            build_id: value.get("build_id")?.as_str()?.to_string(),
            claimed: value.get("claimed").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }
}

/// The process start time (clock ticks since boot), from `/proc/<pid>/stat`.
fn process_start(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces: the fields
    // follow its last ')'. The state is field 3, so starttime (field 22) is
    // the 20th field after it.
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse().ok()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn read_owner(hash_dir: &Path) -> Option<Owner> {
    Owner::parse(&fs::read_to_string(hash_dir.join(OWNER_FILE)).ok()?)
}

/// Claims a canonical tree for the run.
pub(crate) enum Claim {
    /// The tree is this run's until the claim drops.
    Owned(TreeClaim),
    /// Another live run holds it: who, as text.
    Busy(String),
}

/// The claim on a canonical tree; dropping it releases the tree.
pub(crate) struct TreeClaim {
    hash_dir: PathBuf,
    owner: Owner,
}

impl Drop for TreeClaim {
    fn drop(&mut self) {
        let path = self.hash_dir.join(OWNER_FILE);
        if read_owner(&self.hash_dir).is_some_and(|current| current.same_run(&self.owner)) {
            let _ = fs::remove_file(&path);
        }
        // Only an empty dir goes: a kept tree keeps its dir.
        let _ = fs::remove_dir(&self.hash_dir);
    }
}

/// Claims the canonical tree of `request` under `base` for a build of
/// `output_dir`. The `<base>/gaia` parent is checked as the per-user RAM dir
/// is (see [`create_private_tree_parent`]): a directory of ours, not a
/// symlink, mode 0700.
pub(crate) fn claim(
    base: &Path,
    request: &CanonicalRequest,
    output_dir: &Path,
) -> Result<Claim, ImageProviderError> {
    let tree = canonical_tree_dir(base, request);
    create_private_tree_parent(&tree)?;
    let hash_dir = canonical_hash_dir(base, request);
    match fs::DirBuilder::new().mode(0o700).create(&hash_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(ImageProviderError::backend_command(format!(
                "failed to create '{}': {error}",
                hash_dir.display()
            )));
        }
    }
    let Some(me) = Owner::this_run(output_dir, &request.build_id) else {
        return Ok(Claim::Busy(
            "a run whose start time cannot be read".to_string(),
        ));
    };
    // The exclusive lock of the dir serializes concurrent claims. It is
    // released when the file closes, at the end of this function.
    let lock = File::open(&hash_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to open '{}': {error}",
            hash_dir.display()
        ))
    })?;
    // SAFETY: flock takes a valid, open descriptor and an operation.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(ImageProviderError::backend_command(format!(
            "failed to lock '{}': {}",
            hash_dir.display(),
            std::io::Error::last_os_error()
        )));
    }
    if let Some(current) = read_owner(&hash_dir)
        && !current.same_run(&me)
        && current.is_live()
    {
        return Ok(Claim::Busy(current.describe()));
    }
    // Written beside and renamed over the record, so a reader never sees a
    // partial one.
    let temporary = hash_dir.join(format!("{OWNER_FILE}.{}.tmp", me.pid));
    fs::write(&temporary, me.to_json())
        .and_then(|()| fs::rename(&temporary, hash_dir.join(OWNER_FILE)))
        .map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to record the owner of '{}': {error}",
                hash_dir.display()
            ))
        })?;
    Ok(Claim::Owned(TreeClaim {
        hash_dir,
        owner: me,
    }))
}

/// Without claiming: the holder's description when another live run owns the
/// canonical tree of `request` under `base` (for previews).
pub(crate) fn busy_holder(
    base: &Path,
    request: &CanonicalRequest,
    output_dir: &Path,
) -> Option<String> {
    let current = read_owner(&canonical_hash_dir(base, request))?;
    let me = Owner::this_run(output_dir, &request.build_id)?;
    (!current.same_run(&me) && current.is_live()).then(|| current.describe())
}

#[cfg(test)]
#[path = "canonical_tree_tests.rs"]
mod tests;
