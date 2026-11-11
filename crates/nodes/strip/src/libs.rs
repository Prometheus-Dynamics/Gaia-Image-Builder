//! `gaia.strip.unused-libs` and `gaia.strip.prune-libs`: the shared libraries
//! of a tree that no program in it needs.
//!
//! Programs are the tree's ELF files that are not libraries. The names they
//! need (`DT_NEEDED`, and the interpreter's file name) are looked up in the
//! library directories (`lib/`, `lib64/`, `usr/lib/`, `usr/lib64/`,
//! `usr/local/lib/`) by file name and by SONAME. A symlink there leads to its
//! file inside the tree. Each library found is needed, and so is what it
//! needs, in turn.
//!
//! A library is a `lib*.so*` shared object in a library directory. Anything
//! else (a Python extension, a plugin, a program that sits in a library
//! directory) is never a candidate. What no `DT_NEEDED` names is unused. A
//! library a program loads with `dlopen` is in no `DT_NEEDED`, so it is
//! unused too: the report says so, and pruning removes it. `RUNPATH` and
//! `RPATH` are not searched; a name the library directories do not hold is
//! listed as missing and changes nothing.
//!
//! The report node writes the list. The prune node removes what it lists,
//! with the symlinks that lead to it, and gives the tree without them. Which
//! one a graph uses is a declared choice.

use crate::elf::{self, ET_DYN, Elf};
use crate::tree::{self, Kind};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const REPORT: &str = "gaia.strip.unused-libs";
const PRUNE: &str = "gaia.strip.prune-libs";
const LIB_DIRS: [&str; 5] = ["lib/", "lib64/", "usr/lib/", "usr/lib64/", "usr/local/lib/"];

/// The tree's shared libraries nothing needs, as a report: one line each
/// for a library used, unused or missing.
#[node(id = "gaia.strip.unused-libs", inputs(store, tree), outputs(report))]
pub fn unused_libs(store: &str, tree: Tree, cx: &Ctx) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(REPORT))?;
    let root = source(&store, &tree, REPORT)?;
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| body(REPORT, "a tree that is not in the store"))?;
    let key = KeyBuilder::new(REPORT, 1)
        .input("tree", object.digest)
        .finish();
    let made = Step::new(&store, cx, REPORT, key).look_up_or_make(
        &[("report", Path::new("report.txt"))],
        |work| {
            let links = analyse(&root).map_err(|e| body(REPORT, e))?;
            std::fs::write(work.dir().join("report.txt"), report_text(&links))
                .map_err(|e| body(REPORT, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("report"),
        object: made.outputs["report"].into(),
    })
}

/// The tree without its unused shared libraries, and the symlinks that led
/// to them. Entries for removed paths are dropped.
#[node(id = "gaia.strip.prune-libs", inputs(store, tree), outputs(tree))]
pub fn prune_libs(store: &str, tree: Tree, cx: &Ctx) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(PRUNE))?;
    let root = source(&store, &tree, PRUNE)?;
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| body(PRUNE, "a tree that is not in the store"))?;
    let key = KeyBuilder::new(PRUNE, 1)
        .input("tree", object.digest)
        .finish();
    let made = Step::new(&store, cx, PRUNE, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let out = work.dir().join("tree");
            tree::copy(&root, &out).map_err(|e| body(PRUNE, e))?;
            let links = analyse(&root).map_err(|e| body(PRUNE, e))?;
            for path in removals(&links) {
                std::fs::remove_file(out.join(&path))
                    .map_err(|e| body(PRUNE, format!("{path}: {e}")))?;
            }
            Ok(())
        },
    )?;
    let placed = store.path(&made.outputs["tree"]);
    let entries = tree
        .entries
        .into_iter()
        .filter(|entry| {
            let path = entry.path.trim_start_matches('/').trim_end_matches('/');
            std::fs::symlink_metadata(placed.join(path)).is_ok()
        })
        .collect();
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries,
    })
}

fn body(step: &str, why: impl std::fmt::Display) -> StepError {
    StepError::body(step, why)
}

fn source(store: &Store, tree: &Tree, step: &str) -> Result<PathBuf, StepError> {
    tree.object
        .path_in(store)
        .ok_or_else(|| body(step, "a tree that is not in the store"))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn in_lib_dir(path: &str) -> bool {
    LIB_DIRS.iter().any(|dir| path.starts_with(dir))
}

/// A library: `lib*.so*`, a shared object, in a library directory.
fn is_candidate(path: &str, elf: &Elf) -> bool {
    let name = basename(path);
    in_lib_dir(path) && name.starts_with("lib") && name.contains(".so") && elf.kind == ET_DYN
}

/// What the tree's dynamic links say.
#[derive(Debug, Default)]
struct Links {
    /// Every candidate library.
    candidates: BTreeSet<String>,
    /// Each candidate a program needs, with the first path that needs it.
    used: BTreeMap<String, String>,
    /// Each name the library directories lack, with the first path needing it.
    missing: BTreeMap<String, String>,
    /// Each symlink in a library directory, with the file it leads to in the
    /// tree (none when it dangles).
    symlinks: BTreeMap<String, Option<String>>,
}

/// The candidate libraries nothing needs.
fn unused(links: &Links) -> BTreeSet<String> {
    links
        .candidates
        .iter()
        .filter(|lib| !links.used.contains_key(*lib))
        .cloned()
        .collect()
}

/// The paths to remove when pruning: every unused library, and every symlink
/// in a library directory that leads to one.
fn removals(links: &Links) -> BTreeSet<String> {
    let unused = unused(links);
    let mut removed = unused.clone();
    for (link, target) in &links.symlinks {
        if target.as_ref().is_some_and(|t| unused.contains(t)) {
            removed.insert(link.clone());
        }
    }
    removed
}

fn report_text(links: &Links) -> String {
    let mut text = String::from(
        "# gaia.strip.unused-libs: shared libraries no program in the tree needs\n\
         # a library a program loads with dlopen is in no DT_NEEDED, so it shows as unused\n",
    );
    for (lib, by) in &links.used {
        let _ = writeln!(text, "used {lib} by {by}");
    }
    for lib in unused(links) {
        let _ = writeln!(text, "unused {lib}");
    }
    for (name, by) in &links.missing {
        let _ = writeln!(text, "missing {name} needed by {by}");
    }
    text
}

/// Reads the tree at `root` and works out its library links.
fn analyse(root: &Path) -> Result<Links, String> {
    let entries = tree::walk(root)?;
    let kinds: BTreeMap<&str, Kind> = entries.iter().map(|(p, k)| (p.as_str(), *k)).collect();
    let mut elfs: BTreeMap<String, Elf> = BTreeMap::new();
    for (relative, kind) in &entries {
        if *kind != Kind::File {
            continue;
        }
        let path = root.join(relative);
        let bytes = std::fs::read(&path).map_err(|e| format!("{relative}: {e}"))?;
        if let Some(elf) = elf::parse(&bytes).map_err(|e| format!("{relative}: {e}"))? {
            elfs.insert(relative.clone(), elf);
        }
    }

    // Names a library directory holds: a file or symlink by its file name,
    // a library also by its SONAME.
    let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut symlinks = BTreeMap::new();
    for (relative, kind) in &entries {
        if !in_lib_dir(relative) {
            continue;
        }
        match kind {
            Kind::Link => {
                by_name
                    .entry(basename(relative).to_string())
                    .or_default()
                    .insert(relative.clone());
                symlinks.insert(relative.clone(), resolve(root, &kinds, relative)?);
            }
            Kind::File if elfs.contains_key(relative) => {
                by_name
                    .entry(basename(relative).to_string())
                    .or_default()
                    .insert(relative.clone());
            }
            _ => {}
        }
    }
    for (relative, elf) in &elfs {
        if let Some(soname) = elf.soname.as_ref().filter(|_| in_lib_dir(relative)) {
            by_name
                .entry(soname.clone())
                .or_default()
                .insert(relative.clone());
        }
    }

    let candidates: BTreeSet<String> = elfs
        .iter()
        .filter(|(path, elf)| is_candidate(path, elf))
        .map(|(path, _)| path.clone())
        .collect();
    let mut links = Links {
        candidates,
        symlinks,
        ..Links::default()
    };

    // Programs first: every ELF that is not a candidate. Then the libraries
    // they reach, in the order they are reached.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for path in elfs.keys() {
        if !links.candidates.contains(path) {
            seen.insert(path.clone());
            queue.push_back(path.clone());
        }
    }
    while let Some(object) = queue.pop_front() {
        let elf = &elfs[&object];
        let mut names = elf.needed.clone();
        if let Some(interpreter) = &elf.interpreter {
            names.push(basename(interpreter).to_string());
        }
        for name in names {
            let found: BTreeSet<String> = by_name
                .get(&name)
                .into_iter()
                .flatten()
                .filter_map(|path| match kinds.get(path.as_str()) {
                    Some(Kind::Link) => links.symlinks.get(path).cloned().flatten(),
                    _ => Some(path.clone()),
                })
                .collect();
            if found.is_empty() {
                links.missing.entry(name).or_insert_with(|| object.clone());
                continue;
            }
            for lib in found {
                if links.candidates.contains(&lib) {
                    links
                        .used
                        .entry(lib.clone())
                        .or_insert_with(|| object.clone());
                }
                if elfs.contains_key(&lib) && seen.insert(lib.clone()) {
                    queue.push_back(lib);
                }
            }
        }
    }
    Ok(links)
}

/// The file a path in the tree leads to, following symlinks inside the tree.
/// `Ok(None)` when it dangles or leaves the tree.
fn resolve(
    root: &Path,
    kinds: &BTreeMap<&str, Kind>,
    relative: &str,
) -> Result<Option<String>, String> {
    let mut current = relative.to_string();
    for _ in 0..40 {
        match kinds.get(current.as_str()) {
            Some(Kind::File) => return Ok(Some(current)),
            Some(Kind::Link) => {
                let target = std::fs::read_link(root.join(&current))
                    .map_err(|e| format!("{current}: {e}"))?;
                let target = target.to_string_lossy();
                let mut parts: Vec<&str> = if target.starts_with('/') {
                    Vec::new()
                } else {
                    current
                        .rsplit_once('/')
                        .map_or(Vec::new(), |(dir, _)| dir.split('/').collect())
                };
                for part in target.trim_start_matches('/').split('/') {
                    match part {
                        "" | "." => {}
                        ".." => {
                            if parts.pop().is_none() {
                                return Ok(None);
                            }
                        }
                        name => parts.push(name),
                    }
                }
                current = parts.join("/");
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}
