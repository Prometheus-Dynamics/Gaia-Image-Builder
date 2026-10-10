//! Where the Buildroot output tree is built (`[providers.buildroot]
//! work_dir`): in the build dir ("disk", the default), in RAM ("ram": tmpfs
//! under /dev/shm), or in another directory.
//!
//! Most of a Buildroot build's time on a disk goes to file operations, not
//! compiling: extracting sources, linking and copying per-package
//! directories, installing, finalizing, cleaning. On tmpfs those cost almost
//! nothing. The tree lives at `<base>/gaia-<user>/<hash of the build's output
//! path>/buildroot-output` and `<build_dir>/image/buildroot-output` becomes a
//! symlink to it, so everything reading the tree through the usual path
//! keeps working; Buildroot itself is given the real path, which is also
//! what Docker builds mount.
//!
//! What must survive stays on disk: collected images and archives, the
//! package cache, ccache, downloads and Gaia's state. A RAM tree is kept
//! after a build for fast rebuilds (unless `keep_ram_tree = false`); after a
//! reboot it is gone and the next build starts a fresh tree, restoring
//! packages from the package cache.
//!
//! A RAM tree is only used when the RAM it needs (the size of the last such
//! tree, or 30 GiB) fits in the budget and in available memory with a
//! margin; otherwise the build runs on disk. During the build a watchdog
//! stops it cleanly when available memory or tmpfs space runs low.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const GIB: u64 = 1024 * 1024 * 1024;
/// RAM assumed for a tree never built in RAM before.
pub(crate) const DEFAULT_RAM_NEED: u64 = 30 * GIB;
pub(crate) const DEFAULT_RAM_BUDGET: &str = "60G";
/// Available memory left to everything else when a RAM build starts.
pub(crate) const RAM_MARGIN: u64 = 8 * GIB;
/// The watchdog stops the build below this much available memory or tmpfs
/// space.
const RAM_LOW: u64 = 4 * GIB;
/// Size of the last RAM tree, next to the output path on disk.
const RAM_SIZE_FILE: &str = ".gaia-ram-tree-size";
pub(crate) const RAM_BASE: &str = "/dev/shm";

/// Where this build's tree is.
pub(crate) struct WorkDir {
    /// The real tree directory, to give Buildroot.
    pub(crate) dir: PathBuf,
    /// Set for a RAM tree.
    pub(crate) ram: bool,
    pub(crate) messages: Vec<String>,
}

/// Places the output tree for this build. `output_dir` is the usual
/// `<build_dir>/image/buildroot-output`.
pub(crate) fn place_work_dir(
    output_dir: &Path,
    policy: &ImageExecutionPolicy,
) -> Result<WorkDir, ImageProviderError> {
    let facts = work_dir_facts(output_dir, policy)?;
    let decision = decide_work_dir(&facts);
    let messages = work_dir_messages(&decision, output_dir);
    let error = |action: &str, path: &Path, error: std::io::Error| {
        ImageProviderError::backend_command(format!(
            "failed to {action} '{}': {error}",
            path.display()
        ))
    };
    match decision {
        WorkDirDecision::Disk { dropped } | WorkDirDecision::RamFallback { dropped, .. } => {
            // The usual tree in the build dir. A link left by an earlier RAM
            // or work dir build goes together with its tree.
            if let Some(target) = dropped {
                fs::remove_file(output_dir).map_err(|e| error("remove", output_dir, e))?;
                let _ = gaia_process::discard(&target);
            }
            Ok(WorkDir {
                dir: output_dir.to_path_buf(),
                ram: false,
                messages,
            })
        }
        WorkDirDecision::Tree(placement) => {
            let dir = placement.dir.clone();
            if placement.replaced_link {
                fs::remove_file(output_dir).map_err(|e| error("remove", output_dir, e))?;
            }
            if placement.moved_from_disk {
                // A tree built on disk: start over in the work dir, restoring
                // packages from the package cache.
                keep_package_durations(output_dir, output_dir.parent());
                gaia_process::discard(output_dir).map_err(|e| error("remove", output_dir, e))?;
            }
            if placement.relink
                && let Some(parent) = output_dir.parent()
            {
                fs::create_dir_all(parent).map_err(|e| error("create", parent, e))?;
            }
            if placement.create {
                fs::create_dir_all(&dir).map_err(|e| error("create", &dir, e))?;
            }
            if placement.relink {
                std::os::unix::fs::symlink(&dir, output_dir)
                    .map_err(|e| error("link", output_dir, e))?;
            }
            // Package times for the progress estimate survive a lost tree.
            if let Some(parent) = output_dir.parent()
                && !dir.join(PACKAGE_DURATIONS).exists()
            {
                let _ = fs::copy(parent.join(PACKAGE_DURATIONS), dir.join(PACKAGE_DURATIONS));
            }
            // Containers reach the tree through the link in the build dir.
            if let Some(parent) = dir.parent() {
                gaia_process::register_docker_mount(parent);
            }
            Ok(WorkDir {
                dir,
                ram: placement.ram,
                messages,
            })
        }
    }
}

/// `<base>/gaia-<user>/<hash of output_dir>/buildroot-output`.
pub(crate) fn tree_dir(base: &Path, output_dir: &Path) -> PathBuf {
    let user = std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty() && !user.contains('/'))
        .unwrap_or_else(|| {
            use std::os::unix::fs::MetadataExt;
            fs::metadata("/proc/self")
                .map(|metadata| metadata.uid().to_string())
                .unwrap_or_else(|_| "user".to_string())
        });
    let digest = Sha256::digest(output_dir.to_string_lossy().as_bytes());
    base.join(format!("gaia-{user}"))
        .join(hex(&digest[..6]))
        .join("buildroot-output")
}

/// Written in a mirror after its copy succeeds: the identity of the source it
/// was copied from. A later mirror with the same identity is skipped.
const MIRROR_SOURCE_MARKER: &str = ".gaia-mirror-source";

/// Keys of the source provider's state (`.gaia-source-state.txt`) that name
/// the source's content. Per-build fields (build version, profile, ...) are
/// left out.
const SOURCE_CONTENT_KEYS: &[&str] = &[
    "resolved_commit_sha",
    "materialized_head_commit",
    "materialized_tree_digest",
    "extracted_tree_digest",
    "archive_sha256",
];

/// The content keys that are a digest of the whole tree. A source with none
/// of them recorded has no trusted identity (a path source is a live
/// reference, its recorded fingerprint says nothing about later edits).
const SOURCE_TREE_DIGEST_KEYS: &[&str] = &[
    "materialized_tree_digest",
    "extracted_tree_digest",
    "archive_sha256",
];

/// Entries of the source the mirror does not copy: the source provider's
/// own files, and the download and output directories `rsync` excludes.
const MIRROR_SKIPPED: &[&str] = &[
    ".git",
    ".gaia",
    "source.txt",
    ".gaia-source-state.txt",
    "dl",
    "output",
];

/// Identity of a Buildroot source for its mirror: the content the source
/// provider recorded, and the metadata of the source directory and its
/// top-level entries, which changes when the source is materialized again.
/// Taken before anything is copied, so a source that changes during a copy
/// differs from the recorded identity next time.
struct SourceIdentity {
    /// Stored in the mirror's marker; compared as text.
    text: String,
    /// Top-level entries the mirror must hold.
    names: Vec<std::ffi::OsString>,
}

fn source_identity(buildroot_dir: &Path) -> Option<SourceIdentity> {
    use std::os::unix::fs::MetadataExt;
    let state = fs::read_to_string(buildroot_dir.join(".gaia-source-state.txt")).ok()?;
    let state = gaia_spec::KeyValueState::parse(&state).into_map();
    let recorded = |key: &str| state.get(key).filter(|value| !value.trim().is_empty());
    if !SOURCE_TREE_DIGEST_KEYS
        .iter()
        .any(|key| recorded(key).is_some())
    {
        return None;
    }
    let stamp = |metadata: &fs::Metadata| {
        format!(
            "{}:{}:{}.{}:{}.{}",
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    };
    let mut text = String::new();
    for key in SOURCE_CONTENT_KEYS {
        if let Some(value) = recorded(key) {
            text.push_str(&format!("{key}={value}\n"));
        }
    }
    text.push_str(&format!(
        "dir={}\n",
        stamp(&fs::symlink_metadata(buildroot_dir).ok()?)
    ));
    let mut entries = Vec::new();
    for entry in fs::read_dir(buildroot_dir).ok()?.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| MIRROR_SKIPPED.contains(&name))
        {
            continue;
        }
        let metadata = entry.metadata().ok()?;
        entries.push((name, stamp(&metadata)));
    }
    entries.sort();
    for (name, entry_stamp) in &entries {
        text.push_str(&format!("entry={}:{entry_stamp}\n", name.to_string_lossy()));
    }
    Some(SourceIdentity {
        text,
        names: entries.into_iter().map(|(name, _)| name).collect(),
    })
}

/// Whether a mirror holds exactly the source with this identity: its marker
/// matches, and every top-level entry of the source is in it.
fn mirror_is_current(mirror: &Path, identity: &SourceIdentity) -> bool {
    fs::read_to_string(mirror.join(MIRROR_SOURCE_MARKER))
        .is_ok_and(|marker| marker == identity.text)
        && identity
            .names
            .iter()
            .all(|name| fs::symlink_metadata(mirror.join(name)).is_ok())
}

/// For a RAM tree, a mirror of the Buildroot source next to it: `make`
/// parses every package's `.mk` on each run, which on a busy disk can stall
/// a build for minutes. Kept in step with `rsync`; the source itself when
/// the mirror cannot be made.
///
/// A mirror whose source has the same recorded identity (see
/// [`source_identity`]) is not copied again: `rsync` would stat every file of
/// the source, which on a slow disk takes far longer than the build step it
/// serves. The patches Gaia applies to the mirror afterwards are idempotent;
/// when this run applies none (`parallel_packages` off), the mirror's Gaia
/// patches are reverted, as a copy would have them.
pub(crate) fn buildroot_source_for(
    buildroot_dir: &Path,
    work: &WorkDir,
    parallel_packages: bool,
) -> (PathBuf, Vec<String>) {
    let Some(mirror) = work
        .ram
        .then(|| {
            work.dir
                .parent()
                .map(|parent| parent.join("buildroot-source"))
        })
        .flatten()
    else {
        return (buildroot_dir.to_path_buf(), Vec::new());
    };
    let identity = source_identity(buildroot_dir);
    if let Some(identity) = &identity
        && mirror_is_current(&mirror, identity)
        && (parallel_packages || revert_gaia_patches(&mirror).is_ok())
    {
        return (
            mirror,
            vec!["buildroot source mirror: unchanged".to_string()],
        );
    }
    // The marker goes first: a copy that is interrupted leaves no marker.
    let _ = fs::remove_file(mirror.join(MIRROR_SOURCE_MARKER));
    let synced = fs::create_dir_all(&mirror).is_ok()
        && Command::new("rsync")
            .args([
                "-a",
                "--delete",
                "--exclude=/dl/",
                "--exclude=/.git/",
                "--exclude=/output/",
            ])
            .arg(format!("{}/", buildroot_dir.display()))
            .arg(format!("{}/", mirror.display()))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    if synced {
        if let Some(identity) = &identity {
            let _ = fs::write(mirror.join(MIRROR_SOURCE_MARKER), &identity.text);
        }
        (mirror, Vec::new())
    } else {
        (
            buildroot_dir.to_path_buf(),
            vec![format!(
                "could not mirror the Buildroot source into '{}'; reading it from '{}'",
                mirror.display(),
                buildroot_dir.display()
            )],
        )
    }
}

/// After a successful build of a RAM tree: records its size for the next
/// check and drops it unless it is kept.
pub(crate) fn finish_ram_tree(output_dir: &Path, work: &WorkDir, keep: bool) -> Vec<String> {
    if !work.ram {
        return Vec::new();
    }
    let clock = gaia_process::ActiveClock::start();
    let size = tree_size(&work.dir);
    let mut messages = vec![gaia_process::step_time_message(
        "ram tree size",
        clock.elapsed(),
    )];
    let clock = gaia_process::ActiveClock::start();
    keep_package_durations(&work.dir, output_dir.parent());
    if let Some(parent) = output_dir.parent() {
        let _ = fs::write(parent.join(RAM_SIZE_FILE), size.to_string());
    }
    messages.push(format!("RAM tree size: {}", gib(size)));
    if !keep {
        let _ = fs::remove_file(output_dir);
        let _ = gaia_process::discard(&work.dir);
        messages.push("dropped the RAM tree (keep_ram_tree = false)".to_string());
    }
    messages.push(phase_step_message("ram tree finish", &clock, &[]));
    messages
}

/// Copies a tree's package times next to the output path, on disk.
fn keep_package_durations(tree: &Path, to: Option<&Path>) {
    if let Some(to) = to {
        let _ = fs::copy(tree.join(PACKAGE_DURATIONS), to.join(PACKAGE_DURATIONS));
    }
}

pub(crate) fn recorded_tree_size(output_dir: &Path) -> Option<u64> {
    fs::read_to_string(output_dir.parent()?.join(RAM_SIZE_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Bytes in the files under `dir` (hard links once).
pub(crate) fn tree_size(dir: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut seen = BTreeSet::new();
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.nlink() < 2 || seen.insert((metadata.dev(), metadata.ino())) {
                total += metadata.blocks() * 512;
            }
        }
    }
    total
}

/// `MemAvailable` from /proc/meminfo.
pub(crate) fn mem_available() -> Option<u64> {
    parse_mem_available(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_mem_available(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kib| kib * 1024)
}

pub(crate) fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// Stops the build's commands, through their cancel check, when available
/// memory or the tree's tmpfs space runs low; runs until dropped.
pub(crate) struct RamWatchdog {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RamWatchdog {
    /// Watches a RAM tree at `dir`; returns the cancel check to give the
    /// build's commands (the original one, or low memory).
    pub(crate) fn start(
        dir: &Path,
        cancel_check: Option<ProcessCancelCheck>,
        log_sink: Option<ProcessLogSink>,
    ) -> (Self, ProcessCancelCheck) {
        let stop = Arc::new(AtomicBool::new(false));
        let low = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, low, dir) = (stop.clone(), low.clone(), dir.to_path_buf());
            std::thread::spawn(move || {
                let mut last = std::time::Instant::now() - Duration::from_secs(5);
                while !stop.load(Ordering::SeqCst) && !low.load(Ordering::SeqCst) {
                    if last.elapsed() >= Duration::from_secs(5) {
                        last = std::time::Instant::now();
                        let memory = mem_available();
                        let space = filesystem_available_bytes(&dir);
                        if let Some(reason) = ram_low(memory, space) {
                            let line = format!(
                                "RAM low ({reason}); stopping the Buildroot build. Free \
                                 memory and run again to resume, or set \
                                 providers.buildroot.work_dir = \"disk\""
                            );
                            tracing::warn!(provider_domain = "image.buildroot", "{line}");
                            if let Some(sink) = &log_sink {
                                sink(gaia_process::ProcessLogLine {
                                    stream: gaia_process::ProcessLogStream::Stderr,
                                    line,
                                });
                            }
                            low.store(true, Ordering::SeqCst);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
        };
        let check: ProcessCancelCheck = Arc::new(move || {
            low.load(Ordering::SeqCst) || cancel_check.as_ref().is_some_and(|cancel| cancel())
        });
        (
            Self {
                stop,
                thread: Some(thread),
            },
            check,
        )
    }
}

impl Drop for RamWatchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why a RAM build must stop, if it must.
fn ram_low(memory: Option<u64>, space: Option<u64>) -> Option<String> {
    match (memory, space) {
        (Some(memory), _) if memory < RAM_LOW => {
            Some(format!("{} of memory available", gib(memory)))
        }
        (_, Some(space)) if space < RAM_LOW => Some(format!("{} free on tmpfs", gib(space))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(work_dir: &str, budget: Option<&str>) -> ImageExecutionPolicy {
        ImageExecutionPolicy {
            work_dir: gaia_spec::BuildrootWorkDirPolicySpec {
                work_dir: work_dir.to_string(),
                ram_budget: budget.map(str::to_string),
                keep_ram_tree: true,
            },
            ..ImageExecutionPolicy::default()
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gaia-ram-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_work_dir_tree_is_linked_from_the_build_dir_and_dropped_for_disk() {
        let root = temp("workdir");
        let output = root.join("build/image/buildroot-output");
        fs::create_dir_all(&output).expect("disk tree");
        fs::write(output.join("old"), "x").expect("old file");
        let base = root.join("fast");

        let work =
            place_work_dir(&output, &policy(&base.display().to_string(), None)).expect("work dir");
        assert!(!work.ram);
        assert!(work.dir.starts_with(&base));
        assert_eq!(fs::read_link(&output).expect("link"), work.dir);
        assert!(!work.dir.join("old").exists());
        fs::write(work.dir.join("built"), "y").expect("built");
        assert!(output.join("built").is_file());

        // Again: the same tree.
        let again =
            place_work_dir(&output, &policy(&base.display().to_string(), None)).expect("again");
        assert_eq!(again.dir, work.dir);
        assert!(again.dir.join("built").is_file());

        // Gone (a reboot): a fresh one at the same place.
        fs::remove_dir_all(&work.dir).expect("reboot");
        let fresh =
            place_work_dir(&output, &policy(&base.display().to_string(), None)).expect("fresh");
        assert!(fresh.dir.is_dir());
        assert!(
            fresh
                .messages
                .iter()
                .any(|message| message.contains("was gone"))
        );

        // Back to disk: a real directory again.
        let disk = place_work_dir(&output, &policy("disk", None)).expect("disk");
        assert_eq!(disk.dir, output);
        assert!(fs::symlink_metadata(&output).is_err() || !output.is_symlink());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_ram_tree_needs_its_budget_and_free_memory() {
        let root = temp("budget");
        let output = root.join("build/image/buildroot-output");
        // A previous tree of 500 GiB never fits a 60 GiB budget.
        fs::create_dir_all(root.join("build/image")).expect("dirs");
        fs::write(
            root.join("build/image").join(RAM_SIZE_FILE),
            (500 * GIB).to_string(),
        )
        .expect("size");
        let work = place_work_dir(&output, &policy("ram", Some("60G"))).expect("fallback");
        assert!(!work.ram);
        assert_eq!(work.dir, output);
        assert!(
            work.messages[0].contains("building on disk"),
            "{:?}",
            work.messages
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn low_memory_or_tmpfs_space_stops_the_build() {
        assert_eq!(
            parse_mem_available("MemTotal: 100 kB\nMemAvailable:    2048 kB\n"),
            Some(2 * 1024 * 1024)
        );
        assert!(ram_low(Some(64 * GIB), Some(30 * GIB)).is_none());
        assert!(ram_low(Some(GIB), Some(30 * GIB)).is_some());
        assert!(ram_low(Some(64 * GIB), Some(GIB)).is_some());
        assert!(ram_low(None, None).is_none());
    }

    /// A Buildroot source as a source provider materializes it, recording
    /// `digest`.
    fn materialized_source(root: &Path, digest: &str) -> PathBuf {
        let source = root.join("source");
        fs::create_dir_all(source.join("package")).expect("source dirs");
        fs::write(source.join("Makefile"), "all:\n").expect("makefile");
        fs::write(source.join("package/pkg-utils.mk"), "# pkg\n").expect("pkg-utils");
        fs::write(
            source.join(".gaia-source-state.txt"),
            format!("materialized_tree_digest={digest}\nbuild_version=1\n"),
        )
        .expect("state");
        source
    }

    fn ram_work(root: &Path) -> WorkDir {
        WorkDir {
            dir: root.join("tree/buildroot-output"),
            ram: true,
            messages: Vec::new(),
        }
    }

    #[test]
    fn an_unchanged_source_is_not_copied_again() {
        let root = temp("mirror-skip");
        let source = materialized_source(&root, "tree-1");
        let work = ram_work(&root);
        let (mirror, notes) = buildroot_source_for(&source, &work, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("mirror"),
            "all:\n"
        );

        // A copy would restore this; the skipped mirror keeps it.
        fs::write(mirror.join("Makefile"), "kept by the test\n").expect("sentinel");
        let (again, notes) = buildroot_source_for(&source, &work, true);
        assert_eq!(again, mirror);
        assert_eq!(notes, ["buildroot source mirror: unchanged"]);
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("kept"),
            "kept by the test\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_changed_digest_or_entry_copies_the_source_again() {
        let root = temp("mirror-changed");
        let source = materialized_source(&root, "tree-1");
        let work = ram_work(&root);
        let (mirror, _) = buildroot_source_for(&source, &work, true);
        fs::write(mirror.join("Makefile"), "kept by the test\n").expect("sentinel");

        // Re-materialized with another digest: copied.
        fs::write(
            source.join(".gaia-source-state.txt"),
            "materialized_tree_digest=tree-2\n",
        )
        .expect("new digest");
        let (_, notes) = buildroot_source_for(&source, &work, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("copied"),
            "all:\n"
        );

        // A new top-level entry, with the same digest: copied.
        fs::write(mirror.join("Makefile"), "kept by the test\n").expect("sentinel");
        fs::write(source.join("extra.txt"), "new\n").expect("entry");
        buildroot_source_for(&source, &work, true);
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("copied"),
            "all:\n"
        );
        assert!(mirror.join("extra.txt").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_mirror_is_copied_in_full() {
        let root = temp("mirror-missing");
        let source = materialized_source(&root, "tree-1");
        let work = ram_work(&root);
        let (mirror, _) = buildroot_source_for(&source, &work, true);
        // Cleared, as after a reboot, while the source is unchanged.
        fs::remove_dir_all(&mirror).expect("reboot");
        let (mirror, notes) = buildroot_source_for(&source, &work, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(mirror.join("Makefile").is_file());
        assert!(mirror.join("package/pkg-utils.mk").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_source_without_a_trusted_identity_is_copied_every_time() {
        let root = temp("mirror-untrusted");
        let source = materialized_source(&root, "tree-1");
        // A path-only record names no content.
        fs::write(
            source.join(".gaia-source-state.txt"),
            "path=/somewhere\npath_digest=abc\n",
        )
        .expect("state");
        let work = ram_work(&root);
        let (mirror, _) = buildroot_source_for(&source, &work, true);
        fs::write(mirror.join("Makefile"), "kept by the test\n").expect("sentinel");
        let (_, notes) = buildroot_source_for(&source, &work, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("copied"),
            "all:\n"
        );
        assert!(!mirror.join(MIRROR_SOURCE_MARKER).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_skipped_mirror_reverts_gaia_patches_when_no_patches_apply() {
        let root = temp("mirror-patches");
        let source = materialized_source(&root, "tree-1");
        let work = ram_work(&root);
        let (mirror, _) = buildroot_source_for(&source, &work, true);
        // What an earlier parallel run left in the mirror.
        let patched = format!("{HOST_FINALIZE_SKIP}\n");
        fs::write(mirror.join("Makefile"), &patched).expect("patched");

        // Parallel packages: the patches stay, the next patch step keeps them.
        buildroot_source_for(&source, &work, true);
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("kept"),
            patched
        );
        // Not parallel: the mirror reads as a fresh copy would.
        let (_, notes) = buildroot_source_for(&source, &work, false);
        assert_eq!(notes, ["buildroot source mirror: unchanged"]);
        assert_eq!(
            fs::read_to_string(mirror.join("Makefile")).expect("reverted"),
            HOST_FINALIZE_UPSTREAM.to_string() + "\n"
        );
        let _ = fs::remove_dir_all(root);
    }
}
