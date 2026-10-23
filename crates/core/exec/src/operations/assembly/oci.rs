//! The `[[image.assembly.oci]]` output: an OCI image built from layers, with
//! no daemon. It is written as an OCI image layout directory (`oci-layout`,
//! `index.json`, `blobs/sha256/...`, which `podman load`, `skopeo` and
//! `docker load` accept) or as a `docker load` tarball (`docker-archive`).
//!
//! A layer is a `tar` (an uncompressed layer tar, used as it is) or a `tree`
//! (an assembly tree, tarred deterministically here). Its `diff_id` is the
//! sha256 of the uncompressed tar. Its compressed blob is cached under the
//! build dir by `diff_id` and compression, so an unchanged layer is not
//! compressed again; an app-only change rewrites one blob, the config and
//! the manifest. Compression goes through the external `gzip` or `zstd`
//! tool, with fixed flags, so the same layers give the same bytes.

use std::collections::HashMap;
use std::fs as std_fs;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

use super::tar::TarWriter;
use super::transforms::compress_to_dest;
use super::*;
use gaia_spec::{
    AssemblyOciCompressionSpec, AssemblyOciFormatSpec, AssemblyOciLayerSource, AssemblyOciSpec,
};

const OCI_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
/// The created time when `SOURCE_DATE_EPOCH` is unset: the epoch itself.
const DEFAULT_CREATED: i64 = 0;
/// Serialises the cache's compressions: two images that share a layer would
/// otherwise compress it to the same temporary file at once.
static BLOB_CACHE_LOCK: Mutex<()> = Mutex::new(());

/// What one OCI image build produced.
pub(super) struct OciSummary {
    pub(super) output: PathBuf,
    /// Every byte of the output: the blobs and documents, or the tarball.
    pub(super) bytes: u64,
    pub(super) config_digest: String,
    pub(super) manifest_digest: String,
    pub(super) layers: Vec<OciLayerSummary>,
}

pub(super) struct OciLayerSummary {
    /// How the layer is named in the spec: `tar <path>` or `tree <id>`.
    pub(super) label: String,
    pub(super) diff_id: String,
    pub(super) blob_digest: String,
    pub(super) blob_bytes: u64,
    /// Whether the compressed blob was already in the cache.
    pub(super) reused: bool,
}

/// A layer as the image needs it: its uncompressed digest, and the blob
/// that stores it in the output.
struct LayerBlob {
    label: String,
    diff_id: String,
    blob_digest: String,
    blob_bytes: u64,
    /// The file the blob's bytes are copied or linked from.
    path: PathBuf,
    reused: bool,
}

/// Writes an `[[image.assembly.oci]]` entry and publishes it atomically.
pub(super) fn execute_assembly_oci(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    oci: &AssemblyOciSpec,
    cancel: Option<gaia_process::ProcessCancelCheck>,
) -> Result<OciSummary, AssemblyError> {
    let format = oci.parsed_format().ok_or_else(|| {
        format!(
            "assembly oci image '{}' has an unknown format '{}'",
            oci.id, oci.format
        )
    })?;
    let compression = oci.parsed_compression().ok_or_else(|| {
        format!(
            "assembly oci image '{}' has an unknown compression '{}'",
            oci.id, oci.compression
        )
    })?;
    let output = roots.resolve_path(spec, oci.dest.as_str())?;
    let parent = output.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "assembly oci image '{}' output '{}' has no parent dir",
            oci.id,
            output.display()
        )
    })?;
    std_fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "failed to create assembly oci output dir '{}': {error}",
            parent.display()
        )
    })?;
    let cache_dir = roots.resolve_path(spec, format!("{}/oci-blobs", spec.workspace.build_dir))?;
    std_fs::create_dir_all(&cache_dir).map_err(|error| {
        format!(
            "failed to create assembly oci blob cache '{}': {error}",
            cache_dir.display()
        )
    })?;
    let name = output.file_name().map_or_else(
        || "oci".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let stage = parent.join(format!(".{name}.gaia-tmp"));
    let layers_dir = parent.join(format!(".{name}.gaia-layers"));
    remove_path(&stage);
    remove_path(&layers_dir);

    let built = build_oci(
        spec,
        roots,
        oci,
        Build {
            format,
            compression,
            output: &output,
            stage: &stage,
            layers_dir: &layers_dir,
            cache_dir: &cache_dir,
            cancel,
        },
    );
    remove_path(&layers_dir);
    if built.is_err() {
        remove_path(&stage);
    }
    built
}

/// The paths and settings of one build.
struct Build<'a> {
    format: AssemblyOciFormatSpec,
    compression: AssemblyOciCompressionSpec,
    output: &'a Path,
    /// Where the image is built before it is published to `output`.
    stage: &'a Path,
    /// Uncompressed layer tars made from trees, before they are compressed.
    layers_dir: &'a Path,
    cache_dir: &'a Path,
    cancel: Option<gaia_process::ProcessCancelCheck>,
}

fn build_oci(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    oci: &AssemblyOciSpec,
    build: Build<'_>,
) -> Result<OciSummary, AssemblyError> {
    let mut layers = Vec::with_capacity(oci.layers.len());
    for (index, layer) in oci.layers.iter().enumerate() {
        let number = index + 1;
        let source = layer.source().ok_or_else(|| {
            format!(
                "assembly oci image '{}' layer {number} must set tar or tree",
                oci.id
            )
        })?;
        let uncompressed = match source {
            AssemblyOciLayerSource::Tar(template) => {
                let path = roots.resolve_path(spec, template.as_str())?;
                if !path.is_file() {
                    return Err(format!(
                        "assembly oci image '{}' layer {number}: tar '{}' does not exist or is not a file",
                        oci.id,
                        path.display()
                    )
                    .into());
                }
                let diff_id = file_sha256(&path)?;
                (format!("tar {}", template.as_str()), path, diff_id)
            }
            AssemblyOciLayerSource::Tree(tree) => {
                let root = roots.tree_path(tree.as_str())?.to_path_buf();
                let path = build.layers_dir.join(format!("layer-{number}.tar"));
                std_fs::create_dir_all(build.layers_dir).map_err(|error| {
                    format!("failed to create '{}': {error}", build.layers_dir.display())
                })?;
                let diff_id = write_tree_tar(&root, &path).map_err(|error| {
                    format!(
                        "assembly oci image '{}' layer {number} (tree '{tree}'): {error}",
                        oci.id
                    )
                })?;
                (format!("tree {tree}"), path, diff_id)
            }
        };
        let (label, path, diff_id) = uncompressed;
        let blob = layer_blob(
            spec,
            roots,
            build.compression,
            build.cache_dir,
            build.cancel.clone(),
            &label,
            path,
            diff_id,
        )?;
        layers.push(blob);
    }

    let created = created_time();
    let config = image_config(oci, &layers, &created);
    let config_bytes = to_json(&config)?;
    let config_digest = sha256_hex(&config_bytes);
    let manifest = manifest_json(
        &layers,
        build.compression,
        &config_digest,
        config_bytes.len(),
    );
    let manifest_bytes = to_json(&manifest)?;
    let manifest_digest = sha256_hex(&manifest_bytes);

    let layer_summaries = layers
        .iter()
        .map(|layer| OciLayerSummary {
            label: layer.label.clone(),
            diff_id: layer.diff_id.clone(),
            blob_digest: layer.blob_digest.clone(),
            blob_bytes: layer.blob_bytes,
            reused: layer.reused,
        })
        .collect();
    let reference = oci.reference.as_deref();
    let bytes = match build.format {
        AssemblyOciFormatSpec::OciLayout => write_layout(
            &build,
            &layers,
            &config_digest,
            &config_bytes,
            &manifest_digest,
            &manifest_bytes,
            reference,
        )?,
        AssemblyOciFormatSpec::DockerArchive => {
            write_docker_archive(&build, &layers, &config_digest, &config_bytes, reference)?
        }
    };
    Ok(OciSummary {
        output: build.output.to_path_buf(),
        bytes,
        config_digest,
        manifest_digest,
        layers: layer_summaries,
    })
}

/// The compressed blob of one layer, from the cache when it is there. The
/// uncompressed tar is `path`, whose digest is `diff_id`.
#[allow(clippy::too_many_arguments)]
fn layer_blob(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    compression: AssemblyOciCompressionSpec,
    cache_dir: &Path,
    cancel: Option<gaia_process::ProcessCancelCheck>,
    label: &str,
    path: PathBuf,
    diff_id: String,
) -> Result<LayerBlob, AssemblyError> {
    // Fixed levels and one thread: the same input gives the same bytes.
    let (extension, tool, args): (&str, &str, Vec<&str>) = match compression {
        AssemblyOciCompressionSpec::None => {
            let blob_bytes = file_len(&path)?;
            return Ok(LayerBlob {
                label: label.to_string(),
                blob_digest: diff_id.clone(),
                diff_id,
                blob_bytes,
                path,
                reused: false,
            });
        }
        AssemblyOciCompressionSpec::Gzip => ("gzip", "gzip", vec!["-n", "-c", "-6"]),
        AssemblyOciCompressionSpec::Zstd => ("zstd", "zstd", vec!["-q", "-c", "-3"]),
    };
    let cached = cache_dir.join(format!("{diff_id}.{extension}"));
    let reused = {
        let _guard = BLOB_CACHE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if cached.is_file() {
            true
        } else {
            compress_to_dest(
                spec,
                roots,
                tool,
                &args,
                path.clone(),
                cached.clone(),
                cancel,
            )?;
            false
        }
    };
    let blob_digest = file_sha256(&cached)?;
    let blob_bytes = file_len(&cached)?;
    Ok(LayerBlob {
        label: label.to_string(),
        diff_id,
        blob_digest,
        blob_bytes,
        path: cached,
        reused,
    })
}

/// Writes the OCI image layout into the stage and publishes it.
fn write_layout(
    build: &Build<'_>,
    layers: &[LayerBlob],
    config_digest: &str,
    config_bytes: &[u8],
    manifest_digest: &str,
    manifest_bytes: &[u8],
    reference: Option<&str>,
) -> Result<u64, AssemblyError> {
    let blobs = build.stage.join("blobs").join("sha256");
    std_fs::create_dir_all(&blobs)
        .map_err(|error| format!("failed to create '{}': {error}", blobs.display()))?;
    let mut bytes = 0;
    let mut stored: Vec<&str> = Vec::new();
    for layer in layers {
        // Two identical layers are one blob.
        if !stored.contains(&layer.blob_digest.as_str()) {
            bytes += place_blob(&layer.path, &blobs.join(&layer.blob_digest), build)?;
            stored.push(layer.blob_digest.as_str());
        }
    }
    write_document(&blobs.join(config_digest), config_bytes)?;
    write_document(&blobs.join(manifest_digest), manifest_bytes)?;
    bytes += (config_bytes.len() + manifest_bytes.len()) as u64;

    let mut manifest = serde_json::json!({
        "mediaType": OCI_MANIFEST_MEDIA_TYPE,
        "digest": format!("sha256:{manifest_digest}"),
        "size": manifest_bytes.len(),
    });
    if let Some(reference) = reference {
        manifest["annotations"] = serde_json::json!({
            "org.opencontainers.image.ref.name": reference,
        });
    }
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_INDEX_MEDIA_TYPE,
        "manifests": [manifest],
    });
    let index_bytes = to_json(&index)?;
    write_document(&build.stage.join("index.json"), &index_bytes)?;
    let layout_bytes = b"{\"imageLayoutVersion\":\"1.0.0\"}\n";
    write_document(&build.stage.join("oci-layout"), layout_bytes)?;
    bytes += (index_bytes.len() + layout_bytes.len()) as u64;

    publish_dir(build.stage, build.output)?;
    Ok(bytes)
}

/// Writes the `docker load` tarball: the config, each layer under the digest
/// of its blob (once per distinct blob), and `manifest.json`.
fn write_docker_archive(
    build: &Build<'_>,
    layers: &[LayerBlob],
    config_digest: &str,
    config_bytes: &[u8],
    reference: Option<&str>,
) -> Result<u64, AssemblyError> {
    let file = std_fs::File::create(build.stage).map_err(|error| {
        format!(
            "failed to create docker archive '{}': {error}",
            build.stage.display()
        )
    })?;
    let mut writer = TarWriter::new(BufWriter::new(file));
    let config_name = format!("{config_digest}.json");
    append_bytes(&mut writer, &config_name, config_bytes)?;
    let mut layer_names = Vec::with_capacity(layers.len());
    let mut written: Vec<&str> = Vec::new();
    for layer in layers {
        let name = format!("{}/layer.tar", layer.blob_digest);
        if !written.contains(&layer.blob_digest.as_str()) {
            let mut reader = std_fs::File::open(&layer.path)
                .map_err(|error| format!("failed to open '{}': {error}", layer.path.display()))?;
            writer.append_file(&name, 0o644, layer.blob_bytes, &mut reader, |_| {})?;
            written.push(layer.blob_digest.as_str());
        }
        layer_names.push(name);
    }
    let manifest = serde_json::json!([{
        "Config": config_name,
        "RepoTags": reference.map_or(serde_json::Value::Null, |reference| {
            serde_json::json!([reference])
        }),
        "Layers": layer_names,
    }]);
    let manifest_bytes = to_json(&manifest)?;
    append_bytes(&mut writer, "manifest.json", &manifest_bytes)?;
    let mut inner = writer.finish()?;
    inner.flush().map_err(|error| {
        format!(
            "failed to flush docker archive '{}': {error}",
            build.stage.display()
        )
    })?;
    let file = inner.into_inner().map_err(|error| {
        format!(
            "failed to flush docker archive '{}': {error}",
            build.stage.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "failed to sync docker archive '{}': {error}",
            build.stage.display()
        )
    })?;
    let bytes = file_len(build.stage)?;
    publish_assembly_output(build.stage, build.output)?;
    Ok(bytes)
}

fn append_bytes(
    writer: &mut TarWriter<BufWriter<std_fs::File>>,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    writer.append_file(name, 0o644, bytes.len() as u64, &mut &bytes[..], |_| {})
}

/// The image config document. Empty settings are left out.
fn image_config(oci: &AssemblyOciSpec, layers: &[LayerBlob], created: &str) -> serde_json::Value {
    let settings = &oci.config;
    let mut container = serde_json::Map::new();
    if !settings.entrypoint.is_empty() {
        container.insert("Entrypoint".into(), serde_json::json!(settings.entrypoint));
    }
    if !settings.cmd.is_empty() {
        container.insert("Cmd".into(), serde_json::json!(settings.cmd));
    }
    if !settings.env.is_empty() {
        container.insert("Env".into(), serde_json::json!(settings.env));
    }
    if let Some(working_dir) = &settings.working_dir {
        container.insert("WorkingDir".into(), serde_json::json!(working_dir));
    }
    if let Some(stop_signal) = &settings.stop_signal {
        container.insert("StopSignal".into(), serde_json::json!(stop_signal));
    }
    if !settings.labels.is_empty() {
        let labels: serde_json::Map<String, serde_json::Value> = settings
            .labels
            .iter()
            .map(|(key, value)| (key.clone(), serde_json::json!(value)))
            .collect();
        container.insert("Labels".into(), serde_json::Value::Object(labels));
    }
    let diff_ids: Vec<String> = layers
        .iter()
        .map(|layer| format!("sha256:{}", layer.diff_id))
        .collect();
    let history: Vec<serde_json::Value> = layers
        .iter()
        .map(|layer| {
            serde_json::json!({
                "created": created,
                "created_by": format!("gaia assembly oci layer ({})", layer.label),
            })
        })
        .collect();
    serde_json::json!({
        "created": created,
        "architecture": settings.architecture,
        "os": settings.os,
        "config": container,
        "rootfs": { "type": "layers", "diff_ids": diff_ids },
        "history": history,
    })
}

fn manifest_json(
    layers: &[LayerBlob],
    compression: AssemblyOciCompressionSpec,
    config_digest: &str,
    config_len: usize,
) -> serde_json::Value {
    let layer_entries: Vec<serde_json::Value> = layers
        .iter()
        .map(|layer| {
            serde_json::json!({
                "mediaType": compression.layer_media_type(),
                "digest": format!("sha256:{}", layer.blob_digest),
                "size": layer.blob_bytes,
            })
        })
        .collect();
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": OCI_CONFIG_MEDIA_TYPE,
            "digest": format!("sha256:{config_digest}"),
            "size": config_len,
        },
        "layers": layer_entries,
    })
}

/// Copies a blob into the stage: a hard link when the source is an immutable
/// cache or staging file, else a copy (a layer tar the spec names may change
/// in place). Returns the blob's size.
fn place_blob(source: &Path, target: &Path, build: &Build<'_>) -> Result<u64, AssemblyError> {
    let bytes = file_len(source)?;
    if target.exists() {
        return Ok(bytes);
    }
    let immutable = source.starts_with(build.cache_dir) || source.starts_with(build.layers_dir);
    let linked = immutable && std_fs::hard_link(source, target).is_ok();
    if !linked {
        std_fs::copy(source, target).map_err(|error| {
            format!(
                "failed to copy blob '{}' to '{}': {error}",
                source.display(),
                target.display()
            )
        })?;
    }
    Ok(bytes)
}

fn write_document(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std_fs::write(path, bytes)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))
}

fn to_json(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| format!("failed to encode OCI json: {error}"))
}

/// Replaces `output` (a directory) with `stage`. The previous one is kept
/// until the new one is in place.
fn publish_dir(stage: &Path, output: &Path) -> Result<(), String> {
    let backup = gaia_image_providers::temporary_publish_backup_path(output, "oci-output");
    remove_path(&backup);
    let had_previous = std_fs::symlink_metadata(output).is_ok();
    if had_previous {
        std_fs::rename(output, &backup).map_err(|error| {
            format!(
                "failed to move the previous oci output '{}' aside: {error}",
                output.display()
            )
        })?;
    }
    if let Err(error) = std_fs::rename(stage, output) {
        if had_previous {
            let _ = std_fs::rename(&backup, output);
        }
        return Err(format!(
            "failed to publish oci output '{}' from '{}': {error}",
            output.display(),
            stage.display()
        ));
    }
    remove_path(&backup);
    Ok(())
}

fn remove_path(path: &Path) {
    match std_fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            let _ = std_fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std_fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Tars the tree at `root` into `tar`, deterministically: sorted entries,
/// directories before their contents, mtime 0, uid and gid 0, the permission
/// bits kept, symlinks kept as links, and a second name for an inode as a
/// hard link. Returns the sha256 of the tar (the layer's diff_id).
pub(super) fn write_tree_tar(root: &Path, tar: &Path) -> Result<String, String> {
    let file = std_fs::File::create(tar)
        .map_err(|error| format!("failed to create '{}': {error}", tar.display()))?;
    let mut writer = TarWriter::new(HashingWriter::new(BufWriter::new(file)));
    let mut first_names = HashMap::new();
    append_tree(&mut writer, root, "", &mut first_names)?;
    let hashing = writer.finish()?;
    hashing
        .finish()
        .map_err(|error| format!("failed to write '{}': {error}", tar.display()))
}

fn append_tree(
    writer: &mut TarWriter<HashingWriter<BufWriter<std_fs::File>>>,
    dir: &Path,
    prefix: &str,
    first_names: &mut HashMap<(u64, u64), (String, u32)>,
) -> Result<(), String> {
    let read = std_fs::read_dir(dir)
        .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
    let mut children = Vec::new();
    for entry in read {
        children.push(
            entry
                .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?
                .path(),
        );
    }
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("'{}' has no UTF-8 file name", child.display()))?
            .to_string();
        let member = format!("{prefix}{name}");
        let metadata = std_fs::symlink_metadata(&child)
            .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
        let file_type = metadata.file_type();
        let mode = metadata.mode() & 0o7777;
        if file_type.is_dir() {
            writer.append_dir_mode(&format!("{member}/"), mode)?;
            append_tree(writer, &child, &format!("{member}/"), first_names)?;
        } else if file_type.is_symlink() {
            let target = std_fs::read_link(&child)
                .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
            let target = target
                .to_str()
                .ok_or_else(|| format!("symlink '{}' target is not UTF-8", child.display()))?;
            writer.append_symlink(&member, target)?;
        } else if file_type.is_file() {
            if metadata.nlink() > 1 {
                let key = (metadata.dev(), metadata.ino());
                // A link carries the mode of the file it links to.
                if let Some((first, _)) = first_names.get(&key) {
                    writer.append_hardlink(&member, first, mode)?;
                    continue;
                }
                first_names.insert(key, (member.clone(), mode));
            }
            let mut reader = std_fs::File::open(&child)
                .map_err(|error| format!("failed to open '{}': {error}", child.display()))?;
            writer.append_file(&member, mode, metadata.len(), &mut reader, |_| {})?;
        } else {
            return Err(format!(
                "'{}' is not a regular file, directory or symlink; device nodes and fifos cannot go in an OCI layer",
                child.display()
            ));
        }
    }
    Ok(())
}

/// A file writer that hashes what it writes, for the tree tar's digest.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: sha2::Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: sha2::Sha256::new(),
        }
    }
}

impl HashingWriter<BufWriter<std_fs::File>> {
    /// Flushes and syncs the file under the writer, and returns the digest.
    fn finish(mut self) -> io::Result<String> {
        self.inner.flush()?;
        self.inner.get_ref().sync_all()?;
        Ok(hex(self.hasher.finalize().as_slice()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(sha2::Sha256::digest(bytes).as_slice())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The created time: `SOURCE_DATE_EPOCH` when set, else the epoch, so the
/// same layers give the same image.
fn created_time() -> String {
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_CREATED);
    rfc3339_utc(seconds)
}

/// `seconds` since the epoch as RFC 3339 UTC.
pub(super) fn rfc3339_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// The state keys of an OCI image, under `oci.<index>.`.
pub(super) fn record_oci_state(
    state: &mut KeyValueState,
    index: usize,
    oci: &AssemblyOciSpec,
    summary: &OciSummary,
) {
    let oci_state = AssemblyStateKey::new("oci", index);
    state.insert(oci_state.field("id"), oci.id.as_str());
    state.insert(oci_state.field("format"), oci.format.as_str());
    state.insert(oci_state.field("compression"), oci.compression.as_str());
    state.insert(
        oci_state.field("output"),
        summary.output.display().to_string(),
    );
    state.insert(oci_state.field("bytes"), summary.bytes);
    state.insert(oci_state.field("config_digest"), &summary.config_digest);
    state.insert(oci_state.field("manifest_digest"), &summary.manifest_digest);
    state.insert(oci_state.field("layer_count"), summary.layers.len());
    for (layer_index, layer) in summary.layers.iter().enumerate() {
        let field = |name: &str| oci_state.child_field("layer", layer_index + 1, name);
        state.insert(field("source"), &layer.label);
        state.insert(field("diff_id"), &layer.diff_id);
        state.insert(field("blob_digest"), &layer.blob_digest);
        state.insert(field("blob_bytes"), layer.blob_bytes);
        state.insert(field("reused"), layer.reused);
    }
}
