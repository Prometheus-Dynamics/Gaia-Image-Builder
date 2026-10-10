//! The runtime closure of a dynamically linked BusyBox: its interpreter and
//! every library it needs, found under a sysroot (the target root the binary
//! was built for).
//!
//! Nothing here runs a host tool or reads the host's library paths. Names are
//! looked up under the sysroot only, following symlinks that stay inside it
//! (an absolute link target means the sysroot's root). Each library must have
//! the binary's class, byte order and machine, so a host library is never
//! taken for a target one.

use crate::elf::{ElfObject, parse_elf};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

/// Links followed while resolving one name, as the Linux loader allows.
const MAX_SYMLINK_HOPS: usize = 40;
/// Nested `include` files read from `ld.so.conf`.
const MAX_INCLUDE_DEPTH: usize = 8;
/// The default library directories, searched after the RUNPATH entries.
const DEFAULT_LIBRARY_DIRS: [&str; 4] = ["/lib", "/lib64", "/usr/lib", "/usr/lib64"];

/// One thing to place in the target root, at its absolute target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEntry {
    /// A regular file, copied from `source` (a path under the sysroot).
    File { guest: String, source: PathBuf },
    /// A symbolic link, recreated with its target text as the sysroot has it.
    Symlink { guest: String, target: String },
}

impl RuntimeEntry {
    /// The absolute path of the entry in the target root.
    pub fn guest(&self) -> &str {
        match self {
            Self::File { guest, .. } | Self::Symlink { guest, .. } => guest,
        }
    }
}

/// The files a dynamically linked binary needs at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeClosure {
    /// `Some` for a dynamic binary: the sysroot the entries came from.
    pub sysroot: Option<PathBuf>,
    /// Whether the binary is dynamically linked. A static binary has no
    /// entries.
    pub dynamic: bool,
    /// The interpreter (`PT_INTERP`) path the binary names.
    pub interpreter: Option<String>,
    /// Sorted by target path; each path appears once.
    pub entries: Vec<RuntimeEntry>,
}

/// The sysroot a BusyBox binary implies when none is configured: the
/// directory above its `bin/` or `sbin/`, or above `usr/bin/` or `usr/sbin/`.
/// `None` when the binary is not in one of those, or the sysroot would be the
/// host's root.
pub fn derive_sysroot(binary: &Path) -> Option<PathBuf> {
    let dir = binary.parent()?;
    let name = dir.file_name()?.to_str()?;
    if name != "bin" && name != "sbin" {
        return None;
    }
    let above = dir.parent()?;
    let sysroot = if above.file_name().is_some_and(|name| name == "usr") {
        above.parent()?
    } else {
        above
    };
    (sysroot.parent().is_some()).then(|| sysroot.to_path_buf())
}

/// Resolves the runtime closure of `binary` against `sysroot`, or against
/// the sysroot derived from the binary's path when `sysroot` is `None`.
pub fn resolve_runtime_closure(
    binary: &Path,
    sysroot: Option<&Path>,
) -> Result<RuntimeClosure, String> {
    let bytes = fs::read(binary)
        .map_err(|error| format!("failed to read busybox '{}': {error}", binary.display()))?;
    let object = parse_elf(&bytes).map_err(|error| {
        format!(
            "busybox '{}' is not an ELF executable: {error}",
            binary.display()
        )
    })?;
    if !object.is_dynamic() {
        return Ok(RuntimeClosure {
            sysroot: sysroot.map(Path::to_path_buf),
            dynamic: false,
            interpreter: None,
            entries: Vec::new(),
        });
    }

    let sysroot = match sysroot {
        Some(path) => path.to_path_buf(),
        None => derive_sysroot(binary).ok_or_else(|| {
            format!(
                "busybox '{}' is dynamically linked and its sysroot cannot be derived from its path (it must sit in bin/, sbin/, usr/bin/ or usr/sbin/ of the target root); set `sysroot`",
                binary.display()
            )
        })?,
    };
    if !sysroot.is_dir() {
        return Err(format!(
            "busybox sysroot '{}' does not exist or is not a directory",
            sysroot.display()
        ));
    }

    let mut resolver = Resolver::new(&sysroot, object.architecture())?;
    let mut queue = VecDeque::new();
    if let Some(interpreter) = &object.interpreter {
        resolver.add_interpreter(interpreter, &mut queue)?;
    }
    let binary_origin = guest_dir_of(binary, &sysroot);
    queue.push_back(Requester {
        label: binary.display().to_string(),
        origin: binary_origin,
        needed: object.needed.clone(),
        search_paths: object.search_paths.clone(),
    });
    resolver.run(&mut queue)?;
    if !resolver.missing.is_empty() {
        return Err(format!(
            "busybox '{}' needs runtime libraries that are not under sysroot '{}': {}",
            binary.display(),
            sysroot.display(),
            resolver
                .missing
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // The loader searches its own built-in directories (on aarch64 glibc,
    // /lib64 and /usr/lib64), not the ones the lookup above went through:
    // keep the sysroot's links between the library directories, so a
    // library placed under /usr/lib is also where the loader looks.
    let mut entries = resolver.entries;
    for dir in DEFAULT_LIBRARY_DIRS {
        let link = sysroot.join(dir.trim_start_matches('/'));
        if let Ok(target) = fs::read_link(&link) {
            entries
                .entry(dir.to_string())
                .or_insert_with(|| RuntimeEntry::Symlink {
                    guest: dir.to_string(),
                    target: target.to_string_lossy().into_owned(),
                });
        }
    }

    Ok(RuntimeClosure {
        sysroot: Some(sysroot.clone()),
        dynamic: true,
        interpreter: object.interpreter,
        entries: entries.into_values().collect(),
    })
}

/// An object whose own NEEDED names still have to be looked up.
struct Requester {
    /// What the object is called in an error message.
    label: String,
    /// The guest directory the object was loaded from, for `$ORIGIN`.
    origin: Option<String>,
    needed: Vec<String>,
    search_paths: Vec<String>,
}

struct Resolver<'a> {
    sysroot: &'a Path,
    architecture: (bool, bool, u16),
    default_dirs: Vec<String>,
    entries: BTreeMap<String, RuntimeEntry>,
    scanned: BTreeSet<String>,
    missing: BTreeSet<String>,
}

impl<'a> Resolver<'a> {
    fn new(sysroot: &'a Path, architecture: (bool, bool, u16)) -> Result<Self, String> {
        let mut default_dirs: Vec<String> = DEFAULT_LIBRARY_DIRS
            .iter()
            .map(|dir| (*dir).to_string())
            .collect();
        for base in ["/lib", "/usr/lib"] {
            default_dirs.extend(multiarch_dirs(sysroot, base)?);
        }
        let mut seen_conf = BTreeSet::new();
        parse_ld_so_conf(
            sysroot,
            "/etc/ld.so.conf",
            &mut default_dirs,
            0,
            &mut seen_conf,
        )?;
        Ok(Self {
            sysroot,
            architecture,
            default_dirs: dedupe(default_dirs),
            entries: BTreeMap::new(),
            scanned: BTreeSet::new(),
            missing: BTreeSet::new(),
        })
    }

    fn add_interpreter(
        &mut self,
        interpreter: &str,
        queue: &mut VecDeque<Requester>,
    ) -> Result<(), String> {
        if !interpreter.starts_with('/') {
            return Err(format!(
                "program interpreter '{interpreter}' is not an absolute path"
            ));
        }
        let walked = walk(self.sysroot, interpreter)?.ok_or_else(|| {
            format!(
                "program interpreter '{interpreter}' does not exist under sysroot '{}'",
                self.sysroot.display()
            )
        })?;
        if !self.matches_architecture(&walked.real)? {
            return Err(format!(
                "program interpreter '{interpreter}' under sysroot '{}' is not an ELF object for the busybox architecture",
                self.sysroot.display()
            ));
        }
        self.add_walked(&walked);
        self.scan(&walked.real, interpreter, parent_guest(interpreter), queue)
    }

    /// Looks up every NEEDED name, breadth first, until no new object is
    /// reached.
    fn run(&mut self, queue: &mut VecDeque<Requester>) -> Result<(), String> {
        while let Some(requester) = queue.pop_front() {
            for name in &requester.needed {
                if name.contains('/') {
                    if !name.starts_with('/') {
                        return Err(format!(
                            "'{}' needs '{name}' by a relative path, which is not supported",
                            requester.label
                        ));
                    }
                    self.locate_path(name, &requester, queue)?;
                } else if let Some(found) = self.locate(name, &requester)? {
                    self.add_walked(&found.walked);
                    self.scan(&found.walked.real, name, Some(found.dir), queue)?;
                } else {
                    self.missing
                        .insert(format!("{name} (needed by {})", requester.label));
                }
            }
        }
        Ok(())
    }

    /// A NEEDED name that is a path: looked up at that guest path.
    fn locate_path(
        &mut self,
        path: &str,
        requester: &Requester,
        queue: &mut VecDeque<Requester>,
    ) -> Result<(), String> {
        match walk(self.sysroot, path)? {
            Some(walked) if self.matches_architecture(&walked.real)? => {
                self.add_walked(&walked);
                self.scan(&walked.real, path, parent_guest(path), queue)
            }
            _ => {
                self.missing
                    .insert(format!("{path} (needed by {})", requester.label));
                Ok(())
            }
        }
    }

    /// The first library named `name` that the search order finds under the
    /// sysroot and that matches the architecture.
    fn locate(&self, name: &str, requester: &Requester) -> Result<Option<Found>, String> {
        let mut dirs = expand_search_paths(&requester.search_paths, requester.origin.as_deref());
        dirs.extend(self.default_dirs.iter().cloned());
        for dir in dedupe(dirs) {
            let candidate = join_guest(&dir, name);
            let Some(walked) = walk(self.sysroot, &candidate)? else {
                continue;
            };
            if self.matches_architecture(&walked.real)? {
                return Ok(Some(Found { walked, dir }));
            }
        }
        Ok(None)
    }

    /// Whether the real file at a guest path is a regular ELF object for the
    /// busybox architecture. A missing or non-matching file is `false`.
    fn matches_architecture(&self, real: &str) -> Result<bool, String> {
        let path = self.host_path(real);
        if !path.is_file() {
            return Ok(false);
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        Ok(parse_elf(&bytes).is_ok_and(|object| object.architecture() == self.architecture))
    }

    /// Records a walked path: each symlink it crossed, then the real file.
    fn add_walked(&mut self, walked: &Walked) {
        for (guest, target) in &walked.links {
            self.entries.insert(
                guest.clone(),
                RuntimeEntry::Symlink {
                    guest: guest.clone(),
                    target: target.clone(),
                },
            );
        }
        if !self.entries.contains_key(&walked.real) {
            let source = self.host_path(&walked.real);
            self.entries.insert(
                walked.real.clone(),
                RuntimeEntry::File {
                    guest: walked.real.clone(),
                    source,
                },
            );
        }
    }

    /// Reads the object's own dependencies, once per real file.
    fn scan(
        &mut self,
        real: &str,
        label: &str,
        origin: Option<String>,
        queue: &mut VecDeque<Requester>,
    ) -> Result<(), String> {
        if !self.scanned.insert(real.to_string()) {
            return Ok(());
        }
        let path = self.host_path(real);
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        let object: ElfObject = parse_elf(&bytes)
            .map_err(|error| format!("'{label}' is not an ELF object: {error}"))?;
        queue.push_back(Requester {
            label: label.to_string(),
            origin,
            needed: object.needed,
            search_paths: object.search_paths,
        });
        Ok(())
    }

    fn host_path(&self, guest: &str) -> PathBuf {
        self.sysroot.join(guest.trim_start_matches('/'))
    }
}

struct Found {
    walked: Walked,
    /// The guest search directory the name was found in.
    dir: String,
}

/// A guest path looked up under the sysroot.
struct Walked {
    /// The real file's absolute guest path, with no symlink in it.
    real: String,
    /// Each symlink crossed on the way: its absolute guest path and its
    /// target text, in lookup order.
    links: Vec<(String, String)>,
}

/// Looks `guest` up under `sysroot`, following symlinks that stay inside it.
/// `None` when a component is missing.
fn walk(sysroot: &Path, guest: &str) -> Result<Option<Walked>, String> {
    let mut pending: VecDeque<String> = guest
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    let mut real: Vec<String> = Vec::new();
    let mut links = Vec::new();
    let mut hops = 0;
    while let Some(part) = pending.pop_front() {
        // Empty parts come from the slashes of an absolute link target.
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            real.pop();
            continue;
        }
        let mut candidate = real.clone();
        candidate.push(part);
        let candidate_path = sysroot.join(candidate.join("/"));
        let Ok(metadata) = fs::symlink_metadata(&candidate_path) else {
            return Ok(None);
        };
        if !metadata.file_type().is_symlink() {
            real = candidate;
            continue;
        }
        hops += 1;
        if hops > MAX_SYMLINK_HOPS {
            return Err(format!(
                "too many symbolic links resolving '{guest}' under sysroot '{}'",
                sysroot.display()
            ));
        }
        let target = fs::read_link(&candidate_path)
            .map_err(|error| {
                format!(
                    "failed to read link '{}': {error}",
                    candidate_path.display()
                )
            })?
            .to_str()
            .ok_or_else(|| format!("link '{}' has a non-UTF-8 target", candidate_path.display()))?
            .to_string();
        links.push((format!("/{}", candidate.join("/")), target.clone()));
        if target.starts_with('/') {
            real.clear();
        }
        for part in target.split('/').rev() {
            pending.push_front(part.to_string());
        }
    }
    if real.is_empty() {
        return Ok(None);
    }
    let real = format!("/{}", real.join("/"));
    if !sysroot.join(real.trim_start_matches('/')).exists() {
        return Ok(None);
    }
    Ok(Some(Walked { real, links }))
}

/// Directories `<base>/<triplet>` that exist under the sysroot, such as
/// `/usr/lib/aarch64-linux-gnu`, in sorted order.
fn multiarch_dirs(sysroot: &Path, base: &str) -> Result<Vec<String>, String> {
    let Some(walked) = walk(sysroot, base)? else {
        return Ok(Vec::new());
    };
    let host = sysroot.join(walked.real.trim_start_matches('/'));
    let Ok(entries) = fs::read_dir(&host) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.contains("-linux-"))
        .collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| join_guest(base, &name))
        .collect())
}

/// Collects the directories of `ld.so.conf` and its `include` files, read
/// from the sysroot.
fn parse_ld_so_conf(
    sysroot: &Path,
    conf: &str,
    dirs: &mut Vec<String>,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Result<(), String> {
    if depth > MAX_INCLUDE_DEPTH || !seen.insert(conf.to_string()) {
        return Ok(());
    }
    let Some(walked) = walk(sysroot, conf)? else {
        return Ok(());
    };
    let path = sysroot.join(walked.real.trim_start_matches('/'));
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line
            .strip_prefix("include")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            for file in expand_include(sysroot, pattern.trim())? {
                parse_ld_so_conf(sysroot, &file, dirs, depth + 1, seen)?;
            }
        } else if line.starts_with('/') {
            dirs.push(line.to_string());
        }
    }
    Ok(())
}

/// The files an `include` pattern names. A relative pattern is relative to
/// `/etc`; `*` and `?` match within the last path component only.
fn expand_include(sysroot: &Path, pattern: &str) -> Result<Vec<String>, String> {
    let absolute = if pattern.starts_with('/') {
        pattern.to_string()
    } else {
        join_guest("/etc", pattern)
    };
    let (dir, last) = match absolute.rsplit_once('/') {
        Some((dir, last)) => (if dir.is_empty() { "/" } else { dir }, last),
        None => return Ok(Vec::new()),
    };
    if !last.contains('*') && !last.contains('?') {
        return Ok(vec![absolute]);
    }
    let Some(walked) = walk(sysroot, dir)? else {
        return Ok(Vec::new());
    };
    let Ok(entries) = fs::read_dir(sysroot.join(walked.real.trim_start_matches('/'))) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| wildcard_matches(last.as_bytes(), name.as_bytes()))
        .collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| join_guest(dir, &name))
        .collect())
}

/// `*` (any run) and `?` (one byte) matching of one file name.
fn wildcard_matches(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| wildcard_matches(rest, &name[skip..])),
        Some((b'?', rest)) => !name.is_empty() && wildcard_matches(rest, &name[1..]),
        Some((first, rest)) => name
            .split_first()
            .is_some_and(|(byte, tail)| byte == first && wildcard_matches(rest, tail)),
    }
}

/// The RUNPATH/RPATH entries that can be used: `$ORIGIN` is replaced by the
/// object's guest directory; an entry with another `$` token (such as
/// `$LIB`) or a relative path is skipped.
fn expand_search_paths(paths: &[String], origin: Option<&str>) -> Vec<String> {
    paths
        .iter()
        .filter_map(|entry| {
            let expanded = if entry.contains('$') {
                let origin = origin?;
                entry
                    .replace("${ORIGIN}", origin)
                    .replace("$ORIGIN", origin)
            } else {
                entry.clone()
            };
            (expanded.starts_with('/') && !expanded.contains('$'))
                .then(|| expanded.trim_end_matches('/').to_string())
                .map(|dir| if dir.is_empty() { "/".to_string() } else { dir })
        })
        .collect()
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn join_guest(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// The guest directory of an absolute guest path: `/lib/libc.so.6` gives `/lib`.
fn parent_guest(path: &str) -> Option<String> {
    match path.rsplit_once('/') {
        Some(("", _)) => Some("/".to_string()),
        Some((dir, _)) => Some(dir.to_string()),
        None => None,
    }
}

/// The guest directory of the BusyBox binary, when it sits under the sysroot.
fn guest_dir_of(binary: &Path, sysroot: &Path) -> Option<String> {
    let relative = binary.strip_prefix(sysroot).ok()?.parent()?;
    let joined = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?
        .join("/");
    Some(format!("/{joined}"))
}

#[cfg(test)]
#[path = "runtime_libs_tests.rs"]
mod tests;
