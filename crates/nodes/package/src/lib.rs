//! Packages for other installers: a stored tree as an opkg `.ipk`, a Debian
//! `.deb`, or an OCI image archive, each a Daedalus node.
//!
//! - **`gaia.package.ipk`**: an ar archive of `debian-binary`, `control.tar.gz`
//!   and `data.tar.gz`, the layout opkg-build writes. The data is the tree
//!   under `./` with no root entry (opkg would take `.` for a file).
//! - **`gaia.package.deb`**: the same ar layout, as dpkg-deb writes it. The
//!   data keeps the `./` root entry.
//! - **`gaia.package.oci`**: an OCI image archive (an OCI image layout in a
//!   tar, `index.json` with the ref name): one uncompressed layer, the tree
//!   as it is, and the config's entrypoint and architecture. Loads with
//!   `podman load`.
//!
//! The control data is a [`Control`] from the component description. Owners
//! and modes come from the tree's entries, as every node that writes a tree
//! does; times are 0 and gzip headers carry no name, so the same tree and
//! control give the same bytes. Each package's output is one file in the
//! content store.

mod archive;
mod control;

pub use control::Control;

/// The libraries this node links, pinned in its Cargo.toml. Its step key names
/// them, and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[
    ("flate2", "1.1.11"),
    ("miniz_oxide", "0.9.1"),
    ("tar", "0.4.46"),
];

use archive::{control_tar, finish_gz, gz_file, tree_tar, write_ar};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::{Digest, file_digest};
use gaia_types::owned_digest;
use gaia_types::{Artifact, ArtifactKind, Tree};
use serde_json::json;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

const IPK: &str = "gaia.package.ipk";
const DEB: &str = "gaia.package.deb";
const OCI: &str = "gaia.package.oci";
/// The `debian-binary` member: the format version opkg and dpkg read.
const DEBIAN_BINARY: &[u8] = b"2.0\n";
/// Every timestamp the OCI archive writes.
const EPOCH: &str = "1970-01-01T00:00:00Z";
const MANIFEST_MEDIA: &str = "application/vnd.oci.image.manifest.v1+json";
const CONFIG_MEDIA: &str = "application/vnd.oci.image.config.v1+json";
const LAYER_MEDIA: &str = "application/vnd.oci.image.layer.v1.tar";

/// The package formats an ar archive can take.
#[derive(Clone, Copy)]
enum Format {
    Ipk,
    Deb,
}

impl Format {
    fn node(self) -> &'static str {
        match self {
            Self::Ipk => IPK,
            Self::Deb => DEB,
        }
    }

    fn file(self) -> &'static str {
        match self {
            Self::Ipk => "package.ipk",
            Self::Deb => "package.deb",
        }
    }

    /// dpkg-deb keeps the `./` root entry in the data; opkg does not.
    fn root_dir(self) -> bool {
        matches!(self, Self::Deb)
    }
}

/// An `.ipk` package of `tree`, described by `control`.
#[node(
    id = "gaia.package.ipk",
    inputs(store, tree, control),
    outputs(package)
)]
pub fn ipk(store: &str, tree: Tree, control: Control, cx: &Ctx) -> Result<Artifact, StepError> {
    package(store, &tree, &control, Format::Ipk, cx)
}

/// A Debian `.deb` package of `tree`, described by `control`.
#[node(
    id = "gaia.package.deb",
    inputs(store, tree, control),
    outputs(package)
)]
pub fn deb(store: &str, tree: Tree, control: Control, cx: &Ctx) -> Result<Artifact, StepError> {
    package(store, &tree, &control, Format::Deb, cx)
}

/// The shared body of the two ar packages: the data and control tars, gzipped,
/// then the ar archive. The control's `Installed-Size` is the data's bytes.
fn package(
    store: &str,
    tree: &Tree,
    control: &Control,
    format: Format,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let name = format.node();
    let store = Store::open(store).map_err(StepError::store(name))?;
    let err = |e: &dyn std::fmt::Display| StepError::body(name, e.to_string());
    control.check().map_err(|e| err(&e))?;
    let owned = owned_digest(tree).ok_or_else(|| err(&"a tree that is not in the store"))?;
    let root = tree
        .object
        .path_in(&store)
        .ok_or_else(|| err(&"a tree that is not in the store"))?;
    let key = KeyBuilder::new(name, 1)
        .config("control", serde_json::to_vec(control).map_err(|e| err(&e))?)
        .input("tree", owned)
        .libs(LIBS)
        .finish();
    let made = Step::new(&store, cx, name, key).look_up_or_make(
        &[("package", Path::new(format.file()))],
        |work| {
            let dir = work.dir().to_path_buf();
            let data = dir.join("data.tar.gz");
            let (gz, installed) = tree_tar(
                gz_file(&data).map_err(|e| err(&e))?,
                &root,
                &tree.entries,
                "./",
                format.root_dir(),
            )
            .map_err(|e| err(&e))?;
            finish_gz(gz).map_err(|e| err(&e))?;
            let control_gz = dir.join("control.tar.gz");
            let text = control.text(installed);
            finish_gz(
                control_tar(
                    gz_file(&control_gz).map_err(|e| err(&e))?,
                    &[("control", text.as_bytes())],
                )
                .map_err(|e| err(&e))?,
            )
            .map_err(|e| err(&e))?;
            let binary = dir.join("debian-binary");
            std::fs::write(&binary, DEBIAN_BINARY).map_err(|e| err(&e))?;
            let members: [(&str, PathBuf); 3] = [
                ("debian-binary", binary.clone()),
                ("control.tar.gz", control_gz.clone()),
                ("data.tar.gz", data.clone()),
            ];
            write_ar(&dir.join(format.file()), &members).map_err(|e| err(&e))?;
            for part in [binary, control_gz, data] {
                let _ = std::fs::remove_file(part);
            }
            Ok(())
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("package"),
        object: made.outputs["package"].into(),
    })
}

/// An OCI image archive of `tree`: one layer, the config's entrypoint and
/// architecture, tagged `reference`.
#[node(
    id = "gaia.package.oci",
    inputs(store, tree, reference, architecture, entrypoint),
    outputs(archive)
)]
pub fn oci(
    store: &str,
    tree: Tree,
    reference: &str,
    architecture: &str,
    entrypoint: Vec<String>,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(OCI))?;
    let err = |e: &dyn std::fmt::Display| StepError::body(OCI, e.to_string());
    if reference.is_empty() || reference.chars().any(char::is_whitespace) {
        return Err(err(&format!(
            "reference {reference:?} must be a name with no spaces"
        )));
    }
    if architecture.is_empty() || architecture.chars().any(char::is_whitespace) {
        return Err(err(&format!(
            "architecture {architecture:?} must be one word"
        )));
    }
    if entrypoint.iter().any(|arg| arg.is_empty()) {
        return Err(err(&"an entrypoint argument is empty"));
    }
    let owned = owned_digest(&tree).ok_or_else(|| err(&"a tree that is not in the store"))?;
    let root = tree
        .object
        .path_in(&store)
        .ok_or_else(|| err(&"a tree that is not in the store"))?;
    let key = KeyBuilder::new(OCI, 1)
        .config("reference", reference)
        .config("architecture", architecture)
        .config(
            "entrypoint",
            serde_json::to_vec(&entrypoint).map_err(|e| err(&e))?,
        )
        .input("tree", owned)
        .libs(LIBS)
        .finish();
    let made = Step::new(&store, cx, OCI, key).look_up_or_make(
        &[("archive", Path::new("image.tar"))],
        |work| {
            let dir = work.dir().to_path_buf();
            let layer_path = dir.join("layer.tar");
            let (layer, _) = tree_tar(
                BufWriter::new(File::create(&layer_path).map_err(|e| err(&e))?),
                &root,
                &tree.entries,
                "",
                false,
            )
            .map_err(|e| err(&e))?;
            flush(layer).map_err(|e| err(&e))?;
            let layer_digest = file_digest(&layer_path).map_err(|e| err(&e))?;
            let layer_size = std::fs::metadata(&layer_path).map_err(|e| err(&e))?.len();

            let config = serde_json::to_vec(&json!({
                "architecture": architecture,
                "os": "linux",
                "created": EPOCH,
                "config": { "Entrypoint": entrypoint },
                "rootfs": { "type": "layers", "diff_ids": [oci_digest(&layer_digest)] },
                "history": [{ "created": EPOCH, "created_by": OCI }],
            }))
            .map_err(|e| err(&e))?;
            let config_digest = Digest::of_bytes(&config);
            let manifest = serde_json::to_vec(&json!({
                "schemaVersion": 2,
                "mediaType": MANIFEST_MEDIA,
                "config": {
                    "mediaType": CONFIG_MEDIA,
                    "digest": oci_digest(&config_digest),
                    "size": config.len(),
                },
                "layers": [{
                    "mediaType": LAYER_MEDIA,
                    "digest": oci_digest(&layer_digest),
                    "size": layer_size,
                }],
            }))
            .map_err(|e| err(&e))?;
            let manifest_digest = Digest::of_bytes(&manifest);
            let index = serde_json::to_vec(&json!({
                "schemaVersion": 2,
                "manifests": [{
                    "mediaType": MANIFEST_MEDIA,
                    "digest": oci_digest(&manifest_digest),
                    "size": manifest.len(),
                    "annotations": { "org.opencontainers.image.ref.name": reference },
                }],
            }))
            .map_err(|e| err(&e))?;

            let out = BufWriter::new(File::create(dir.join("image.tar")).map_err(|e| err(&e))?);
            let mut archive = tar::Builder::new(out);
            archive.mode(tar::HeaderMode::Deterministic);
            entry(
                &mut archive,
                "oci-layout",
                b"{\"imageLayoutVersion\":\"1.0.0\"}\n",
            )
            .map_err(|e| err(&e))?;
            entry(&mut archive, "index.json", &index).map_err(|e| err(&e))?;
            entry(&mut archive, &blob(&config_digest), &config).map_err(|e| err(&e))?;
            let mut layer_file = File::open(&layer_path).map_err(|e| err(&e))?;
            let mut header = header(layer_size);
            archive
                .append_data(&mut header, blob(&layer_digest), &mut layer_file)
                .map_err(|e| err(&e))?;
            entry(&mut archive, &blob(&manifest_digest), &manifest).map_err(|e| err(&e))?;
            flush(archive.into_inner().map_err(|e| err(&e))?).map_err(|e| err(&e))?;
            let _ = std::fs::remove_file(&layer_path);
            Ok(())
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("archive"),
        object: made.outputs["archive"].into(),
    })
}

/// A regular file header for `size` bytes: mode 0644, owner 0, time 0.
fn header(size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header
}

/// One whole-bytes entry of the archive.
fn entry<W: Write>(archive: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut header = header(bytes.len() as u64);
    archive.append_data(&mut header, name, bytes)
}

/// Ends a buffered file and syncs it.
fn flush(out: BufWriter<File>) -> io::Result<()> {
    out.into_inner().map_err(|e| e.into_error())?.sync_all()
}

/// `sha256:<hex>`, the digest form OCI uses.
fn oci_digest(digest: &Digest) -> String {
    format!("sha256:{}", digest.hex())
}

/// The layout path of a blob.
fn blob(digest: &Digest) -> String {
    format!("blobs/sha256/{}", digest.hex())
}

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 3] = [&IPK_NODE, &DEB_NODE, &OCI_NODE];
