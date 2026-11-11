//! `gaia.os.none`: the OS provider that provides nothing.
//!
//! A product with no OS (firmware for a microcontroller, a bare package)
//! still takes the graph's shape: the board's target goes to an OS
//! provider, and the provider's root filesystem goes on to assembly. Here
//! that root filesystem is the empty tree, so the image's outputs are only
//! what its components build: a `.bin`, a `.uf2`, whatever kind they name.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Target, Tree};
use std::path::Path;

const NAME: &str = "gaia.os.none";

/// The empty root filesystem for `target`. Keyed on the target, so a
/// report or a lock sees which target asked for no OS.
#[node(id = "gaia.os.none", inputs(store, target), outputs(rootfs))]
pub fn os_none(store: &str, target: Target, cx: &Ctx) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let key = KeyBuilder::new(NAME, 1)
        .config("arch", target.arch.as_str())
        .config("cpu", target.cpu.as_deref().unwrap_or(""))
        .config("os", target.os.as_str())
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("rootfs", Path::new("rootfs"))],
        |work| {
            std::fs::create_dir_all(work.dir().join("rootfs"))
                .map_err(|e| StepError::body(NAME, format!("rootfs: {e}")))
        },
    )?;
    Ok(Tree {
        object: made.outputs["rootfs"].into(),
        entries: Vec::new(),
    })
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&OS_NONE_NODE];
