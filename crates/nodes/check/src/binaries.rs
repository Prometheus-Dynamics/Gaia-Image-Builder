//! `gaia.check.binaries`: every dynamically linked file in the image can run:
//! its program interpreter is in the image, and so is each library it names.
//!
//! A file is checked when it is an ELF file that names an interpreter or a
//! `DT_NEEDED` library. A library is looked up by its name in `lib/`, `lib64/`,
//! `usr/lib/`, `usr/lib64/` and `usr/local/lib/`, following links inside the
//! tree. `RUNPATH` and `RPATH` are not searched: a library found only through
//! one is reported as missing.

use crate::common::{root, unreadable, verdict};
use crate::elf;
use crate::tree::{self, Kind};
use daedalus::prelude::*;
use gaia_step::{Ctx, StepError};
use gaia_types::Tree;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const NAME: &str = "gaia.check.binaries";
const LIB_DIRS: [&str; 5] = ["lib/", "lib64/", "usr/lib/", "usr/lib64/", "usr/local/lib/"];
const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

/// Fails when a dynamically linked file names an interpreter or a library the
/// image does not hold. Gives the tree unchanged.
#[node(id = "gaia.check.binaries", inputs(store, tree), outputs(tree))]
pub fn check_binaries(store: &str, tree: Tree, _cx: &Ctx) -> Result<Tree, StepError> {
    let dir = root(NAME, store, &tree)?;
    let problems = problems(&dir).map_err(|e| unreadable(NAME, e))?;
    verdict(NAME, problems)?;
    Ok(tree)
}

/// Every interpreter and library that is missing, and every ELF file that
/// cannot be read, as one line each.
pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    for (path, kind) in tree::walk(root)? {
        if kind != Kind::File || !is_elf(&root.join(&path)) {
            continue;
        }
        let bytes = std::fs::read(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        let elf = match elf::parse(&bytes) {
            Ok(Some(elf)) => elf,
            Ok(None) => continue,
            Err(why) => {
                problems.push(format!("{path} is not a readable ELF file: {why}"));
                continue;
            }
        };
        if let Some(interpreter) = &elf.interpreter
            && !tree::is_file(root, interpreter.trim_start_matches('/'))
        {
            problems.push(format!(
                "{path} needs the program interpreter {interpreter}, which is not in the image"
            ));
        }
        for library in &elf.needed {
            let found = LIB_DIRS
                .iter()
                .any(|dir| tree::is_file(root, &format!("{dir}{library}")));
            if !found {
                problems.push(format!(
                    "{path} needs the library {library}, which no library directory holds ({})",
                    LIB_DIRS.join(" ")
                ));
            }
        }
    }
    Ok(problems)
}

/// Whether the file starts with the ELF magic, read without reading the rest.
fn is_elf(path: &Path) -> bool {
    let mut head = [0u8; 4];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && &head == ELF_MAGIC
}
