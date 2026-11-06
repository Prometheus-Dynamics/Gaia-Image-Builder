//! The copies of the Rust standard library each program of the root
//! filesystem runs with: its own and those of the libraries it loads,
//! breadth first through `DT_NEEDED`. Reported, not checked. A program that
//! links a Rust library statically and also loads a `cdylib` built from Rust
//! carries std twice (about 245 KB of `lemnos-capi`'s 326 KB of text): a
//! cost, and a sign that a core crate may be in the process twice.
//!
//! An object has its own copy when it holds std's panic locations
//! (`library/std/src/`) and does not need a shared `libstd-*.so`: an object
//! linked with `-C prefer-dynamic` holds some of them too, from the generic
//! std code compiled into it. The shared `libstd-*.so` is one copy itself.
//! Libraries a program opens at run time (`dlopen`) are not seen, except
//! the plugins of a surface's `plugin_dirs`, counted in each program that
//! loads the surface.

use gaia_plan::{find_sysroot_library, parse_elf};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// What an object holding std's code contains: a path of one of its panic
/// locations.
const STD_MARKER: &[u8] = b"library/std/src/";

/// A program with more than one copy, and where they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Duplicated {
    pub program: String,
    pub copies: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct StdCopies {
    /// Programs that run with any copy of std.
    pub rust_programs: usize,
    pub duplicated: Vec<Duplicated>,
}

impl StdCopies {
    pub(super) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "rust_programs": self.rust_programs,
            "duplicated": self
                .duplicated
                .iter()
                .map(|process| serde_json::json!({
                    "program": process.program,
                    "copies": process.copies,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// The run summary's words for it, when a program has more than one.
    pub(super) fn summary(&self) -> Option<String> {
        let first = self.duplicated.first()?;
        Some(format!(
            "{} program(s) run with more than one copy of the Rust standard library, such as {} ({})",
            self.duplicated.len(),
            first.program,
            first.copies.join(", ")
        ))
    }
}

struct Object {
    needed: Vec<String>,
    /// Holds a copy of std: its own, or it is the shared `libstd-*.so`.
    copy: bool,
}

/// Every program of `rootfs` and its copies of std. `plugins` maps a
/// library's name (file name or `DT_SONAME`) to the plugins it opens.
pub(super) fn copies(rootfs: &Path, plugins: &BTreeMap<String, Vec<String>>) -> StdCopies {
    let mut files = Vec::new();
    super::surfaces::walk(rootfs, &mut files);
    let mut objects: BTreeMap<PathBuf, Option<Object>> = BTreeMap::new();
    let mut libraries: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    let mut result = StdCopies::default();
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(object) = parse_elf(&bytes) else {
            continue;
        };
        if object.interpreter.is_none() && !is_executable(&bytes, object.little_endian) {
            // A library: read when a program loads it.
            continue;
        }
        let program = Object {
            copy: holds_copy(&path, &object.needed, &bytes),
            needed: object.needed,
        };
        let mut copies = BTreeSet::new();
        if program.copy {
            copies.insert(guest(rootfs, &path));
        }
        let mut seen = BTreeSet::new();
        let mut queue = program.needed.iter().cloned().collect::<VecDeque<_>>();
        while let Some(name) = queue.pop_front() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let library = libraries
                .entry(name.clone())
                .or_insert_with(|| find_sysroot_library(rootfs, &name).ok().flatten())
                .clone();
            let Some(library) = library else {
                continue;
            };
            let loaded = objects
                .entry(library.clone())
                .or_insert_with(|| read_object(&library));
            if let Some(loaded) = loaded {
                if loaded.copy {
                    copies.insert(guest(rootfs, &library));
                }
                queue.extend(loaded.needed.iter().cloned());
            }
            for plugin in plugins.get(&name).into_iter().flatten() {
                let path = rootfs.join(plugin.trim_start_matches('/'));
                let opened = objects
                    .entry(path.clone())
                    .or_insert_with(|| read_object(&path));
                if let Some(opened) = opened {
                    if opened.copy {
                        copies.insert(plugin.clone());
                    }
                    queue.extend(opened.needed.iter().cloned());
                }
            }
        }
        if copies.is_empty() {
            continue;
        }
        result.rust_programs += 1;
        if copies.len() > 1 {
            result.duplicated.push(Duplicated {
                program: guest(rootfs, &path),
                copies: copies.into_iter().collect(),
            });
        }
    }
    result
}

fn read_object(path: &Path) -> Option<Object> {
    let bytes = std::fs::read(path).ok()?;
    let object = parse_elf(&bytes).ok()?;
    Some(Object {
        copy: holds_copy(path, &object.needed, &bytes),
        needed: object.needed,
    })
}

/// `e_type` is `ET_EXEC`: a program without an interpreter, linked
/// statically (a musl Rust binary).
fn is_executable(bytes: &[u8], little_endian: bool) -> bool {
    let Some(&[a, b]) = bytes.get(16..18) else {
        return false;
    };
    let e_type = if little_endian {
        u16::from_le_bytes([a, b])
    } else {
        u16::from_be_bytes([a, b])
    };
    e_type == 2
}

fn holds_copy(path: &Path, needed: &[String], bytes: &[u8]) -> bool {
    if is_shared_std(path.file_name().and_then(|name| name.to_str())) {
        return true;
    }
    !needed.iter().any(|name| is_shared_std(Some(name.as_str())))
        && memchr::memmem::find(bytes, STD_MARKER).is_some()
}

fn is_shared_std(name: Option<&str>) -> bool {
    name.is_some_and(|name| name.starts_with("libstd-") && name.ends_with(".so"))
}

fn guest(rootfs: &Path, path: &Path) -> String {
    format!("/{}", path.strip_prefix(rootfs).unwrap_or(path).display())
}
