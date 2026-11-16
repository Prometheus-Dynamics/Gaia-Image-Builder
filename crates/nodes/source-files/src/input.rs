//! `gaia.source.input`: a file the command line names, as a stored tree
//! that holds that one file under its name.
//!
//! The copy is 0755 when the file is executable by anyone, and 0644
//! otherwise: a script a build runs from the tree (a Buildroot post-build
//! script named by a fragment) stays runnable, and nothing else of the
//! file's mode on this machine reaches the store.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::file_digest;
use gaia_types::Tree;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const NAME: &str = "gaia.source.input";

/// Stores the regular file at `path` as `name` in a tree, keyed on the
/// file's content digest and `name`, not its path: the same bytes from two
/// places are one step, and changed bytes are another.
#[node(id = "gaia.source.input", inputs(store, path, name), outputs(tree))]
pub fn source_input(store: &str, path: &str, name: &str, cx: &Ctx) -> Result<Tree, StepError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(StepError::body(
            NAME,
            format!("`{name}` is not a file name"),
        ));
    }
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let file = Path::new(path);
    if !file.is_file() {
        return Err(StepError::body(
            NAME,
            format!("{path} is not a regular file"),
        ));
    }
    let digest = file_digest(file).map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?;
    let executable = std::fs::metadata(file)
        .map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?
        .permissions()
        .mode()
        & 0o111
        != 0;
    let mut key = KeyBuilder::new(NAME, 1)
        .input("file", digest)
        .config("name", name);
    // Only when it is: a file that is not executable keeps the key it had.
    if executable {
        key = key.config("executable", "yes");
    }
    let key = key.finish();
    let mode = if executable { 0o755 } else { 0o644 };
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let dir = work.dir().join("tree");
            std::fs::create_dir(&dir).map_err(|e| StepError::body(NAME, e))?;
            let copy = dir.join(name);
            std::fs::copy(file, &copy)
                .map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?;
            std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(mode))
                .map_err(|e| StepError::body(NAME, e))?;
            // Changed while it was copied: not the bytes the key names.
            let copied = file_digest(&copy).map_err(|e| StepError::body(NAME, e))?;
            if copied != digest {
                return Err(StepError::body(
                    NAME,
                    format!("{path} changed while it was copied: {digest} became {copied}"),
                ));
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}
