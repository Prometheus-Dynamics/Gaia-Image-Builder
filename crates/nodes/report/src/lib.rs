//! `gaia.report.build`: the report of one image. What it is made of and
//! where each file came from, which locked sources each owner was built
//! from, and the size and content delta against a previous report.
//!
//! - **Inputs.** The image's name, its layers (each an owner and its
//!   tree, as `gaia.tree.merge` takes them), the image archive, the lock
//!   text, and the store reference of the previous report (empty for none).
//! - **Output.** `report.json` as an artifact of kind `report`. Its fields
//!   are sorted and carry no host paths, so the same inputs give the same
//!   bytes. [`render_text`] lays the same content out for reading.
//! - **Masking.** URLs lose their user and password, query values and
//!   names that say secret are masked. Secrets never reach the output.
//! - **Key.** Image digest, each layer's tree digest and entries, the lock
//!   text and the previous report's reference. A report made again from the
//!   same inputs is a store hit.
//!
//! What it does not do: read the run record that `gaia build` writes, or
//! read the build's own steps. The layers and the image are its inputs.

mod collect;
mod delta;
mod masking;
mod model;
mod render;

pub use masking::{MASK, is_secret_name, mask_url, mask_value};
pub use model::{
    ComponentRecord, Delta, FORMAT, FileChange, FileRecord, OwnerChange, Report, SourceChange,
    SourceRecord, VERSION,
};
pub use render::render_text;

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_lock::Lock;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::{Kind, ObjectRef};
use gaia_tree::Layer;
use gaia_types::{Artifact, ArtifactKind};
use std::path::Path;

const NAME: &str = "gaia.report.build";

/// The report of one image, from its layers, the image archive, the lock
/// and the previous report (`""` when there is none).
#[node(
    id = "gaia.report.build",
    inputs(store, name, layers, image, lock, previous),
    outputs(report)
)]
pub fn report_build(
    store: &str,
    name: &str,
    layers: FanIn<'_, Layer>,
    image: Artifact,
    lock: &str,
    previous: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let layers: Vec<Layer> = layers.iter().map(|layer| (*layer).clone()).collect();
    report_layers(store, name, &layers, &image, lock, previous, cx)
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 1] = [&REPORT_BUILD_NODE];

/// [`report_build`] on a slice of layers, for a caller that has them in hand.
pub fn report_layers(
    store: &str,
    name: &str,
    layers: &[Layer],
    image: &Artifact,
    lock: &str,
    previous: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let image_object = image
        .object
        .object_ref()
        .ok_or_else(|| StepError::body(NAME, "the image is not in the store"))?;
    let mut key = KeyBuilder::new(NAME, 1)
        .config("image name", name)
        .config("lock", lock)
        .config("previous", previous.trim())
        .input("image", image_object.digest);
    for (i, layer) in layers.iter().enumerate() {
        let object = layer.tree.object.object_ref().ok_or_else(|| {
            StepError::body(
                NAME,
                format!("{}: its tree is not in the store", layer.owner),
            )
        })?;
        let mut entries = layer.tree.entries.clone();
        entries.sort();
        let modes: Vec<String> = entries
            .iter()
            .map(|e| format!("{} {:o} {} {}", e.path, e.mode, e.uid, e.gid))
            .collect();
        key = key
            .config(&format!("layer {i} owner"), &layer.owner)
            .config(&format!("layer {i} entries"), modes.join("\n"))
            .input(&format!("layer {i}"), object.digest);
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("report", Path::new("report.json"))],
        |work| {
            let report = build(&store, name, layers, image_object, lock, previous)?;
            let text = serde_json::to_vec_pretty(&report).map_err(|e| StepError::body(NAME, e))?;
            std::fs::write(work.dir().join("report.json"), text)
                .map_err(|e| StepError::body(NAME, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("report"),
        object: made.outputs["report"].into(),
    })
}

fn build(
    store: &Store,
    name: &str,
    layers: &[Layer],
    image: ObjectRef,
    lock: &str,
    previous: &str,
) -> Result<Report, StepError> {
    let image_bytes = std::fs::metadata(store.path(&image))
        .map_err(|e| StepError::body(NAME, format!("the image: {e}")))?
        .len();
    let lock = if lock.trim().is_empty() {
        Lock::default()
    } else {
        Lock::parse(lock).map_err(|e| StepError::body(NAME, format!("the lock: {e}")))?
    };
    let owners: Vec<&str> = layers.iter().map(|layer| layer.owner.as_str()).collect();
    let (files, components) =
        collect::files(store, layers).map_err(|e| StepError::body(NAME, e))?;
    let total_bytes = files.iter().map(|file| file.bytes).sum();
    let previous_report = read_previous(store, previous)?;
    let mut report = Report {
        format: FORMAT.to_string(),
        version: VERSION,
        image: name.to_string(),
        image_object: image.to_string(),
        image_bytes,
        sources: collect::sources(&lock, &owners),
        components,
        files,
        total_bytes,
        previous: previous_report
            .as_ref()
            .map(|_| previous.trim().to_string()),
        delta: None,
    };
    report.delta = previous_report
        .as_ref()
        .map(|before| delta::compare(before, &report));
    Ok(report)
}

/// The report a store reference names, when one is given.
fn read_previous(store: &Store, previous: &str) -> Result<Option<Report>, StepError> {
    let text = previous.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let object: ObjectRef = text
        .parse()
        .map_err(|e| StepError::body(NAME, format!("the previous report: {e}")))?;
    if object.kind != Kind::File {
        return Err(StepError::body(
            NAME,
            format!("the previous report `{text}` is not a file"),
        ));
    }
    let bytes = std::fs::read(store.path(&object))
        .map_err(|e| StepError::body(NAME, format!("the previous report `{text}`: {e}")))?;
    let report: Report = serde_json::from_slice(&bytes)
        .map_err(|e| StepError::body(NAME, format!("the previous report `{text}`: {e}")))?;
    if report.format != FORMAT {
        return Err(StepError::body(
            NAME,
            format!("the previous report `{text}` is not a {FORMAT}"),
        ));
    }
    Ok(Some(report))
}
