//! Surface consumers: a program in the root filesystem that loads a declared
//! surface library (a Rust artifact with `surface`, installed into the image)
//! imports from it only what it exports. The artifact build already held the
//! library to its declared list, so its exports here are the surface; a
//! consumer that needs anything else was linked against another build of it
//! and would fail to load (or bind a symbol the surface never promised).
//!
//! A consumer's imports are compared with the exports of the surfaces it
//! loads and of every other library it loads, breadth first, found where
//! the loader looks by default. When one of those is not in the image, the
//! consumer cannot be checked and is a warning. A library consumer's
//! leftover imports may come from the program that loads it, so they are a
//! warning too; a program's are an error.

use super::kernel_modules::Outcome;
use gaia_artifact_providers::dynamic_symbols;
use gaia_plan::{find_sysroot_library, parse_elf};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::collections::{BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};

/// How many symbols a finding names before it counts the rest.
const NAMED: usize = 8;

/// A surface library as installed into the root filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstalledSurface {
    pub artifact: String,
    /// Its path in the image, such as `/usr/lib/libacme_surface.so`.
    pub guest: String,
}

/// The surfaces the build installs into `rootfs`: each install of a Rust
/// artifact with a declared surface. An install whose file is not in the
/// root filesystem (it went to another tree) is skipped.
pub(super) fn installed_surfaces(spec: &ResolvedBuildSpec, rootfs: &Path) -> Vec<InstalledSurface> {
    installed_rust(spec, rootfs)
        .into_iter()
        .filter(|(artifact, _)| {
            matches!(&artifact.definition, ArtifactDefinition::Rust(rust) if rust.surface.is_some())
        })
        .map(|(artifact, guest)| InstalledSurface {
            artifact: artifact.id.as_str().to_string(),
            guest,
        })
        .collect()
}

/// Each install of a Rust artifact into `rootfs`, with its path in the
/// image; an install whose file is not there (it went to another tree) is
/// left out.
pub(super) fn installed_rust<'a>(
    spec: &'a ResolvedBuildSpec,
    rootfs: &Path,
) -> Vec<(&'a ArtifactSpec, String)> {
    let mut installed = Vec::new();
    for entry in &spec.install.entries {
        let Some(artifact) = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == entry.artifact.id)
        else {
            continue;
        };
        if !matches!(artifact.definition, ArtifactDefinition::Rust(_)) {
            continue;
        }
        let mut guest = format!("/{}", entry.dest.trim_start_matches('/'));
        if rootfs.join(guest.trim_start_matches('/')).is_dir()
            && let Some(name) = Path::new(&artifact.output.path).file_name()
        {
            guest = format!("{}/{}", guest.trim_end_matches('/'), name.to_string_lossy());
        }
        if rootfs.join(guest.trim_start_matches('/')).is_file() {
            installed.push((artifact, guest));
        }
    }
    installed
}

/// The names of every library `needed` brings in, breadth first, found
/// where the loader looks by default; and the names that were not found.
pub(super) fn needed_closure(
    rootfs: &Path,
    needed: &[String],
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut seen = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    let mut queue = needed.iter().cloned().collect::<VecDeque<_>>();
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let object = find_sysroot_library(rootfs, &name)
            .ok()
            .flatten()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| parse_elf(&bytes).ok());
        match object {
            Some(object) => queue.extend(object.needed),
            None => {
                unresolved.insert(name);
            }
        }
    }
    (seen, unresolved)
}

struct Loaded {
    surface: InstalledSurface,
    /// The names a consumer needs it by: its file name and its `DT_SONAME`.
    names: BTreeSet<String>,
    exports: BTreeSet<String>,
}

/// Checks every ELF file of `rootfs` that needs one of `surfaces`. Returns
/// how many consumers were checked.
pub(super) fn check(surfaces: &[InstalledSurface], rootfs: &Path, outcome: &mut Outcome) -> usize {
    let mut loaded = Vec::new();
    for surface in surfaces {
        let path = rootfs.join(surface.guest.trim_start_matches('/'));
        let read = std::fs::read(&path).map_err(|error| error.to_string());
        let parsed = read.and_then(|bytes| {
            let object = parse_elf(&bytes)?;
            let symbols = dynamic_symbols(&bytes)?;
            Ok((object, symbols))
        });
        match parsed {
            Ok((object, symbols)) => {
                let mut names = BTreeSet::new();
                if let Some(name) = Path::new(&surface.guest).file_name() {
                    names.insert(name.to_string_lossy().into_owned());
                }
                names.extend(object.soname);
                loaded.push(Loaded {
                    surface: surface.clone(),
                    names,
                    exports: symbols.exports,
                });
            }
            Err(message) => outcome.error(
                "surface_unreadable",
                Some(&surface.guest),
                format!(
                    "surface library {} of artifact '{}' cannot be read: {message}",
                    surface.guest, surface.artifact
                ),
            ),
        }
    }
    if loaded.is_empty() {
        return 0;
    }
    let mut files = Vec::new();
    walk(rootfs, &mut files);
    let mut consumers = 0;
    for path in files {
        let guest = format!("/{}", path.strip_prefix(rootfs).unwrap_or(&path).display());
        if loaded.iter().any(|surface| surface.surface.guest == guest) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(object) = parse_elf(&bytes) else {
            continue;
        };
        let uses = loaded
            .iter()
            .filter(|surface| {
                object
                    .needed
                    .iter()
                    .any(|name| surface.names.contains(name))
            })
            .collect::<Vec<_>>();
        if uses.is_empty() {
            continue;
        }
        consumers += 1;
        check_consumer(&guest, &bytes, &object, &uses, &loaded, rootfs, outcome);
    }
    consumers
}

fn check_consumer(
    guest: &str,
    bytes: &[u8],
    object: &gaia_plan::ElfObject,
    uses: &[&Loaded],
    loaded: &[Loaded],
    rootfs: &Path,
    outcome: &mut Outcome,
) {
    let imports = match dynamic_symbols(bytes) {
        Ok(symbols) => symbols.imports,
        Err(message) => {
            outcome.error(
                "surface_consumer_unreadable",
                Some(guest),
                format!(
                    "{guest} loads a surface library but its symbols cannot be read: {message}"
                ),
            );
            return;
        }
    };
    let mut available = BTreeSet::new();
    for surface in uses {
        available.extend(surface.exports.iter().cloned());
    }
    // Every other library the loader brings in for it, breadth first.
    let mut unresolved = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queue = object.needed.iter().cloned().collect::<VecDeque<_>>();
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(surface) = loaded.iter().find(|surface| surface.names.contains(&name)) {
            available.extend(surface.exports.iter().cloned());
            continue;
        }
        let library = match find_sysroot_library(rootfs, &name) {
            Ok(Some(path)) => path,
            _ => {
                unresolved.insert(name);
                continue;
            }
        };
        let Ok(library_bytes) = std::fs::read(&library) else {
            unresolved.insert(name);
            continue;
        };
        match (parse_elf(&library_bytes), dynamic_symbols(&library_bytes)) {
            (Ok(library_object), Ok(symbols)) => {
                queue.extend(library_object.needed);
                available.extend(symbols.exports);
            }
            _ => {
                unresolved.insert(name);
            }
        }
    }
    let missing = imports
        .iter()
        .filter(|import| !import.weak && !available.contains(&import.name))
        .map(|import| import.name.as_str())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return;
    }
    let surfaces = uses
        .iter()
        .map(|surface| format!("{} ('{}')", surface.surface.guest, surface.surface.artifact))
        .collect::<Vec<_>>()
        .join(", ");
    if !unresolved.is_empty() {
        outcome.warning(
            "surface_consumer_unchecked",
            Some(guest),
            format!(
                "{guest} loads surface {surfaces}, but its imports cannot be checked: {} not found under the default library directories",
                unresolved.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
        return;
    }
    let message = format!(
        "{guest} imports {} symbol(s) that neither surface {surfaces} nor its other libraries export: {}; it was linked against another build of the surface",
        missing.len(),
        named(&missing)
    );
    if object.interpreter.is_some() {
        outcome.error("surface_import_undeclared", Some(guest), message);
    } else {
        // A library: the program that loads it may define them.
        outcome.warning("surface_import_undeclared", Some(guest), message);
    }
}

fn named(names: &[&str]) -> String {
    let mut text = names
        .iter()
        .take(NAMED)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > NAMED {
        text.push_str(&format!(" and {} more", names.len() - NAMED));
    }
    text
}

/// Every regular file under `dir` that starts with the ELF magic, sorted;
/// symbolic links are not followed.
pub(super) fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk(&path, files);
        } else if metadata.is_file() && is_elf(&path) {
            files.push(path);
        }
    }
}

fn is_elf(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && magic == *b"\x7fELF"
}
