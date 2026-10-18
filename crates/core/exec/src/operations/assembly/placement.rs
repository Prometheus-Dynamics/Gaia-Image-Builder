//! Where an assembly's intermediates are written: in the build dir (the
//! default) or in RAM.
//!
//! `[image.assembly] work_dir = "ram"` puts the intermediates in RAM under
//! `<base>/gaia-<user>/<hash>/assembly`, where `<base>` is `/dev/shm`: the
//! trees, filesystem images such as `boot.vfat`, the transform outputs under
//! the work dir, and the raw disk images. An unset `work_dir` follows the
//! Buildroot tree (`[providers.buildroot] work_dir = "ram"`). A path, or
//! `"disk"`, keeps the intermediates on disk as before.
//!
//! The published outputs stay on disk: the archive, every raw disk image at
//! its spec path, and every published filesystem image in the image output
//! dir. A raw disk is built in RAM and copied to its place on
//! disk when built (sparse, so unwritten space costs nothing). The RAM copies
//! are re-creatable and are removed when the assembly ends.
//!
//! RAM is used only when the expected size of the intermediates fits in
//! available memory and in the tmpfs, each with a margin. Otherwise the
//! assembly runs on disk and says why.
//!
//! Intermediates in RAM are not in the collect dir: a filesystem image such
//! as `boot.vfat` is no longer left there in RAM mode.

use super::*;
use gaia_spec::{
    AssemblyJsonValue, AssemblyPathTemplate, AssemblyWorkDirKeyword, ImageAssemblySpec,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;
/// Where RAM assemblies live: a tmpfs.
pub(crate) const RAM_BASE: &str = "/dev/shm";
/// Free memory and tmpfs space kept beyond the expected size of the
/// intermediates.
const RAM_MARGIN: u64 = 4 * GIB;
/// Chunk size for copying a raw disk without writing its zeros.
const SPARSE_CHUNK: usize = MIB as usize;
/// Prefixes of the references in archive entries, generated files and JSON
/// values, inside `${...}`: a file's digest, a file's size, and a field of a
/// verity transform's output.
const DIGEST_TOKEN_PREFIX: &str = "assembly.sha256:";
const SIZE_TOKEN_PREFIX: &str = "assembly.size:";
const VERITY_TOKEN_PREFIX: &str = "assembly.verity:";
/// A tree digest names a tree id (kept, as the RAM copy keeps the id) or a
/// path, which moves like any other path.
const TREE_DIGEST_TOKEN_PREFIX: &str = "assembly.sha256_tree:";

/// Where the facts a RAM placement depends on come from. Tests fill it in;
/// a run reads it from the system, and only when RAM is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlacementEnv {
    /// The tmpfs base (`/dev/shm`).
    pub(crate) ram_base: PathBuf,
    /// The user in the RAM path (`gaia-<user>`).
    pub(crate) user: String,
    /// `MemAvailable` in bytes, when readable.
    pub(crate) memory_available: Option<u64>,
    /// Free space on the tmpfs in bytes, when readable.
    pub(crate) tmpfs_available: Option<u64>,
}

impl PlacementEnv {
    pub(crate) fn system() -> Self {
        let ram_base = ram_base();
        Self {
            user: current_user(),
            memory_available: mem_available(),
            tmpfs_available: tmpfs_available(&ram_base),
            ram_base,
        }
    }
}

/// The tmpfs RAM work dirs live under: [`RAM_BASE`], or `GAIA_RAM_BASE` when
/// set (tests point it at a scratch dir, so they never touch the real
/// per-user dir that holds other builds' RAM trees). Unit tests of this crate
/// always use a scratch base.
fn ram_base() -> PathBuf {
    if let Some(base) = std::env::var_os("GAIA_RAM_BASE").filter(|base| !base.is_empty()) {
        return PathBuf::from(base);
    }
    if cfg!(test) {
        return std::env::temp_dir().join("gaia-test-ram-base");
    }
    PathBuf::from(RAM_BASE)
}

/// The decision for one assembly run.
#[derive(Debug)]
pub(super) enum AssemblyPlacement {
    Disk {
        messages: Vec<String>,
    },
    Ram(Box<RamPlacement>),
    /// RAM was asked for and the RAM base cannot be trusted: the run stops.
    Refused(String),
}

#[derive(Debug)]
pub(super) struct RamPlacement {
    /// `<base>/gaia-<user>/<hash>/assembly`, removed when the run ends.
    pub(super) root: PathBuf,
    /// `<base>/gaia-<user>`: the stale work dirs of earlier runs live here.
    pub(super) user_dir: PathBuf,
    /// The spec with every path absolute, the intermediates in RAM.
    pub(super) assembly: ImageAssemblySpec,
    /// The same paths as [`AssemblyRoots`] for `assembly`.
    pub(super) roots: AssemblyRoots,
    /// Per disk: the on-disk path it is published to, from its spec.
    pub(super) disk_publish: Vec<PathBuf>,
    /// Disk-view paths of the intermediates: removed before the run so an
    /// earlier disk run's files are not mistaken for this run's.
    pub(super) stale: Vec<PathBuf>,
    pub(super) expected_bytes: u64,
    pub(super) messages: Vec<String>,
}

/// Decides where the run's intermediates go. `disk_roots` is the spec's own
/// view of the paths. `env` is only read when RAM is asked for.
pub(super) fn decide_placement(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    disk_roots: &AssemblyRoots,
    env: impl FnOnce() -> PlacementEnv,
) -> AssemblyPlacement {
    if !ram_requested(spec, assembly) {
        return AssemblyPlacement::Disk {
            messages: Vec::new(),
        };
    }
    let env = env();
    match ram_placement(spec, assembly, disk_roots, &env) {
        // The RAM base is shared (a world-writable tmpfs): only a directory
        // this user owns is used, and a foreign or linked one stops the build.
        Ok(ram) => match check_ram_user_dir(&env.ram_base, &env.user) {
            Ok(()) => AssemblyPlacement::Ram(Box::new(ram)),
            Err(reason) => AssemblyPlacement::Refused(reason),
        },
        Err(reason) => AssemblyPlacement::Disk {
            messages: vec![format!(
                "assembly work dir falls back to disk: {reason}; building on disk"
            )],
        },
    }
}

/// Whether the work dir setting asks for RAM: `ram` itself, or no setting
/// while the Buildroot tree is in RAM.
fn ram_requested(spec: &ResolvedBuildSpec, assembly: &ImageAssemblySpec) -> bool {
    match assembly.work_dir.as_ref() {
        Some(work_dir) => {
            gaia_spec::assembly_work_dir_keyword(work_dir.as_str())
                == Some(AssemblyWorkDirKeyword::Ram)
        }
        None => {
            spec.image.provider_kind() == gaia_spec::ImageProviderKind::Buildroot
                && spec.policy.providers.buildroot.work_dir.work_dir == "ram"
        }
    }
}

fn ram_placement(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    disk_roots: &AssemblyRoots,
    env: &PlacementEnv,
) -> Result<RamPlacement, String> {
    let root = ram_root(&env.ram_base, &env.user, &disk_roots.assembly_work);
    let expected = expected_bytes(spec, assembly, disk_roots);
    let needed = expected.saturating_add(RAM_MARGIN);
    match env.memory_available {
        None => return Err("MemAvailable is unknown".into()),
        Some(available) if available < needed => {
            return Err(format!(
                "{} of memory available, {} needed",
                gib(available),
                gib(needed)
            ));
        }
        Some(_) => {}
    }
    match env.tmpfs_available {
        None => {
            return Err(format!(
                "free space on '{}' is unknown",
                env.ram_base.display()
            ));
        }
        Some(available) if available < needed => {
            return Err(format!(
                "{} free on '{}', {} needed",
                gib(available),
                env.ram_base.display(),
                gib(needed)
            ));
        }
        Some(_) => {}
    }

    let plan = intermediate_moves(spec, assembly, disk_roots, &root)?;
    let rewritten = rewrite_assembly(spec, assembly, disk_roots, &plan.moves, &root)?;
    let roots = AssemblyRoots::new(spec, &rewritten)?;
    let messages = vec![format!(
        "assembly intermediates in RAM at '{}' ({} expected)",
        root.display(),
        gib(expected)
    )];
    Ok(RamPlacement {
        root,
        user_dir: env.ram_base.join(format!("gaia-{}", env.user)),
        assembly: rewritten,
        roots,
        disk_publish: plan.disk_publish,
        stale: plan.stale,
        expected_bytes: expected,
        messages,
    })
}

/// `<base>/gaia-<user>/<hash of the work dir>/assembly`: stable per build.
pub(super) fn ram_root(base: &Path, user: &str, work_dir: &Path) -> PathBuf {
    let digest = Sha256::digest(work_dir.to_string_lossy().as_bytes());
    let hash: String = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    base.join(format!("gaia-{user}"))
        .join(hash)
        .join("assembly")
}

/// Bytes the intermediates need, from the spec: the disks' layouts, the
/// filesystem sizes and the transforms' inputs. An upper bound, not a
/// measurement.
fn expected_bytes(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    disk_roots: &AssemblyRoots,
) -> u64 {
    let mut filesystem_sizes = HashMap::new();
    let mut total = 0u64;
    for filesystem in &assembly.filesystems {
        let size = filesystem
            .parsed_size()
            .ok()
            .flatten()
            .map_or(0, |size| size.bytes());
        if let Ok(output) = disk_roots.resolve_path(spec, &filesystem.output) {
            filesystem_sizes.insert(output, size);
        }
        total = total.saturating_add(size);
    }
    for transform in &assembly.transforms {
        if let Some(src) = &transform.src
            && let Ok(path) = disk_roots.resolve_path(spec, src)
        {
            total = total.saturating_add(std::fs::metadata(path).map_or(0, |m| m.len()));
        }
    }
    for disk in &assembly.disks {
        total = total.saturating_add(MIB);
        for partition in &disk.partitions {
            let size = partition
                .parsed_size()
                .ok()
                .flatten()
                .map(|size| size.bytes());
            let image = partition
                .image
                .as_ref()
                .and_then(|image| disk_roots.resolve_path(spec, image).ok());
            let bytes = size
                .or_else(|| {
                    image
                        .as_ref()
                        .and_then(|image| filesystem_sizes.get(image).copied())
                        .filter(|bytes| *bytes > 0)
                })
                .or_else(|| {
                    image
                        .as_ref()
                        .and_then(|image| std::fs::metadata(image).ok())
                        .map(|metadata| metadata.len())
                })
                .unwrap_or(0);
            total = total.saturating_add(bytes);
        }
    }
    total
}

/// Where each intermediate moves: `(disk-view path, RAM path)`, the longest
/// matching prefix winning.
struct IntermediateMoves {
    moves: Vec<(PathBuf, PathBuf)>,
    disk_publish: Vec<PathBuf>,
    stale: Vec<PathBuf>,
}

fn intermediate_moves(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    disk_roots: &AssemblyRoots,
    root: &Path,
) -> Result<IntermediateMoves, String> {
    let work = disk_roots.assembly_work.clone();
    let mut moves = vec![(work.clone(), root.join("work"))];
    let mut stale = Vec::new();
    for tree in &assembly.trees {
        let from = disk_roots.tree_path(&tree.id)?.to_path_buf();
        moves.push((from.clone(), root.join("trees").join(tree.id.to_string())));
        stale.push(from);
    }
    // Outputs outside the work dir and the trees get their own RAM name.
    let mut names = BTreeSet::new();
    let mut place_output = |moves: &mut Vec<(PathBuf, PathBuf)>,
                            from: PathBuf|
     -> Result<(), String> {
        if !moves.iter().any(|(prefix, _)| from.starts_with(prefix)) {
            let name = from
                .file_name()
                .ok_or_else(|| format!("assembly output '{}' has no file name", from.display()))?
                .to_os_string();
            if !names.insert(name.clone()) {
                return Err(format!(
                    "two intermediate outputs are named '{}'",
                    name.to_string_lossy()
                ));
            }
            moves.push((from.clone(), root.join("out").join(name)));
        }
        stale.push(from);
        Ok(())
    };
    for filesystem in &assembly.filesystems {
        let from = disk_roots.resolve_path(spec, &filesystem.output)?;
        place_output(&mut moves, from)?;
    }
    let mut disk_publish = Vec::new();
    for disk in &assembly.disks {
        let from = disk_roots.resolve_path(spec, &disk.output)?;
        disk_publish.push(from.clone());
        place_output(&mut moves, from)?;
    }
    Ok(IntermediateMoves {
        moves,
        disk_publish,
        stale,
    })
}

/// The RAM path for a disk-view path: the longest matching move.
pub(super) fn map_path(moves: &[(PathBuf, PathBuf)], path: &Path) -> Option<PathBuf> {
    let (from, to) = moves
        .iter()
        .filter(|(from, _)| path.starts_with(from))
        .max_by_key(|(from, _)| from.components().count())?;
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() {
        to.clone()
    } else {
        to.join(rest)
    })
}

/// The spec with every path template resolved to an absolute path, moved
/// into RAM where it is an intermediate. Resolving here, with the spec's own
/// view, keeps `$assembly.work` and `$assembly.out` meaning what they meant.
fn rewrite_assembly(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    disk_roots: &AssemblyRoots,
    moves: &[(PathBuf, PathBuf)],
    root: &Path,
) -> Result<ImageAssemblySpec, String> {
    let absolute = |template: &AssemblyPathTemplate| -> Result<AssemblyPathTemplate, String> {
        let path = disk_roots.resolve_path(spec, template.as_str())?;
        let mapped = map_path(moves, &path).unwrap_or(path);
        Ok(AssemblyPathTemplate::new(mapped.display().to_string()))
    };
    let absolute_opt =
        |template: &Option<AssemblyPathTemplate>| template.as_ref().map(absolute).transpose();
    let mut rewritten = assembly.clone();
    rewritten.work_dir = Some(AssemblyPathTemplate::new(
        root.join("work").display().to_string(),
    ));
    rewritten.out_dir = absolute_opt(&assembly.out_dir)?;
    for tree in &mut rewritten.trees {
        tree.path = absolute(&tree.path)?;
    }
    // Generated text and JSON name their tokens' paths; those move with the
    // rest of the assembly.
    let rewrite_text = |text: &str| -> Result<String, String> {
        rewrite_digest_tokens(
            text,
            |inner| {
                absolute(&AssemblyPathTemplate::new(inner.trim()))
                    .map(|template| template.as_str().to_string())
            },
            |inner| {
                if disk_roots.trees.contains_key(inner.trim()) {
                    Ok(inner.trim().to_string())
                } else {
                    absolute(&AssemblyPathTemplate::new(inner.trim()))
                        .map(|template| template.as_str().to_string())
                }
            },
        )
    };
    for file in &mut rewritten.files {
        file.src = absolute_opt(&file.src)?;
        file.src_glob = absolute_opt(&file.src_glob)?;
        file.content = file.content.as_deref().map(rewrite_text).transpose()?;
        if let Some(json) = &mut file.json {
            rewrite_json_strings(json, &rewrite_text)?;
        }
    }
    for binary in &mut rewritten.binaries {
        binary.src = absolute(&binary.src)?;
        binary.sysroot = absolute_opt(&binary.sysroot)?;
    }
    for modules in &mut rewritten.kernel_modules {
        modules.from = absolute(&modules.from)?;
        modules.depmod = absolute_opt(&modules.depmod)?;
    }
    for transform in &mut rewritten.transforms {
        transform.src = absolute_opt(&transform.src)?;
        transform.dest = absolute(&transform.dest)?;
        transform.hash_dest = absolute_opt(&transform.hash_dest)?;
    }
    for filesystem in &mut rewritten.filesystems {
        filesystem.output = absolute(&filesystem.output)?;
    }
    for initramfs in &mut rewritten.busybox_initramfs {
        initramfs.busybox = absolute(&initramfs.busybox)?;
        initramfs.sysroot = absolute_opt(&initramfs.sysroot)?;
    }
    for disk in &mut rewritten.disks {
        disk.output = absolute(&disk.output)?;
        for partition in &mut disk.partitions {
            partition.image = absolute_opt(&partition.image)?;
        }
    }
    for archive in &mut rewritten.archives {
        archive.output = absolute(&archive.output)?;
        for member in &mut archive.members {
            member.src = absolute_opt(&member.src)?;
            member.src_dir = absolute_opt(&member.src_dir)?;
            member.src_glob = absolute_opt(&member.src_glob)?;
            if let Some(entries) = &mut member.entries {
                for (_, value) in entries.iter_mut() {
                    *value = rewrite_text(value)?;
                }
            }
            if let Some(json) = &mut member.json {
                rewrite_json_strings(json, &rewrite_text)?;
            }
        }
    }
    Ok(rewritten)
}

/// Rewrites every string of a JSON value with `rewrite`.
fn rewrite_json_strings(
    value: &mut AssemblyJsonValue,
    rewrite: &dyn Fn(&str) -> Result<String, String>,
) -> Result<(), String> {
    match value {
        AssemblyJsonValue::String(text) => *text = rewrite(text)?,
        AssemblyJsonValue::Integer(_) | AssemblyJsonValue::Bool(_) => {}
        AssemblyJsonValue::Array(items) => {
            for item in items {
                rewrite_json_strings(item, rewrite)?;
            }
        }
        AssemblyJsonValue::Object(members) => {
            for (_, item) in members {
                rewrite_json_strings(item, rewrite)?;
            }
        }
    }
    Ok(())
}

/// Rewrites the path inside each `${assembly.sha256:<path>}`,
/// `${assembly.sha256_tree:<tree or path>}`, `${assembly.size:<path>}` and
/// `${assembly.verity:<path>:<field>}` in `value`; the field and any other
/// token are kept as they are. A tree digest's path goes to `rewrite_tree`.
fn rewrite_digest_tokens(
    value: &str,
    mut rewrite: impl FnMut(&str) -> Result<String, String>,
    mut rewrite_tree: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let token = &after[..end];
        if let Some(path) = token.strip_prefix(DIGEST_TOKEN_PREFIX) {
            out.push_str("${");
            out.push_str(DIGEST_TOKEN_PREFIX);
            out.push_str(&rewrite(path)?);
            out.push('}');
        } else if let Some(path) = token.strip_prefix(TREE_DIGEST_TOKEN_PREFIX) {
            out.push_str("${");
            out.push_str(TREE_DIGEST_TOKEN_PREFIX);
            out.push_str(&rewrite_tree(path)?);
            out.push('}');
        } else if let Some(path) = token.strip_prefix(SIZE_TOKEN_PREFIX) {
            out.push_str("${");
            out.push_str(SIZE_TOKEN_PREFIX);
            out.push_str(&rewrite(path)?);
            out.push('}');
        } else if let Some(body) = token.strip_prefix(VERITY_TOKEN_PREFIX)
            && let Some((path, field)) = body.rsplit_once(':')
        {
            out.push_str("${");
            out.push_str(VERITY_TOKEN_PREFIX);
            out.push_str(&rewrite(path)?);
            out.push(':');
            out.push_str(field);
            out.push('}');
        } else {
            out.push_str("${");
            out.push_str(token);
            out.push('}');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Writes `source` (a RAM file) to `target` on disk, with the same bytes,
/// and publishes it the way every assembly output is published.
pub(super) fn publish_copy_to_disk(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly output dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temp = temporary_assembly_output_path(target);
    if let Err(error) = copy_sparse(source, &temp) {
        let _ = std_fs::remove_file(&temp);
        return Err(format!(
            "failed to copy assembly disk '{}' to '{}': {error}",
            source.display(),
            target.display()
        ));
    }
    publish_assembly_output(&temp, target).map(|_| ())
}

/// Copies a file, leaving its zero runs as holes: the copy is the same bytes
/// and a raw disk image with unwritten partitions costs only its data.
pub(super) fn copy_sparse(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut input = std_fs::File::open(source)?;
    let len = input.metadata()?.len();
    let mut output = std_fs::File::create(target)?;
    copy_sparse_into(&mut input, &mut output)?;
    output.set_len(len)
}

/// Copies `input` into `output` from their current positions, skipping zero
/// runs with a seek so they stay holes where `output` is a fresh (or
/// truncated) file. The bytes are the same as a plain copy. A copy that ends
/// in a zero run does not extend `output`: the caller sets its length.
pub(super) fn copy_sparse_into(
    input: &mut std_fs::File,
    output: &mut std_fs::File,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut buffer = vec![0u8; SPARSE_CHUNK];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().all(|byte| *byte == 0) {
            output.seek(SeekFrom::Current(read as i64))?;
        } else {
            output.write_all(&buffer[..read])?;
        }
    }
    Ok(())
}

/// Removes the RAM copies of a run. Best effort: they are re-creatable.
pub(super) fn discard_ram_root(root: &Path) {
    let _ = gaia_process::discard(root);
}

fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty() && !user.contains('/'))
        .unwrap_or_else(|| {
            use std::os::unix::fs::MetadataExt;
            std_fs::metadata("/proc/self")
                .map(|metadata| metadata.uid().to_string())
                .unwrap_or_else(|_| "user".to_string())
        })
}

/// `MemAvailable` from /proc/meminfo, in bytes.
fn mem_available() -> Option<u64> {
    let meminfo = std_fs::read_to_string("/proc/meminfo").ok()?;
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kib| kib * 1024)
}

/// Free bytes on the filesystem holding `dir`, from `df`.
fn tmpfs_available(dir: &Path) -> Option<u64> {
    let output = Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let available_kib = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()?;
    Some(available_kib * 1024)
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
