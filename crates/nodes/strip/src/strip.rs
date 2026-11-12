//! `gaia.strip.elf`: the ELF files of a tree, stripped with the tool it names.
//!
//! Executables and shared objects get `--strip-unneeded`, which keeps what
//! the dynamic loader reads. Relocatable objects (kernel modules too) keep
//! their symbols, so they get `--strip-debug`. Other ELF kinds are left
//! alone, and so is every file that is not ELF. Symlinks are not touched.
//!
//! The tool is a pinned path, its `--version` is in the key, and it runs in
//! the step's output directory on the copy, never on the stored tree. Paths
//! and modes are unchanged, so the output tree keeps the input's entries.

use crate::elf::{self, ET_DYN, ET_EXEC, ET_REL};
use crate::tree::{self, Kind};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::Tree;
use gaia_types::owned_digest;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const NAME: &str = "gaia.strip.elf";
const STRIP_UNNEEDED: &str = "--strip-unneeded";
const STRIP_DEBUG: &str = "--strip-debug";
/// Files per tool run: one run per chunk, not per file.
const CHUNK: usize = 64;

/// The tree with its ELF files stripped by `strip_tool`.
#[node(id = "gaia.strip.elf", inputs(store, strip_tool, tree), outputs(tree))]
pub fn strip_elf(store: &str, strip_tool: &str, tree: Tree, cx: &Ctx) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let owned = owned_digest(&tree).ok_or_else(|| body("a tree that is not in the store"))?;
    let source = tree
        .object
        .path_in(&store)
        .ok_or_else(|| body("a tree that is not in the store"))?;
    let version = Command::new(strip_tool)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|e| body(format!("{strip_tool} --version: {e}")))?;
    let version = [version.stdout, version.stderr].concat();
    let key = KeyBuilder::new(NAME, 1)
        .config("strip", &version)
        .input("tree", owned)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let out = work.dir().join("tree");
            tree::copy(&source, &out).map_err(body)?;
            let jobs = elf_jobs(&out).map_err(body)?;
            for flag in [STRIP_UNNEEDED, STRIP_DEBUG] {
                let paths: Vec<&PathBuf> = jobs
                    .iter()
                    .filter(|job| job.flag == flag)
                    .map(|job| &job.path)
                    .collect();
                for chunk in paths.chunks(CHUNK) {
                    work.run(
                        &Tool::new(strip_tool)
                            .arg(flag)
                            .args(chunk.iter().map(|path| path.as_os_str())),
                    )?;
                }
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: tree.entries,
    })
}

/// One file to strip, with the flag it takes.
struct Job {
    flag: &'static str,
    path: PathBuf,
}

/// The ELF files of `root` that a strip changes, with their flags.
fn elf_jobs(root: &Path) -> Result<Vec<Job>, String> {
    let mut jobs = Vec::new();
    for (relative, kind) in tree::walk(root)? {
        let path = root.join(&relative);
        if kind != Kind::File || !has_elf_magic(&path) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{relative}: {e}"))?;
        let Some(elf) = elf::parse(&bytes).map_err(|e| format!("{relative}: {e}"))? else {
            continue;
        };
        let flag = match elf.kind {
            ET_REL => STRIP_DEBUG,
            ET_EXEC | ET_DYN => STRIP_UNNEEDED,
            _ => continue,
        };
        jobs.push(Job { flag, path });
    }
    Ok(jobs)
}

/// Whether the file starts with the ELF magic. Most files are not ELF, and
/// their first four bytes settle it without a full read.
fn has_elf_magic(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && &magic == b"\x7fELF"
}

fn body(why: impl std::fmt::Display) -> StepError {
    StepError::body(NAME, why)
}
