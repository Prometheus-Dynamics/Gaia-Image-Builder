//! Node `gaia.source.oci`: a container image, pulled by digest, as its root
//! filesystem and its config.
//!
//! Inputs: the store, the `curl` program (pinned by path), the image
//! reference, the manifest digest from the lock, the platform
//! (`linux/arm64`, `linux/arm/v7`) for an image index, and the name of an
//! environment variable holding `user:password` or a token, or an empty
//! name for none.
//!
//! - **By digest only:** the manifest is fetched by the lock's digest and
//!   its bytes must hash to it; an index picks the platform's manifest by
//!   digest the same way; the config and every layer must hash to the
//!   digests the manifest gives. A mismatch fails, naming both digests.
//! - **Anonymous by default:** a bearer challenge is answered with an
//!   anonymous token. Credentials are read only from the environment
//!   variable the description declares, and go only to the registry's
//!   token service or, for a basic challenge, the registry.
//! - **Layers in order**, lowest first, with whiteouts (see [`layer`]).
//! - **Outputs:** `rootfs`, a [`Tree`] whose entries list paths not owned
//!   by root; `config`, the image config as an [`Artifact`] of kind
//!   `config`. The key is the manifest digest and the platform.

mod layer;
mod reference;
mod registry;

pub use reference::Reference;

/// The libraries this node links, pinned in its Cargo.toml. Its step key names
/// them, and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[
    ("flate2", "1.1.11"),
    ("miniz_oxide", "0.9.1"),
    ("tar", "0.4.46"),
];

/// libzstd, as the linked library reports its version (zstd-safe reads it at
/// run time). The step key names it with [`LIBS`].
pub fn runtime_libs() -> [(&'static str, &'static str); 1] {
    [("libzstd", zstd::zstd_safe::version_string())]
}

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Work};
use gaia_store::{Digest, file_digest};
use gaia_types::{Artifact, ArtifactKind, FileEntry, Tree};
use registry::Registry;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// The node's id, and the step name in its errors and logs.
pub const NAME: &str = "gaia.source.oci";

const INDEX_TYPES: [&str; 2] = [
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
];
const MANIFEST_TYPES: [&str; 2] = [
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
];

/// Pulls `reference` at `digest` for `platform`.
#[node(
    id = "gaia.source.oci",
    inputs(store, curl, reference, digest, platform, credentials),
    outputs(rootfs, config)
)]
pub fn source(
    store: &str,
    curl: &str,
    reference: &str,
    digest: &str,
    platform: &str,
    credentials: &str,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let parsed = Reference::parse(reference).map_err(|e| StepError::body(NAME, e))?;
    let want = Digest::parse(digest.trim()).ok_or_else(|| {
        StepError::body(
            NAME,
            format!("{reference}: `{digest}` is not a sha256 digest"),
        )
    })?;
    if let Some(named) = &parsed.digest
        && Digest::parse(named) != Some(want)
    {
        return Err(StepError::body(
            NAME,
            format!("{reference} names {named}, but the lock gives {want}"),
        ));
    }
    let credentials = match credentials.trim() {
        "" => None,
        name => {
            let value = std::env::var(name).map_err(|_| {
                StepError::body(
                    NAME,
                    format!("{reference}: credentials are declared in ${name}, which is not set"),
                )
            })?;
            Some(if value.contains(':') {
                value
            } else {
                format!("gaia:{value}")
            })
        }
    };
    let key = KeyBuilder::new(NAME, 1)
        .config("platform", platform.trim())
        .input("manifest", want)
        .libs(LIBS)
        .libs(&runtime_libs())
        .finish();
    let outputs = [
        ("rootfs", Path::new("rootfs")),
        ("config", Path::new("config.json")),
        ("owners", Path::new("owners.json")),
    ];
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(&outputs, |work| {
        let mut registry = Registry::new(
            curl,
            parsed.base.clone(),
            parsed.repository.clone(),
            credentials,
        );
        pull(work, &mut registry, reference, want, platform.trim())
    })?;
    let owners: Vec<FileEntry> = serde_json::from_slice(
        &fs::read(store.path(&made.outputs["owners"])).map_err(|e| StepError::body(NAME, e))?,
    )
    .map_err(|e| StepError::body(NAME, e))?;
    Ok((
        Tree {
            object: made.outputs["rootfs"].into(),
            entries: owners,
        },
        Artifact {
            kind: ArtifactKind::Config,
            object: made.outputs["config"].into(),
        },
    ))
}

fn pull(
    work: &mut Work<'_>,
    registry: &mut Registry,
    reference: &str,
    want: Digest,
    platform: &str,
) -> Result<(), StepError> {
    let dir = work.dir().to_path_buf();
    let accept: Vec<&str> = INDEX_TYPES.iter().chain(&MANIFEST_TYPES).copied().collect();
    let mut manifest = fetch_json(
        work,
        registry,
        &format!("manifests/{want}"),
        &accept,
        want,
        &dir.join("manifest.json"),
    )?;
    if manifest.get("manifests").is_some() {
        let chosen = pick(&manifest, platform).ok_or_else(|| {
            StepError::body(
                NAME,
                format!("{reference}: the index has no manifest for `{platform}`"),
            )
        })?;
        manifest = fetch_json(
            work,
            registry,
            &format!("manifests/{chosen}"),
            &MANIFEST_TYPES,
            chosen,
            &dir.join("manifest.json"),
        )?;
    }
    let config = descriptor(&manifest["config"])
        .ok_or_else(|| StepError::body(NAME, format!("{reference}: the manifest has no config")))?;
    fetch_blob(work, registry, config, &dir.join("config.json"))?;
    let layers: Vec<Digest> = manifest["layers"]
        .as_array()
        .ok_or_else(|| StepError::body(NAME, format!("{reference}: the manifest has no layers")))?
        .iter()
        .map(|layer| {
            descriptor(layer).ok_or_else(|| {
                StepError::body(NAME, format!("{reference}: a layer with no digest"))
            })
        })
        .collect::<Result<_, _>>()?;
    let root = dir.join("rootfs");
    fs::create_dir(&root).map_err(|e| StepError::body(NAME, e))?;
    let mut rootfs = layer::Rootfs::default();
    for (index, digest) in layers.iter().enumerate() {
        let file = dir.join("layer");
        fetch_blob(work, registry, *digest, &file)?;
        let cancelled = || work.cancelled();
        rootfs
            .apply(&root, &file, &cancelled)
            .map_err(|e| StepError::body(NAME, format!("layer {index} ({digest}): {e}")))?;
        fs::remove_file(&file).map_err(|e| StepError::body(NAME, e))?;
    }
    rootfs.finish(&root).map_err(|e| StepError::body(NAME, e))?;
    let owners: Vec<FileEntry> = rootfs
        .owners
        .iter()
        .map(|(path, owner)| FileEntry {
            path: path.to_string_lossy().into_owned(),
            mode: owner.mode,
            uid: owner.uid,
            gid: owner.gid,
        })
        .collect();
    let text = serde_json::to_vec_pretty(&owners).map_err(|e| StepError::body(NAME, e))?;
    fs::write(dir.join("owners.json"), text).map_err(|e| StepError::body(NAME, e))?;
    fs::remove_file(dir.join("manifest.json")).map_err(|e| StepError::body(NAME, e))
}

/// GETs a manifest by digest and checks its bytes hash to it.
fn fetch_json(
    work: &mut Work<'_>,
    registry: &mut Registry,
    what: &str,
    accept: &[&str],
    want: Digest,
    out: &Path,
) -> Result<Value, StepError> {
    registry.get(work, what, accept, out)?;
    check(out, want, what)?;
    serde_json::from_slice(&fs::read(out).map_err(|e| StepError::body(NAME, e))?)
        .map_err(|e| StepError::body(NAME, format!("{what}: {e}")))
}

/// GETs a blob and checks it hashes to its digest.
fn fetch_blob(
    work: &mut Work<'_>,
    registry: &mut Registry,
    digest: Digest,
    out: &Path,
) -> Result<(), StepError> {
    let what = format!("blobs/{digest}");
    registry.get(work, &what, &[], out)?;
    check(out, digest, &what)
}

fn check(file: &Path, want: Digest, what: &str) -> Result<(), StepError> {
    let got = file_digest(file).map_err(|e| StepError::body(NAME, e))?;
    if got == want {
        Ok(())
    } else {
        Err(StepError::body(
            NAME,
            format!("{what}: digest mismatch: wanted {want}, got {got}"),
        ))
    }
}

/// A descriptor's digest.
fn descriptor(value: &Value) -> Option<Digest> {
    value
        .get("digest")
        .and_then(Value::as_str)
        .and_then(Digest::parse)
}

/// The index entry for `platform` (`os/arch[/variant]`).
fn pick(index: &Value, platform: &str) -> Option<Digest> {
    let mut parts = platform.split('/');
    let (os, arch, variant) = (parts.next()?, parts.next()?, parts.next());
    index["manifests"].as_array()?.iter().find_map(|entry| {
        let p = &entry["platform"];
        let matches = p["os"].as_str() == Some(os)
            && p["architecture"].as_str() == Some(arch)
            && variant.is_none_or(|v| p["variant"].as_str() == Some(v));
        if matches { descriptor(entry) } else { None }
    })
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&SOURCE_NODE];
