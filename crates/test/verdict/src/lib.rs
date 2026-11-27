//! A heavy test's pass, kept under the hash of everything it reads.
//!
//! The reproducibility cases (a toolchain's project built twice from
//! nothing, a stream encoded at several thread counts) take tens of
//! CPU-seconds each, by design: they share nothing between their builds.
//! Locally, a case whose inputs did not change since it passed has nothing
//! new to say. So such a case asks [`Verdict::reused`] first, and returns
//! when the same inputs passed before; it calls [`Verdict::passed`] after its
//! last assertion. CI never reuses: it runs every case on every push.
//!
//! The key is the hash of:
//! - **the test binary's code**: the sections it loads ([`code_hash`]).
//!   They hold the case's recipe, the pins it is compiled with (a toolchain
//!   release, an image digest), every line of Gaia it runs (the node, the
//!   store, the sandbox, the step kit), the external crates, and what the
//!   compiler made of them. A change to code the binary runs moves the key;
//!   a change the linker leaves out (a function nothing calls) moves only
//!   the symbols and debug information, which are not loaded, and not the
//!   key. A crate the binary does not link cannot move it either.
//! - **the host toolchains** the case runs, by identity: each tool's
//!   resolved path and what it says its version is.
//! - **the gate's files** the case runs under ([`GATE_FILES`]), and any file
//!   the case reads at run time, by content: under the repository (a script,
//!   a fixture) or anywhere by its absolute path (a kernel, firmware, a
//!   library put into an image).
//! - **pins** given as text, as tools are: an image digest, a release.
//! - **the case's name.**
//!
//! A binary built again from the same code may differ in its code; that
//! only runs the case again. Nothing that can change what the case does is
//! left out of the key: what is not in the binary is a tool or a file.
//!
//! The records are in `$GAIA_VERDICTS`, else `~/.cache/gaia/verdicts`, one
//! file per passed key. `GAIA_VERDICT_REUSE=0` turns reuse off, and so does
//! `GITHUB_ACTIONS`.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The gate's files a case runs under: the gate itself, the runner that
/// bounds each case, and nextest's configuration.
pub const GATE_FILES: &[&str] = &[
    "scripts/gate.sh",
    "scripts/test-bound.py",
    ".config/nextest.toml",
];

/// Where a case's key comes from and its records go.
#[derive(Debug, Clone)]
pub struct Place {
    /// The repository: [`GATE_FILES`] and a case's own files are under it.
    pub root: PathBuf,
    /// The test binary.
    pub exe: PathBuf,
    /// The record directory, or `None` when nothing may be reused.
    pub records: Option<PathBuf>,
}

impl Place {
    /// For the running test binary of the crate at `manifest_dir`, with the
    /// records and the reuse switch from the environment.
    pub fn from_env(manifest_dir: &str) -> Place {
        let mut root = PathBuf::from(manifest_dir);
        while !root.join("Cargo.lock").is_file() {
            if !root.pop() {
                root = PathBuf::from(manifest_dir);
                break;
            }
        }
        let off = std::env::var_os("GITHUB_ACTIONS").is_some()
            || std::env::var("GAIA_VERDICT_REUSE").is_ok_and(|v| v == "0");
        let records = if off {
            None
        } else if let Some(dir) = std::env::var_os("GAIA_VERDICTS") {
            Some(PathBuf::from(dir))
        } else {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/gaia/verdicts"))
        };
        Place {
            root,
            exe: std::env::current_exe().unwrap_or_default(),
            records,
        }
    }
}

/// A case's key, and whether it passed before.
#[derive(Debug)]
pub struct Verdict {
    case: String,
    key: String,
    records: Option<PathBuf>,
}

impl Verdict {
    /// The verdict of `case` in the running test binary of the crate at
    /// `manifest_dir` (pass `env!("CARGO_MANIFEST_DIR")`). `tools` are the
    /// host toolchains it runs, as (name, [`identity`]), and any pin as
    /// (name, text); `files` are files or directories it reads at run time,
    /// under the repository or by absolute path.
    pub fn new(
        case: &str,
        manifest_dir: &str,
        tools: &[(&str, String)],
        files: &[&str],
    ) -> Verdict {
        Verdict::at(case, &Place::from_env(manifest_dir), tools, files)
    }

    /// [`Verdict::new`] at a given place.
    pub fn at(case: &str, place: &Place, tools: &[(&str, String)], files: &[&str]) -> Verdict {
        let mut hasher = Sha256::new();
        let mut part = |label: &str, bytes: &[u8]| {
            hasher.update((label.len() as u64).to_le_bytes());
            hasher.update(label.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        part("verdict v2", b"");
        part("case", case.as_bytes());
        part("binary", binary_hash(&place.exe).as_bytes());
        let mut tools: Vec<_> = tools.to_vec();
        tools.sort();
        for (name, identity) in &tools {
            part(&format!("tool {name}"), identity.as_bytes());
        }
        let mut files: Vec<&str> = GATE_FILES
            .iter()
            .copied()
            .chain(files.iter().copied())
            .collect();
        files.sort_unstable();
        files.dedup();
        for file in files {
            part(
                &format!("file {file}"),
                content_hash(&place.root.join(file)).as_bytes(),
            );
        }
        Verdict {
            case: case.to_string(),
            key: hex(&hasher.finalize()),
            records: place.records.clone(),
        }
    }

    /// The key: what the case reads, hashed.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn record(&self) -> Option<PathBuf> {
        let name: String = self
            .case
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.records
            .as_ref()
            .map(|dir| dir.join(name).join(&self.key))
    }

    /// Whether this case passed before with this key, where reuse is on.
    /// Says so when it did: the case is then not run.
    pub fn reused(&self) -> bool {
        let reused = self.record().is_some_and(|record| record.is_file());
        if reused {
            println!(
                "verdict reused: {} passed with key {} (its binary, tools and files are as they were); CI runs it",
                self.case,
                &self.key[..12]
            );
        }
        reused
    }

    /// Records that the case passed: call it after the last assertion. A
    /// record that cannot be written costs only the next run.
    pub fn passed(&self) {
        let Some(record) = self.record() else { return };
        let Some(dir) = record.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = dir.join(format!(".{}.{}", self.key, std::process::id()));
        if std::fs::write(&temp, "passed\n").is_ok() && std::fs::rename(&temp, &record).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// A case that has begun ([`Verdict::begin`]): when it is dropped by the
/// test returning, which is the test passing, the pass is recorded; when it
/// is dropped by a panic, which is the test failing, nothing is.
#[derive(Debug)]
pub struct Pass(Verdict);

impl Drop for Pass {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.0.passed();
        }
    }
}

impl Verdict {
    /// `None` when this key passed before (the case returns); otherwise the
    /// [`Pass`] to hold until the test ends: `let Some(_pass) =
    /// verdict.begin() else { return };`. For a case with early returns
    /// before its checks are done, use [`Verdict::reused`] and
    /// [`Verdict::passed`] instead.
    pub fn begin(self) -> Option<Pass> {
        if self.reused() {
            None
        } else {
            Some(Pass(self))
        }
    }
}

/// A tool's identity: its resolved path (a bare name is looked up on
/// `PATH`) and what `args` make it print (its version), on both streams. A
/// tool that cannot run says so; that is an identity too. The command runs
/// twice: output that differs between the two (a tool that prints the time
/// or its process id, as one given an option it does not know may) would
/// move the key on every run, so such a tool is named by its executable's
/// content instead.
pub fn identity(program: &Path, args: &[&str]) -> String {
    let found = if program.components().count() == 1 {
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(program))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| program.to_path_buf())
    } else {
        program.to_path_buf()
    };
    let resolved = std::fs::canonicalize(&found).unwrap_or(found);
    let said = || match Command::new(program).args(args).output() {
        Ok(out) => format!(
            "{}\n{}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(error) => format!("cannot run: {error}"),
    };
    let (first, second) = (said(), said());
    if first == second {
        format!("{}\n{first}", resolved.display())
    } else {
        format!(
            "{}\nits output varies; its content: {}",
            resolved.display(),
            content_hash(&resolved)
        )
    }
}

/// [`code_hash`] of a test binary, once per process for each version of
/// the file: every keyed case in one binary hashes the same tens of
/// megabytes. The file's device, inode, size and change time name the
/// version; a file written again gets a new change time and is hashed again.
fn binary_hash(path: &Path) -> String {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Mutex, OnceLock};
    /// The path, device, inode, size and change time (s, ns).
    type Version = (PathBuf, u64, u64, u64, i64, i64);
    static HASHES: OnceLock<Mutex<HashMap<Version, String>>> = OnceLock::new();
    let Ok(meta) = std::fs::metadata(path) else {
        return code_hash(path);
    };
    let version = (
        path.to_path_buf(),
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.ctime(),
        meta.ctime_nsec(),
    );
    let hashes = HASHES.get_or_init(Default::default);
    if let Some(hash) = hashes.lock().expect("hashes").get(&version) {
        return hash.clone();
    }
    let hash = code_hash(path);
    hashes.lock().expect("hashes").insert(version, hash.clone());
    hash
}

/// The hash of what an executable loads: for a 64-bit little-endian ELF
/// file, its entry point, its program headers, and each section it maps
/// into memory, by name, address, flags and bytes (a section with no bytes
/// in the file, by its size), the build id left out. Symbols, debug
/// information and comments are not loaded, and are not in it. Any other
/// file, and an ELF file that does not read whole (truncated, or with a
/// header out of bounds), is hashed by its whole content instead: never by
/// less than it holds.
pub fn code_hash(path: &Path) -> String {
    match std::fs::read(path).ok().and_then(|bytes| elf_code(&bytes)) {
        Some(hash) => format!("elf code {hash}"),
        None => format!("content {}", content_hash(path)),
    }
}

fn elf_code(elf: &[u8]) -> Option<String> {
    const SHF_ALLOC: u64 = 0x2;
    const SHT_NOBITS: u32 = 8;
    let bytes = |at: usize, len: usize| elf.get(at..at.checked_add(len)?);
    let u16_at = |at: usize| bytes(at, 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |at: usize| bytes(at, 4).map(|b| u32::from_le_bytes(b.try_into().expect("4")));
    let u64_at = |at: usize| bytes(at, 8).map(|b| u64::from_le_bytes(b.try_into().expect("8")));
    let usize_at = |at: usize| u64_at(at).and_then(|v| usize::try_from(v).ok());
    // 64-bit, little-endian, version 1.
    if bytes(0, 7)? != b"\x7fELF\x02\x01\x01" {
        return None;
    }
    let (entry, phoff, shoff) = (u64_at(0x18)?, usize_at(0x20)?, usize_at(0x28)?);
    let (phentsize, phnum) = (u16_at(0x36)?, u16_at(0x38)?);
    let (shentsize, shnum, shstrndx) = (u16_at(0x3a)?, u16_at(0x3c)?, u16_at(0x3e)?);
    // Extended numbering (0 sections, or an index past the reserved range)
    // is not read: the whole content stands instead.
    if shentsize != 64 || shnum == 0 || shstrndx >= shnum || phentsize != 56 {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(entry.to_le_bytes());
    hasher.update(bytes(phoff, phentsize.checked_mul(phnum)?)?);
    let header = |index: usize| bytes(shoff.checked_add(index.checked_mul(64)?)?, 64);
    let names = header(shstrndx)?;
    let names = bytes(
        usize::try_from(u64::from_le_bytes(names[24..32].try_into().ok()?)).ok()?,
        usize::try_from(u64::from_le_bytes(names[32..40].try_into().ok()?)).ok()?,
    )?;
    for index in 0..shnum {
        let at = shoff.checked_add(index.checked_mul(64)?)?;
        header(index)?;
        let (name, kind, flags) = (u32_at(at)? as usize, u32_at(at + 4)?, u64_at(at + 8)?);
        let (addr, offset, size) = (u64_at(at + 16)?, usize_at(at + 24)?, usize_at(at + 32)?);
        let name = names.get(name..)?;
        let name = &name[..name.iter().position(|&b| b == 0)?];
        if flags & SHF_ALLOC == 0 || name == b".note.gnu.build-id" {
            continue;
        }
        let content = if kind == SHT_NOBITS {
            &[][..]
        } else {
            bytes(offset, size)?
        };
        for field in [
            name,
            &flags.to_le_bytes(),
            &addr.to_le_bytes(),
            &(size as u64).to_le_bytes(),
            content,
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
    }
    Some(hex(&hasher.finalize()))
}

/// sha256 of a file's bytes, or of a directory's sorted entries (name, kind,
/// bytes or link target); `missing` when there is nothing there.
fn content_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    if add(&mut hasher, path, "").is_err() {
        return "missing".to_string();
    }
    hex(&hasher.finalize())
}

fn add(hasher: &mut Sha256, path: &Path, name: &str) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    hasher.update(name.as_bytes());
    hasher.update([0]);
    if meta.file_type().is_symlink() {
        hasher.update(b"l");
        hasher.update(std::fs::read_link(path)?.as_os_str().as_encoded_bytes());
    } else if meta.is_dir() {
        hasher.update(b"d");
        let mut entries: Vec<_> = std::fs::read_dir(path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        entries.sort();
        for entry in entries {
            let child = format!("{name}/{}", entry.to_string_lossy());
            add(hasher, &path.join(&entry), &child)?;
        }
    } else {
        hasher.update(b"f");
        let mut file = std::fs::File::open(path)?;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = std::io::Read::read(&mut file, &mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    hasher.update([0]);
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
