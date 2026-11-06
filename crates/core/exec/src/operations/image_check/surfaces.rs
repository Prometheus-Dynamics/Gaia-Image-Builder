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
//!
//! A plugin under one of a surface's `plugin_dirs` is opened at run time by
//! a process that loads the surface, so it is a consumer of that surface
//! whether or not it needs it by name. Its imports are compared with the
//! surface's, the surface's own libraries' and its own libraries' exports,
//! and leftovers are an error: nothing else is there to define them. A
//! plugin must also export each of the surface's `plugin_exports` (the
//! plugin ABI's entry points), or the surface cannot open it.

use super::kernel_modules::Outcome;
use gaia_artifact_providers::{dynamic_symbols, exported_u32};
use gaia_plan::{find_sysroot_library, parse_elf};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// The surface's `plugin_dirs`.
    pub plugin_dirs: Vec<String>,
    /// What each plugin there must export.
    pub plugin_exports: Vec<String>,
    /// The plugin ABI version's symbol and accepted range.
    pub plugin_version: Option<gaia_spec::SurfacePluginVersionSpec>,
}

/// The surfaces the build installs into `rootfs`: each install of a Rust
/// artifact with a declared surface. An install whose file is not in the
/// root filesystem (it went to another tree) is skipped.
pub(super) fn installed_surfaces(spec: &ResolvedBuildSpec, rootfs: &Path) -> Vec<InstalledSurface> {
    installed_rust(spec, rootfs)
        .into_iter()
        .filter_map(|(artifact, guest)| {
            let ArtifactDefinition::Rust(rust) = &artifact.definition else {
                return None;
            };
            let surface = rust.surface.as_ref()?;
            Some(InstalledSurface {
                artifact: artifact.id.as_str().to_string(),
                guest,
                plugin_dirs: surface.plugin_dirs.clone(),
                plugin_exports: surface.plugin_exports.clone(),
                plugin_version: surface.plugin_version.clone(),
            })
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

/// Whether the image path `guest` lies under one of `dirs`.
pub(super) fn in_plugin_dir(guest: &str, dirs: &[String]) -> bool {
    dirs.iter().any(|dir| {
        guest
            .strip_prefix(dir.trim_end_matches('/'))
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The ELF files under `dirs` in `rootfs`, as image paths.
pub(super) fn plugin_files(rootfs: &Path, dirs: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    for dir in dirs {
        walk(&rootfs.join(dir.trim_start_matches('/')), &mut files);
    }
    let mut guests = files
        .iter()
        .map(|path| format!("/{}", path.strip_prefix(rootfs).unwrap_or(path).display()))
        .collect::<Vec<_>>();
    guests.sort();
    guests.dedup();
    guests
}

/// For the std report: each surface's names (file name, `DT_SONAME`) and
/// the plugins under its `plugin_dirs`.
pub(super) fn plugins_by_name(
    surfaces: &[InstalledSurface],
    rootfs: &Path,
) -> BTreeMap<String, Vec<String>> {
    let mut plugins = BTreeMap::new();
    for surface in surfaces
        .iter()
        .filter(|surface| !surface.plugin_dirs.is_empty())
    {
        let files = plugin_files(rootfs, &surface.plugin_dirs);
        let mut names = Vec::new();
        if let Some(name) = Path::new(&surface.guest).file_name() {
            names.push(name.to_string_lossy().into_owned());
        }
        let soname = std::fs::read(rootfs.join(surface.guest.trim_start_matches('/')))
            .ok()
            .and_then(|bytes| parse_elf(&bytes).ok())
            .and_then(|object| object.soname);
        names.extend(soname);
        for name in names {
            plugins
                .entry(name)
                .or_insert_with(Vec::new)
                .extend(files.iter().cloned());
        }
    }
    plugins
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
    /// Its own `DT_NEEDED`: loaded before any plugin it opens.
    needed: Vec<String>,
    /// The plugin ABI range it opens, when `plugin_version` is set and both
    /// ends could be read.
    plugin_range: Option<(u32, u32)>,
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
            Ok((object, symbols, bytes))
        });
        match parsed {
            Ok((object, symbols, bytes)) => {
                let plugin_range = surface
                    .plugin_version
                    .as_ref()
                    .and_then(|version| plugin_range(surface, version, &bytes, outcome));
                let mut names = BTreeSet::new();
                if let Some(name) = Path::new(&surface.guest).file_name() {
                    names.insert(name.to_string_lossy().into_owned());
                }
                names.extend(object.soname);
                loaded.push(Loaded {
                    surface: surface.clone(),
                    names,
                    exports: symbols.exports,
                    needed: object.needed,
                    plugin_range,
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
        let plugin = loaded
            .iter()
            .any(|surface| in_plugin_dir(&guest, &surface.surface.plugin_dirs));
        let uses = loaded
            .iter()
            .filter(|surface| {
                in_plugin_dir(&guest, &surface.surface.plugin_dirs)
                    || object
                        .needed
                        .iter()
                        .any(|name| surface.names.contains(name))
            })
            .collect::<Vec<_>>();
        if uses.is_empty() {
            continue;
        }
        consumers += 1;
        let consumer = Consumer {
            guest: &guest,
            bytes: &bytes,
            object: &object,
            plugin,
        };
        check_consumer(&consumer, &uses, &loaded, rootfs, outcome);
    }
    consumers
}

struct Consumer<'a> {
    guest: &'a str,
    bytes: &'a [u8],
    object: &'a gaia_plan::ElfObject,
    /// Under a surface's `plugin_dirs`: opened by the surface's process.
    plugin: bool,
}

fn check_consumer(
    consumer: &Consumer<'_>,
    uses: &[&Loaded],
    loaded: &[Loaded],
    rootfs: &Path,
    outcome: &mut Outcome,
) {
    let Consumer {
        guest,
        bytes,
        object,
        plugin,
    } = *consumer;
    let symbols = match dynamic_symbols(bytes) {
        Ok(symbols) => symbols,
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
    if plugin {
        plugin_entry_points(guest, &symbols.exports, uses, outcome);
        plugin_version(guest, bytes, uses, outcome);
    }
    let imports = symbols.imports;
    let mut available = BTreeSet::new();
    for surface in uses {
        available.extend(surface.exports.iter().cloned());
    }
    // Every other library the loader brings in for it, breadth first.
    let mut unresolved = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queue = object.needed.iter().cloned().collect::<VecDeque<_>>();
    if plugin {
        // The surface's own libraries are in the process before it.
        for surface in uses {
            queue.extend(surface.needed.iter().cloned());
        }
    }
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
        "{guest} imports {} symbol(s) that neither surface {surfaces} nor its other libraries export: {}; {}",
        missing.len(),
        named(&missing),
        if plugin {
            "the plugin was built for another build of the surface, or needs a library the image does not ship"
        } else {
            "it was linked against another build of the surface"
        }
    );
    if object.interpreter.is_some() || plugin {
        outcome.error("surface_import_undeclared", Some(guest), message);
    } else {
        // A library: the program that loads it may define them.
        outcome.warning("surface_import_undeclared", Some(guest), message);
    }
}

/// A plugin without one of its surface's `plugin_exports` cannot be opened
/// through the surface (a plugin built for a static host only, say).
fn plugin_entry_points(
    guest: &str,
    exports: &BTreeSet<String>,
    uses: &[&Loaded],
    outcome: &mut Outcome,
) {
    for surface in uses
        .iter()
        .filter(|surface| in_plugin_dir(guest, &surface.surface.plugin_dirs))
    {
        let missing = surface
            .surface
            .plugin_exports
            .iter()
            .filter(|name| !exports.contains(*name))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            outcome.error(
                "surface_plugin_export_missing",
                Some(guest),
                format!(
                    "plugin {guest} does not export {} that surface {} ('{}') opens its plugins by; it cannot be loaded through the surface",
                    named(&missing),
                    surface.surface.guest,
                    surface.surface.artifact
                ),
            );
        }
    }
}

/// The range a surface opens: each end fixed, or read from the constant
/// the surface library exports. A missing or unreadable constant is an
/// error, once per surface, and its plugins' versions go unchecked.
fn plugin_range(
    surface: &InstalledSurface,
    version: &gaia_spec::SurfacePluginVersionSpec,
    bytes: &[u8],
    outcome: &mut Outcome,
) -> Option<(u32, u32)> {
    let mut end = |fixed: Option<u32>, symbol: &Option<String>| -> Option<u32> {
        if fixed.is_some() {
            return fixed;
        }
        let symbol = symbol.as_ref()?;
        let problem = match exported_u32(bytes, symbol) {
            Ok(Some(value)) => return Some(value),
            Ok(None) => "it does not export it".to_string(),
            Err(message) => message,
        };
        outcome.error(
            "surface_plugin_range_unreadable",
            Some(&surface.guest),
            format!(
                "surface {} ('{}') gives no plugin ABI bound {symbol}: {problem}; it was built before the surface exported its range, or plugin_version names another constant",
                surface.guest, surface.artifact
            ),
        );
        None
    };
    let min = end(version.min, &version.surface_min);
    let max = end(version.max, &version.surface_max);
    min.zip(max)
}

/// A plugin's ABI version outside the range its surface opens. A plugin
/// without the symbol (built before it existed) is left to the loader.
fn plugin_version(guest: &str, bytes: &[u8], uses: &[&Loaded], outcome: &mut Outcome) {
    for surface in uses
        .iter()
        .filter(|surface| in_plugin_dir(guest, &surface.surface.plugin_dirs))
    {
        let (Some(range), Some((min, max))) =
            (&surface.surface.plugin_version, surface.plugin_range)
        else {
            continue;
        };
        let opened_by = format!(
            "surface {} ('{}')",
            surface.surface.guest, surface.surface.artifact
        );
        match exported_u32(bytes, &range.symbol) {
            Ok(None) => {}
            Ok(Some(version)) if (min..=max).contains(&version) => {}
            Ok(Some(version)) => outcome.error(
                "surface_plugin_version_unsupported",
                Some(guest),
                format!(
                    "plugin {guest} has {} {version}, outside the {}..={} that {opened_by} opens; rebuild it against a supported ABI",
                    range.symbol, min, max
                ),
            ),
            Err(message) => outcome.error(
                "surface_plugin_version_unreadable",
                Some(guest),
                format!("plugin {guest}'s {} cannot be read: {message}", range.symbol),
            ),
        }
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
