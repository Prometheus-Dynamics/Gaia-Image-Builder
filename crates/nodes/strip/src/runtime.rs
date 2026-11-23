//! `gaia.strip.runtime-libs`: a root without the toolchain runtime libraries
//! that nothing in it links.
//!
//! A prebuilt toolchain's runtime libraries are copied into the root whether
//! or not a program there uses them: Buildroot's external toolchain copies
//! each one its options select (`TOOLCHAIN_EXTERNAL_LIBS` in
//! `toolchain/toolchain-external/pkg-toolchain-external.mk`), and the
//! Bootlin toolchains select C++, Fortran and OpenMP. No config line turns
//! those off, so a root with no C++ program still carries libstdc++.
//!
//! This is not a sweep for unused libraries. A library can be loaded by name
//! at run time (`dlopen`) and appear in no `DT_NEEDED`, so only libraries on
//! a list are ever removed:
//!
//! - [`RUNTIME`], the compiler's language runtimes from that copy list, which
//!   a program gets only by being linked against them;
//! - the libraries the product names as `extra`, each with its reason.
//!
//! Some of the copy list's other entries are loaded by name, and can be
//! neither removed nor named: the loader and the C library, the name-service
//! modules, `libthread_db` (a debugger loads it), and `libgcc_s` (glibc loads
//! it to cancel a thread).
//!
//! A listed library is removed when no ELF file left in the root has it in
//! its `DT_NEEDED`: every ELF counts, a program, a library or a plugin,
//! wherever it is. That is decided in rounds until nothing more goes, so a
//! listed library that only another removed one needed goes too. The product
//! can `keep` a runtime by name, with its reason, when something loads it by
//! name.
//!
//! The report says each library removed, why, its files and their bytes, each
//! one that stays and what needs it, and the bytes saved.

use crate::elf::{self, Elf};
use crate::tree::{self, Kind};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

const ID: &str = "gaia.strip.runtime-libs";
const LIB_DIRS: [&str; 5] = ["lib/", "lib64/", "usr/lib/", "usr/lib64/", "usr/local/lib/"];

/// The toolchain runtime libraries removed when nothing links them: the
/// language runtimes of Buildroot's external toolchain copy list (C++,
/// Fortran and its quad-precision maths, OpenMP, D), and the compiler's
/// atomic and stack-protector support libraries. Each is reached only through
/// `DT_NEEDED`.
pub const RUNTIME: [&str; 8] = [
    "libatomic",
    "libgdruntime",
    "libgfortran",
    "libgomp",
    "libgphobos",
    "libquadmath",
    "libssp",
    "libstdc++",
];

/// Libraries that are loaded by name at run time, so `DT_NEEDED` says
/// nothing about them: never removed, and not accepted as `extra`.
const BY_NAME: [&str; 4] = ["ld-", "libc", "libgcc_s", "libthread_db"];

/// Why `name` cannot be an `extra` library, if it cannot.
pub fn refused(name: &str) -> Option<String> {
    if RUNTIME.contains(&name) {
        return Some(format!(
            "{name} is a toolchain runtime library, removed already when nothing links it"
        ));
    }
    let by_name = BY_NAME.contains(&name) || name.starts_with("ld-") || name.starts_with("libnss_");
    by_name.then(|| {
        format!(
            "{name} is loaded by name at run time, so no ELF's needed list says whether it is used"
        )
    })
}

/// Whether `name` is a library name as the lists take it: `lib` and a plain
/// stem, with no `.so` and no version.
pub fn is_name(name: &str) -> bool {
    name.len() > 3
        && name.starts_with("lib")
        && !name.contains(".so")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'.'))
}

/// One library a product names, and why.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Named {
    pub lib: String,
    pub reason: String,
}

/// The tree without the listed libraries nothing in it needs, and the report.
/// `extra` and `keep` are JSON lists of `{ "lib": ..., "reason": ... }`.
#[node(
    id = "gaia.strip.runtime-libs",
    inputs(store, tree, extra, keep),
    outputs(tree, report)
)]
pub fn runtime_libs(
    store: &str,
    tree: Tree,
    extra: Option<&String>,
    keep: Option<&String>,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let body = |why: String| StepError::body(ID, why);
    let store = Store::open(store).map_err(StepError::store(ID))?;
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| body("a tree that is not in the store".into()))?;
    let root = store.path(&object);
    let list = |name: &str, text: Option<&String>| -> Result<Vec<Named>, StepError> {
        match text {
            None => Ok(Vec::new()),
            Some(text) => serde_json::from_str(text).map_err(|e| body(format!("{name}: {e}"))),
        }
    };
    let (extra, keep) = (list("extra", extra)?, list("keep", keep)?);
    let show = |list: &[Named]| {
        list.iter()
            .map(|n| format!("{}\t{}", n.lib, n.reason))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let key = KeyBuilder::new(ID, 1)
        .input("tree", object.digest)
        .config("extra", show(&extra))
        .config("keep", show(&keep))
        .finish();
    let made = Step::new(&store, cx, ID, key).look_up_or_make(
        &[
            ("tree", Path::new("tree")),
            ("report", Path::new("report.txt")),
        ],
        |work| {
            let plan = plan(&root, &extra, &keep).map_err(body)?;
            let out = work.dir().join("tree");
            tree::copy(&root, &out).map_err(body)?;
            for path in &plan.removed {
                std::fs::remove_file(out.join(path)).map_err(|e| body(format!("{path}: {e}")))?;
            }
            std::fs::write(work.dir().join("report.txt"), &plan.report)
                .map_err(|e| body(e.to_string()))
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
    Ok((
        Tree {
            object: made.outputs["tree"].into(),
            entries,
        },
        Artifact {
            kind: ArtifactKind::parse("report"),
            object: made.outputs["report"].into(),
        },
    ))
}

/// What to remove from a root, and the report.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Paths to remove: each removed library's files and the links to them.
    removed: BTreeSet<String>,
    report: String,
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether the file name is one of library `lib`'s: `lib.so`, or `lib.so.`
/// and a version (its `-gdb.py` beside it included).
fn is_of(file: &str, lib: &str) -> bool {
    file.strip_prefix(lib)
        .and_then(|rest| rest.strip_prefix(".so"))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

fn plan(root: &Path, extra: &[Named], keep: &[Named]) -> Result<Plan, String> {
    for named in keep {
        if !RUNTIME.contains(&named.lib.as_str()) {
            return Err(format!(
                "keep {}: not a toolchain runtime library this step removes (those are {})",
                named.lib,
                RUNTIME.join(", ")
            ));
        }
    }
    for named in extra {
        if let Some(why) = refused(&named.lib) {
            return Err(format!("extra {}: {why}", named.lib));
        }
    }
    let entries = tree::walk(root)?;
    // Each ELF file's needed names, and each library's files by its name.
    let mut elfs: BTreeMap<String, Elf> = BTreeMap::new();
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for (relative, kind) in &entries {
        if *kind != Kind::File {
            continue;
        }
        let path = root.join(relative);
        let bytes = std::fs::read(&path).map_err(|e| format!("{relative}: {e}"))?;
        sizes.insert(relative.clone(), bytes.len() as u64);
        if let Some(elf) = elf::parse(&bytes).map_err(|e| format!("{relative}: {e}"))? {
            elfs.insert(relative.clone(), elf);
        }
    }
    // The listed libraries, in name order, with why each is listed.
    let mut listed: BTreeMap<&str, String> = RUNTIME
        .iter()
        .map(|lib| (*lib, "a toolchain runtime library".to_string()))
        .collect();
    for named in extra {
        listed.insert(
            &named.lib,
            format!("named by the product: {}", named.reason),
        );
    }
    let files_of = |lib: &str| -> Vec<&String> {
        entries
            .iter()
            .filter(|(path, kind)| {
                matches!(kind, Kind::File | Kind::Link)
                    && LIB_DIRS.iter().any(|dir| path.starts_with(dir))
                    && is_of(basename(path), lib)
            })
            .map(|(path, _)| path)
            .collect()
    };
    let files: BTreeMap<&str, Vec<&String>> =
        listed.keys().map(|lib| (*lib, files_of(lib))).collect();
    for named in extra {
        if files[named.lib.as_str()].is_empty() {
            return Err(format!(
                "extra {}: the root has no {}.so*; remove it from the list",
                named.lib, named.lib
            ));
        }
    }
    for named in keep {
        if files[named.lib.as_str()].is_empty() {
            return Err(format!(
                "keep {}: the root has no {}.so*; remove it from the list",
                named.lib, named.lib
            ));
        }
    }
    // The names a library answers to: its files' names and their SONAMEs.
    let names_of = |lib: &str| -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for path in &files[lib] {
            names.insert(basename(path).to_string());
            if let Some(soname) = elfs.get(*path).and_then(|elf| elf.soname.clone()) {
                names.insert(soname);
            }
        }
        names
    };
    // Rounds: a listed library goes when no ELF left in the root, outside
    // its own files, needs one of its names.
    let kept: BTreeMap<&str, &str> = keep
        .iter()
        .map(|n| (n.lib.as_str(), n.reason.as_str()))
        .collect();
    let mut gone: BTreeMap<&str, usize> = BTreeMap::new();
    let mut removed: BTreeSet<String> = BTreeSet::new();
    let needer = |lib: &str, removed: &BTreeSet<String>| -> Option<String> {
        let names = names_of(lib);
        let own: BTreeSet<&String> = files[lib].iter().copied().collect();
        elfs.iter()
            .filter(|(path, _)| !removed.contains(*path) && !own.contains(path))
            .find(|(_, elf)| elf.needed.iter().any(|name| names.contains(name)))
            .map(|(path, _)| path.clone())
    };
    for round in 1.. {
        let going: Vec<&str> = listed
            .keys()
            .copied()
            .filter(|lib| {
                !files[lib].is_empty()
                    && !gone.contains_key(lib)
                    && !kept.contains_key(lib)
                    && needer(lib, &removed).is_none()
            })
            .collect();
        if going.is_empty() {
            break;
        }
        for lib in going {
            gone.insert(lib, round);
            removed.extend(files[lib].iter().map(|path| (*path).clone()));
        }
    }
    let mut report = String::from(
        "# gaia.strip.runtime-libs: listed libraries no ELF file in the root needs are removed\n",
    );
    let mut saved = 0u64;
    for (lib, why) in &listed {
        if files[lib].is_empty() {
            continue;
        }
        if let Some(round) = gone.get(lib) {
            let bytes: u64 = files[lib].iter().filter_map(|path| sizes.get(*path)).sum();
            saved += bytes;
            let paths: Vec<&str> = files[lib].iter().map(|path| path.as_str()).collect();
            let after = if *round > 1 {
                format!(" (round {round}: only removed libraries needed it)")
            } else {
                String::new()
            };
            let _ = writeln!(
                report,
                "removed {lib}: {why}; no ELF in the root needs it{after}: {bytes} B in {}",
                paths.join(" ")
            );
        } else if let Some(reason) = kept.get(lib) {
            let _ = writeln!(report, "kept    {lib}: the product keeps it: {reason}");
        } else {
            let by = needer(lib, &removed).unwrap_or_default();
            let _ = writeln!(report, "kept    {lib}: needed by {by}");
        }
    }
    let _ = writeln!(
        report,
        "saved   {saved} B in {} file{}",
        removed.len(),
        if removed.len() == 1 { "" } else { "s" }
    );
    Ok(Plan { removed, report })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_librarys_files_are_its_so_names_and_versions() {
        assert!(is_of("libstdc++.so", "libstdc++"));
        assert!(is_of("libstdc++.so.6", "libstdc++"));
        assert!(is_of("libstdc++.so.6.0.34-gdb.py", "libstdc++"));
        assert!(!is_of("libstdc++.a", "libstdc++"));
        assert!(!is_of("libgpiodcxx.so.2", "libgpiod"));
        assert!(!is_of("libgomp.sox", "libgomp"));
    }

    #[test]
    fn names_are_plain_stems_and_by_name_libraries_are_refused() {
        assert!(is_name("libgpiodcxx"));
        assert!(is_name("libstdc++"));
        for bad in ["gpiod", "lib", "libx.so", "libx.so.1", "lib x", "lib/x"] {
            assert!(!is_name(bad), "{bad}");
        }
        assert!(refused("libgpiodcxx").is_none());
        for (name, says) in [
            ("libstdc++", "removed already"),
            ("libgcc_s", "loaded by name"),
            ("libnss_files", "loaded by name"),
            ("libthread_db", "loaded by name"),
            ("libc", "loaded by name"),
        ] {
            assert!(
                refused(name).is_some_and(|why| why.contains(says)),
                "{name}"
            );
        }
    }
}
