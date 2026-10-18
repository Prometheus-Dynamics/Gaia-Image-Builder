//! Stripping ELF files in an assembly: the copies a `strip = true` entry
//! makes in a tree, and every ELF file of a tree that is packed with
//! `strip = true`. Only the copies in the tree are touched, never the
//! source; a file reached through a symlink is stripped where it lives.

use super::*;
use gaia_spec::AssemblyPathTemplate;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `e_machine` values the strip tool is chosen by.
const EM_ARM: u16 = 40;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
/// `e_type` of a relocatable object (a kernel module is one too).
const ET_REL: u16 = 1;

/// What one strip pass changed: the files stripped and the bytes they lost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct StripSummary {
    pub(super) files: usize,
    pub(super) saved_bytes: u64,
}

/// The strip totals of one assembly run, recorded once the steps ran.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct StripTotals {
    pub(super) trees: usize,
    pub(super) files: usize,
    pub(super) saved_bytes: u64,
}

impl StripTotals {
    pub(super) fn add(&mut self, summary: StripSummary) {
        self.files += summary.files;
        self.saved_bytes += summary.saved_bytes;
    }
}

/// Strips ELF files with the tool the run resolves once per machine: the
/// configured `strip_tool`, else a `<triplet>-strip` of the provider's
/// tools, else `llvm-strip`, else a plain `strip` for the host's machine.
pub(super) struct Stripper<'a> {
    spec: &'a ResolvedBuildSpec,
    roots: &'a AssemblyRoots,
    configured: Option<&'a AssemblyPathTemplate>,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    tools: HashMap<u16, ResolvedTool>,
}

impl<'a> Stripper<'a> {
    pub(super) fn new(
        spec: &'a ResolvedBuildSpec,
        roots: &'a AssemblyRoots,
        configured: Option<&'a AssemblyPathTemplate>,
        cancel_check: Option<gaia_process::ProcessCancelCheck>,
    ) -> Self {
        Self {
            spec,
            roots,
            configured,
            cancel_check,
            tools: HashMap::new(),
        }
    }

    /// Strips the regular files among `paths` that are ELF files needing it.
    /// Symlinks and other non-regular paths are skipped. Hard links are
    /// stripped once and the other names are linked to the stripped file.
    pub(super) fn strip_paths(&mut self, paths: &[PathBuf]) -> Result<StripSummary, AssemblyError> {
        let mut groups: Vec<Vec<PathBuf>> = Vec::new();
        let mut group_of: HashMap<(u64, u64), usize> = HashMap::new();
        for path in paths {
            let metadata = std_fs::symlink_metadata(path).map_err(|error| {
                format!(
                    "failed to read assembly file metadata '{}' for strip: {error}",
                    path.display()
                )
            })?;
            if !metadata.is_file() {
                continue;
            }
            match hard_link_key(&metadata) {
                Some(key) => match group_of.get(&key) {
                    Some(&group) => groups[group].push(path.clone()),
                    None => {
                        group_of.insert(key, groups.len());
                        groups.push(vec![path.clone()]);
                    }
                },
                None => groups.push(vec![path.clone()]),
            }
        }

        // Every file is planned from its headers first, then the planned
        // files are stripped in batches; the hard-linked copies follow their
        // first file once it is stripped.
        let mut jobs = Vec::new();
        for (group_index, group) in groups.iter().enumerate() {
            if let Some(job) = self.plan_strip(&group[0], group_index)? {
                jobs.push(job);
            }
        }
        let stripped = run_strip_jobs(self.spec, self.cancel_check.clone(), &jobs)?;
        let mut summary = StripSummary::default();
        for (group_index, saved) in stripped {
            summary.files += 1;
            summary.saved_bytes += saved;
            let group = &groups[group_index];
            for other in &group[1..] {
                relink(&group[0], other)?;
            }
        }
        Ok(summary)
    }

    /// Strips one file in place when it is an ELF file that still has what
    /// its kind strips. `Ok(None)` leaves a file alone.
    fn plan_strip(&mut self, path: &Path, group: usize) -> Result<Option<StripJob>, AssemblyError> {
        // Most tree files are not ELF: their magic settles it without a read.
        if !has_elf_magic(path) {
            return Ok(None);
        }
        // Only the headers and section names are read, by seeking.
        let mut file = std_fs::File::open(path).map_err(|error| {
            format!(
                "failed to read assembly file '{}' for strip: {error}",
                path.display()
            )
        })?;
        let Some(sections) = gaia_plan::elf_sections_of_file(&mut file).map_err(|error| {
            format!(
                "assembly file '{}' is not a readable ELF file: {error}",
                path.display()
            )
        })?
        else {
            return Ok(None);
        };
        // Kernel modules and other relocatable objects keep their symbols,
        // which the loader or the linker needs: only their debug data goes.
        let relocatable = sections.object_type == ET_REL || is_kernel_module(path);
        let (flag, needed) = if relocatable {
            ("--strip-debug", sections.has_debug)
        } else {
            (
                "--strip-unneeded",
                sections.has_symtab || sections.has_debug,
            )
        };
        if !needed {
            return Ok(None);
        }
        let tool = self.tool_for(sections.machine, path)?;
        let before = std_fs::metadata(path)
            .map_err(|error| {
                format!(
                    "failed to read assembly file '{}' for strip: {error}",
                    path.display()
                )
            })?
            .len();
        Ok(Some(StripJob {
            group,
            path: path.to_path_buf(),
            flag,
            tool,
            before,
        }))
    }

    /// The strip tool for files of `machine`. A configured tool is used for
    /// every machine; otherwise the choice is made once per machine.
    fn tool_for(&mut self, machine: u16, file: &Path) -> Result<ResolvedTool, AssemblyError> {
        if let Some(tool) = self.configured_tool()? {
            return Ok(tool);
        }
        if let Some(tool) = self.tools.get(&machine) {
            return Ok(tool.clone());
        }
        let tool = find_strip_tool(self.roots, machine).ok_or_else(|| {
            format!(
                "no strip tool for '{}' (ELF machine {machine}): set [image.assembly] strip_tool, \
                 install a '{}*-strip' tool in the provider's host tools, put llvm-strip on PATH, \
                 or use a plain strip on the host's own machine",
                file.display(),
                triplet_prefix(machine).unwrap_or("<triplet>-")
            )
        })?;
        self.tools.insert(machine, tool.clone());
        Ok(tool)
    }

    /// The configured `strip_tool`, when set: a program name looked up like
    /// the other assembly tools, or a path (which may use `$` tokens).
    fn configured_tool(&self) -> Result<Option<ResolvedTool>, AssemblyError> {
        let Some(template) = self.configured else {
            return Ok(None);
        };
        let text = template.as_str();
        if !text.contains('/') && !text.contains('$') {
            return resolve_assembly_tool(self.roots, text)
                .map(Some)
                .map_err(AssemblyError::from);
        }
        let path = self.roots.resolve_path(self.spec, text)?;
        if !path.is_file() {
            return Err(format!(
                "assembly strip_tool '{text}' was not found at '{}'",
                path.display()
            )
            .into());
        }
        Ok(Some(ResolvedTool {
            display: path.display().to_string(),
            program: path,
        }))
    }
}

/// Files per strip invocation: one tool run per chunk, not per file.
const STRIP_CHUNK_FILES: usize = 64;

/// An ELF file to strip, planned from its headers.
struct StripJob {
    /// The hard-link group of the file, by its index in the pass.
    group: usize,
    path: PathBuf,
    flag: &'static str,
    tool: ResolvedTool,
    /// The file's size before stripping.
    before: u64,
}

/// Strips the planned files: chunks of up to `STRIP_CHUNK_FILES` jobs that
/// share a tool and a flag, one tool run each, with the chunks run in
/// parallel. Returns each job's group and the bytes it saved, in job order.
fn run_strip_jobs(
    spec: &ResolvedBuildSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    jobs: &[StripJob],
) -> Result<Vec<(usize, u64)>, AssemblyError> {
    let mut chunks: Vec<Vec<usize>> = Vec::new();
    let mut open: HashMap<(PathBuf, &'static str), usize> = HashMap::new();
    for (index, job) in jobs.iter().enumerate() {
        let key = (job.tool.program.clone(), job.flag);
        let slot = match open.get(&key) {
            Some(&slot) if chunks[slot].len() < STRIP_CHUNK_FILES => slot,
            _ => {
                chunks.push(Vec::new());
                open.insert(key, chunks.len() - 1);
                chunks.len() - 1
            }
        };
        chunks[slot].push(index);
    }

    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(chunks.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let outcomes = Mutex::new(Vec::with_capacity(chunks.len()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let chunk = next.fetch_add(1, Ordering::Relaxed);
                    let Some(indices) = chunks.get(chunk) else {
                        break;
                    };
                    let result = run_strip_chunk(spec, cancel_check.clone(), jobs, indices);
                    let mut done = outcomes.lock().unwrap_or_else(|poison| poison.into_inner());
                    done.push((chunk, result));
                }
            });
        }
    });
    // The earliest failing chunk is the error, whatever order they ran in.
    let mut outcomes = outcomes
        .into_inner()
        .unwrap_or_else(|poison| poison.into_inner());
    outcomes.sort_by_key(|(chunk, _)| *chunk);
    if let Some((_, Err(error))) = outcomes.into_iter().find(|(_, result)| result.is_err()) {
        return Err(error);
    }

    let mut saved = Vec::with_capacity(jobs.len());
    for job in jobs {
        let after = std_fs::metadata(&job.path)
            .map_err(|error| {
                format!(
                    "failed to read stripped assembly file '{}': {error}",
                    job.path.display()
                )
            })?
            .len();
        saved.push((job.group, job.before.saturating_sub(after)));
    }
    Ok(saved)
}

/// One strip run over the files of `indices`, which share a tool and a flag.
fn run_strip_chunk(
    spec: &ResolvedBuildSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    jobs: &[StripJob],
    indices: &[usize],
) -> Result<(), AssemblyError> {
    let first = &jobs[indices[0]];
    let mut command = Command::new(&first.tool.program);
    command.arg(first.flag);
    for index in indices {
        command.arg(&jobs[*index].path);
    }
    let output = run_command_capture_tail(
        spec,
        &mut command,
        process_output_retention(spec),
        cancel_check,
    )?;
    if !output.status.success() {
        return Err(format!(
            "strip of {} file(s) starting at '{}' with '{}' failed: {}",
            indices.len(),
            first.path.display(),
            first.tool.display,
            output.failure_context(&command)
        )
        .into());
    }
    Ok(())
}

/// The strip tool for an ELF `machine` when none is configured.
fn find_strip_tool(roots: &AssemblyRoots, machine: u16) -> Option<ResolvedTool> {
    if let (Some(prefix), Some(host)) = (triplet_prefix(machine), &roots.provider_host) {
        for relative in ["bin", "sbin", "usr/bin", "usr/sbin", ""] {
            let dir = host.join(relative);
            let Ok(entries) = std_fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.starts_with(prefix) && name.ends_with("-strip"))
                .collect();
            names.sort();
            for name in names {
                let candidate = dir.join(&name);
                if candidate.is_file() {
                    return Some(ResolvedTool {
                        display: candidate.display().to_string(),
                        program: candidate,
                    });
                }
            }
        }
    }
    if let Ok(tool) = resolve_assembly_tool(roots, "llvm-strip") {
        return Some(tool);
    }
    if host_machine() == Some(machine) {
        return resolve_assembly_tool(roots, "strip").ok();
    }
    None
}

/// The `<triplet>-` prefix of a `*-strip` tool for an ELF machine.
fn triplet_prefix(machine: u16) -> Option<&'static str> {
    match machine {
        EM_ARM => Some("arm-"),
        EM_AARCH64 => Some("aarch64-"),
        EM_X86_64 => Some("x86_64-"),
        _ => None,
    }
}

/// The ELF machine of the host, when it is one the table knows.
fn host_machine() -> Option<u16> {
    match std::env::consts::ARCH {
        "arm" => Some(EM_ARM),
        "aarch64" => Some(EM_AARCH64),
        "x86_64" => Some(EM_X86_64),
        _ => None,
    }
}

fn has_elf_magic(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std_fs::File::open(path)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut magic))
        .is_ok()
        && &magic == b"\x7fELF"
}

fn is_kernel_module(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".ko"))
}

/// Every regular file under `dir`, found without following symlinks.
fn collect_tree_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std_fs::read_dir(dir).map_err(|error| {
        format!(
            "failed to read assembly tree '{}' for strip: {error}",
            dir.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read an entry of assembly tree '{}': {error}",
                dir.display()
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "failed to read the type of '{}' for strip: {error}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            collect_tree_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn hard_link_key(metadata: &std_fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn hard_link_key(_metadata: &std_fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Replaces `other` with a hard link to `first`, which was stripped in place
/// (the strip tool writes a new file, so the other names are now stale).
fn relink(first: &Path, other: &Path) -> Result<(), String> {
    std_fs::remove_file(other).map_err(|error| {
        format!(
            "failed to replace hard link '{}' after strip: {error}",
            other.display()
        )
    })?;
    std_fs::hard_link(first, other).map_err(|error| {
        format!(
            "failed to restore hard link '{}' to '{}' after strip: {error}",
            other.display(),
            first.display()
        )
    })
}

/// The ELF candidates of a `strip = true` tree: its regular files, sorted.
pub(super) fn tree_files(tree_path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_tree_files(tree_path, &mut files)?;
    files.sort();
    Ok(files)
}
