use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::ExecCtx;
use crate::planner::Plan;
use crate::workspace::{WorkspaceConfig, WorkspacePaths};

const SUPPORTED_ANCHORS: &[&str] = &["buildroot.build"];
const DEFAULT_BUILDROOT_BASE_FINGERPRINT_PATHS: &[&str] = &[
    "buildroot.version",
    "buildroot.defconfig",
    "buildroot.packages",
    "buildroot.package_versions",
    "buildroot.symbols",
    "buildroot.external",
    "buildroot.starting_point",
    "buildroot.rpi.arch",
    "buildroot.rpi.board",
    "buildroot.rpi.defconfig",
    "buildroot.rpi.overlay",
    "buildroot.rpi.config_file",
    "buildroot.rpi.cmdline_file",
];

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointUsePolicy {
    Auto,
    Off,
    Required,
}

impl Default for CheckpointUsePolicy {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointUploadPolicy {
    Off,
    OnSuccess,
    Always,
}

impl Default for CheckpointUploadPolicy {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTrustMode {
    Verify,
    Permissive,
}

impl Default for CheckpointTrustMode {
    fn default() -> Self {
        Self::Verify
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct S3BackendConfig {
    pub bucket: String,
    pub bucket_env: Option<String>,
    pub region: Option<String>,
    pub region_env: Option<String>,
    pub prefix: Option<String>,
    pub prefix_env: Option<String>,
    pub endpoint_url: Option<String>,
    pub endpoint_url_env: Option<String>,
    pub profile: Option<String>,
    pub profile_env: Option<String>,
    pub aws_access_key_id_env: Option<String>,
    pub aws_secret_access_key_env: Option<String>,
    pub aws_session_token_env: Option<String>,
    pub aws_shared_credentials_file_env: Option<String>,
    pub aws_config_file_env: Option<String>,
    pub aws_ca_bundle_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct HttpBackendConfig {
    pub base_url: String,
    pub base_url_env: Option<String>,
    pub token: Option<String>,
    pub token_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct SshBackendConfig {
    // "user@host:/base/path"
    pub target: String,
    pub target_env: Option<String>,
    pub port: Option<u16>,
    pub port_env: Option<String>,
    pub identity_file: Option<String>,
    pub identity_file_env: Option<String>,
    pub known_hosts_file: Option<String>,
    pub known_hosts_file_env: Option<String>,
    pub strict_host_key_checking: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct CheckpointBackendsConfig {
    pub s3: BTreeMap<String, S3BackendConfig>,
    pub http: BTreeMap<String, HttpBackendConfig>,
    pub ssh: BTreeMap<String, SshBackendConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CheckpointPointConfig {
    pub id: String,
    pub anchor_task: String,
    pub use_policy: Option<CheckpointUsePolicy>,
    pub upload_policy: Option<CheckpointUploadPolicy>,
    pub fingerprint_from: Vec<String>,
    pub backend: Option<String>,
    pub trust_mode: Option<CheckpointTrustMode>,
}

impl Default for CheckpointPointConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            anchor_task: String::new(),
            use_policy: None,
            upload_policy: None,
            fingerprint_from: Vec::new(),
            backend: None,
            trust_mode: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CheckpointsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    pub default_use_policy: CheckpointUsePolicy,
    pub default_upload_policy: CheckpointUploadPolicy,
    pub trust_mode: CheckpointTrustMode,
    pub queue_file: Option<String>,
    pub points: Vec<CheckpointPointConfig>,
    pub backends: CheckpointBackendsConfig,
}

impl Default for CheckpointsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_use_policy: CheckpointUsePolicy::Auto,
            default_upload_policy: CheckpointUploadPolicy::Off,
            trust_mode: CheckpointTrustMode::Verify,
            queue_file: None,
            points: Vec::new(),
            backends: CheckpointBackendsConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointTarget {
    pub name: String,
    pub path: PathBuf,
}

impl CheckpointTarget {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub version: u32,
    pub id: String,
    pub anchor_task: String,
    pub fingerprint: String,
    pub lineage: String,
    pub created_at: String,
    pub trust_mode: CheckpointTrustMode,
    #[serde(default)]
    pub fingerprint_inputs: BTreeMap<String, serde_json::Value>,
    pub targets: Vec<CheckpointManifestTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifestTarget {
    pub name: String,
    pub payload_rel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointIndexDoc {
    version: u32,
    points: BTreeMap<String, CheckpointIndexEntry>,
}

impl Default for CheckpointIndexDoc {
    fn default() -> Self {
        Self {
            version: 1,
            points: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointIndexEntry {
    id: String,
    anchor_task: String,
    latest_fingerprint: String,
    latest_manifest_rel: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UploadState {
    Pending,
    Uploaded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadQueueDoc {
    version: u32,
    entries: Vec<UploadQueueEntry>,
}

impl Default for UploadQueueDoc {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadQueueEntry {
    id: String,
    anchor_task: String,
    fingerprint: String,
    backend_ref: String,
    object_rel_dir: String,
    state: UploadState,
    attempts: u32,
    last_error: Option<String>,
    updated_at: String,
}

#[derive(Debug, Clone)]
pub struct CheckpointStatus {
    pub id: String,
    pub anchor_task: String,
    pub use_policy: CheckpointUsePolicy,
    pub upload_policy: CheckpointUploadPolicy,
    pub backend: Option<String>,
    pub fingerprint: String,
    pub exists: bool,
    pub remote_exists: Option<bool>,
    pub remote_error: Option<String>,
    pub will_use: bool,
    pub will_download: bool,
    pub will_rebuild: bool,
    pub will_upload: bool,
    pub pending_upload: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CheckpointInventory {
    pub id: String,
    pub anchor_task: String,
    pub backend: Option<String>,
    pub current_fingerprint: String,
    pub local_fingerprints: Vec<String>,
    pub local_latest: Option<String>,
    pub remote_fingerprints: Vec<String>,
    pub remote_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RetryReport {
    pub attempted: usize,
    pub uploaded: usize,
    pub failed: usize,
}

#[derive(Debug, Clone)]
struct PointRuntime<'a> {
    point: &'a CheckpointPointConfig,
    use_policy: CheckpointUsePolicy,
    upload_policy: CheckpointUploadPolicy,
    trust_mode: CheckpointTrustMode,
}

fn load_cfg(doc: &ConfigDoc) -> Result<CheckpointsConfig> {
    Ok(doc.deserialize_path("checkpoints")?.unwrap_or_default())
}

fn workspace_paths_for_doc(doc: &ConfigDoc) -> Result<WorkspacePaths> {
    let ws: WorkspaceConfig = doc.deserialize_path("workspace")?.unwrap_or_default();
    crate::workspace::load_paths(&ws)
}

fn safe_id(id: &str) -> Result<String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(Error::msg("checkpoint id is empty"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(Error::msg(format!(
            "checkpoint id '{}' contains invalid characters",
            id
        )));
    }
    Ok(id.to_string())
}

fn store_root(ws: &WorkspacePaths) -> PathBuf {
    ws.build_dir.join("checkpoints")
}

fn points_root(ws: &WorkspacePaths) -> PathBuf {
    store_root(ws).join("points")
}

fn queue_path(ws: &WorkspacePaths, cfg: &CheckpointsConfig) -> Result<PathBuf> {
    if let Some(raw) = cfg
        .queue_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return ws.resolve_config_path(raw);
    }
    Ok(store_root(ws).join("upload-queue.json"))
}

fn index_path(ws: &WorkspacePaths) -> PathBuf {
    store_root(ws).join("index.json")
}

fn object_dir_for(ws: &WorkspacePaths, point_id: &str, fingerprint: &str) -> Result<PathBuf> {
    let id = safe_id(point_id)?;
    Ok(points_root(ws).join(id).join(fingerprint))
}

fn payload_dir_for_object(object_dir: &Path) -> PathBuf {
    object_dir.join("payload")
}

fn manifest_path_for_object(object_dir: &Path) -> PathBuf {
    object_dir.join("manifest.json")
}

fn payload_archive_for_object(object_dir: &Path) -> PathBuf {
    object_dir.join("payload.tar")
}

fn marker_name(anchor_task: &str) -> String {
    let mut s = String::with_capacity(anchor_task.len());
    for c in anchor_task.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            s.push(c);
        } else {
            s.push('_');
        }
    }
    format!(".checkpoint-restored-{s}.json")
}

fn marker_path(doc: &ConfigDoc, ctx: &ExecCtx, anchor_task: &str) -> Result<PathBuf> {
    let run_dir = crate::modules::util::gaia_run_dir(doc, ctx)?;
    Ok(run_dir.join(marker_name(anchor_task)))
}

fn default_fingerprint_paths(anchor_task: &str) -> Vec<String> {
    if anchor_task == "buildroot.build" {
        return DEFAULT_BUILDROOT_BASE_FINGERPRINT_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    Vec::new()
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let mut out = serde_json::Map::new();
            for (k, v) in tbl {
                out.insert(k.clone(), toml_to_json(v));
            }
            serde_json::Value::Object(out)
        }
    }
}

fn compute_point_fingerprint(doc: &ConfigDoc, point: &CheckpointPointConfig) -> Result<String> {
    let selected = selected_fingerprint_inputs(doc, point);
    compute_fingerprint_for_selected(point, &selected)
}

fn selected_fingerprint_inputs(
    doc: &ConfigDoc,
    point: &CheckpointPointConfig,
) -> BTreeMap<String, serde_json::Value> {
    let mut selected = BTreeMap::<String, serde_json::Value>::new();
    let paths = if point.fingerprint_from.is_empty() {
        default_fingerprint_paths(point.anchor_task.trim())
    } else {
        point.fingerprint_from.clone()
    };

    for raw in paths {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        let v = doc
            .value_path(path)
            .map(toml_to_json)
            .unwrap_or(serde_json::Value::Null);
        selected.insert(path.to_string(), v);
    }
    selected
}

fn compute_fingerprint_for_selected(
    point: &CheckpointPointConfig,
    selected: &BTreeMap<String, serde_json::Value>,
) -> Result<String> {
    let payload = serde_json::json!({
        "id": point.id.trim(),
        "anchor_task": point.anchor_task.trim(),
        "selected": selected,
    });
    let encoded = serde_json::to_vec(&payload)
        .map_err(|e| Error::msg(format!("checkpoint fingerprint encode failed: {e}")))?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&encoded);
    Ok(hex::encode(hasher.finalize()))
}

fn compute_lineage(anchor_task: &str, fingerprint: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(anchor_task.as_bytes());
    hasher.update(b"\n");
    hasher.update(fingerprint.as_bytes());
    hex::encode(hasher.finalize())
}

fn effective_point_runtime<'a>(
    cfg: &'a CheckpointsConfig,
    point: &'a CheckpointPointConfig,
) -> PointRuntime<'a> {
    PointRuntime {
        point,
        use_policy: point.use_policy.unwrap_or(cfg.default_use_policy),
        upload_policy: point.upload_policy.unwrap_or(cfg.default_upload_policy),
        trust_mode: point.trust_mode.unwrap_or(cfg.trust_mode),
    }
}

fn find_point_for_anchor<'a>(
    cfg: &'a CheckpointsConfig,
    anchor_task: &str,
) -> Result<Option<PointRuntime<'a>>> {
    let mut matches = cfg
        .points
        .iter()
        .filter(|p| p.anchor_task.trim() == anchor_task)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() > 1 {
        let ids = matches
            .iter()
            .map(|p| p.id.trim().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::msg(format!(
            "multiple checkpoints map to anchor '{}': {}",
            anchor_task, ids
        )));
    }
    let point = matches.remove(0);
    Ok(Some(effective_point_runtime(cfg, point)))
}

fn load_index(ws: &WorkspacePaths) -> Result<CheckpointIndexDoc> {
    let p = index_path(ws);
    if !p.is_file() {
        return Ok(CheckpointIndexDoc::default());
    }
    let raw = fs::read_to_string(&p).map_err(|e| {
        Error::msg(format!(
            "failed to read checkpoint index {}: {e}",
            p.display()
        ))
    })?;
    serde_json::from_str::<CheckpointIndexDoc>(&raw).map_err(|e| {
        Error::msg(format!(
            "failed to parse checkpoint index {}: {e}",
            p.display()
        ))
    })
}

fn atomic_write_text(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    }
    let file_name = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        Error::msg(format!(
            "invalid file path for atomic write: {}",
            path.display()
        ))
    })?;
    let tmp = path.with_file_name(format!(
        ".{}.tmp.{}.{}",
        file_name,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::write(&tmp, body)
        .map_err(|e| Error::msg(format!("failed to write temp file {}: {e}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|e| {
        Error::msg(format!(
            "failed to rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

fn save_index(ws: &WorkspacePaths, idx: &CheckpointIndexDoc) -> Result<()> {
    let p = index_path(ws);
    let body = serde_json::to_string_pretty(idx)
        .map_err(|e| Error::msg(format!("failed to encode checkpoint index: {e}")))?;
    atomic_write_text(&p, &body).map_err(|e| {
        Error::msg(format!(
            "failed to write checkpoint index {}: {e}",
            p.display()
        ))
    })
}

fn load_queue(path: &Path) -> Result<UploadQueueDoc> {
    if !path.is_file() {
        return Ok(UploadQueueDoc::default());
    }
    let raw = fs::read_to_string(path).map_err(|e| {
        Error::msg(format!(
            "failed to read upload queue {}: {e}",
            path.display()
        ))
    })?;
    serde_json::from_str::<UploadQueueDoc>(&raw).map_err(|e| {
        Error::msg(format!(
            "failed to parse upload queue {}: {e}",
            path.display()
        ))
    })
}

fn save_queue(path: &Path, q: &UploadQueueDoc) -> Result<()> {
    let body = serde_json::to_string_pretty(q)
        .map_err(|e| Error::msg(format!("failed to encode upload queue: {e}")))?;
    atomic_write_text(path, &body).map_err(|e| {
        Error::msg(format!(
            "failed to write upload queue {}: {e}",
            path.display()
        ))
    })
}

struct StoreLock {
    path: PathBuf,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_store_lock(ws: &WorkspacePaths) -> Result<StoreLock> {
    let path = store_root(ws).join(".store.lock");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(StoreLock { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    return Err(Error::msg(format!(
                        "timed out waiting for checkpoint store lock {}",
                        path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(Error::msg(format!(
                    "failed to acquire checkpoint store lock {}: {e}",
                    path.display()
                )));
            }
        }
    }
}

fn enqueue_upload(
    ws: &WorkspacePaths,
    cfg: &CheckpointsConfig,
    point: &CheckpointPointConfig,
    fingerprint: &str,
    backend_ref: &str,
    object_dir: &Path,
    err: &str,
) -> Result<()> {
    let qpath = queue_path(ws, cfg)?;
    let mut q = load_queue(&qpath)?;
    let rel = object_dir
        .strip_prefix(store_root(ws))
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| object_dir.to_string_lossy().to_string());

    if let Some(existing) = q.entries.iter_mut().find(|e| {
        e.id == point.id.trim() && e.fingerprint == fingerprint && e.backend_ref == backend_ref
    }) {
        existing.state = UploadState::Failed;
        existing.last_error = Some(err.to_string());
        existing.attempts = existing.attempts.saturating_add(1);
        existing.updated_at = chrono::Utc::now().to_rfc3339();
        return save_queue(&qpath, &q);
    }

    q.entries.push(UploadQueueEntry {
        id: point.id.trim().to_string(),
        anchor_task: point.anchor_task.trim().to_string(),
        fingerprint: fingerprint.to_string(),
        backend_ref: backend_ref.to_string(),
        object_rel_dir: rel,
        state: UploadState::Failed,
        attempts: 1,
        last_error: Some(err.to_string()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    });
    save_queue(&qpath, &q)
}

fn pending_upload_exists(
    ws: &WorkspacePaths,
    cfg: &CheckpointsConfig,
    point_id: &str,
) -> Result<bool> {
    let qpath = queue_path(ws, cfg)?;
    let q = load_queue(&qpath)?;
    Ok(q.entries
        .iter()
        .any(|e| e.id == point_id && matches!(e.state, UploadState::Pending | UploadState::Failed)))
}

fn clear_dir_or_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                fs::remove_dir_all(path).map_err(|e| {
                    Error::msg(format!(
                        "failed to remove directory {}: {e}",
                        path.display()
                    ))
                })?;
            } else {
                fs::remove_file(path)
                    .map_err(|e| Error::msg(format!("failed to remove {}: {e}", path.display())))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::msg(format!(
            "failed to inspect {} before cleanup: {e}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    }
    clear_dir_or_file(dst)?;
    let target = fs::read_link(src)
        .map_err(|e| Error::msg(format!("failed to read symlink {}: {e}", src.display())))?;
    symlink(&target, dst).map_err(|e| {
        Error::msg(format!(
            "failed to create symlink {} -> {}: {e}",
            dst.display(),
            target.display()
        ))
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    copy_path(src, dst)
}

fn copy_path(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .map_err(|e| Error::msg(format!("failed to stat {}: {e}", src.display())))?;
    if meta.file_type().is_symlink() {
        return copy_symlink(src, dst);
    }
    if meta.is_dir() {
        fs::create_dir_all(dst)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", dst.display())))?;
        for entry in walkdir::WalkDir::new(src) {
            let entry = entry.map_err(|e| Error::msg(format!("walkdir error: {e}")))?;
            let p = entry.path();
            let rel = p
                .strip_prefix(src)
                .map_err(|e| Error::msg(format!("strip_prefix failed: {e}")))?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out = dst.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&out)
                    .map_err(|e| Error::msg(format!("failed to create {}: {e}", out.display())))?;
            } else if entry.file_type().is_symlink() {
                copy_symlink(p, &out)?;
            } else {
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        Error::msg(format!("failed to create {}: {e}", parent.display()))
                    })?;
                }
                fs::copy(p, &out).map_err(|e| {
                    Error::msg(format!(
                        "failed to copy {} -> {}: {e}",
                        p.display(),
                        out.display()
                    ))
                })?;
            }
        }
        return Ok(());
    }

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    }
    fs::copy(src, dst).map_err(|e| {
        Error::msg(format!(
            "failed to copy {} -> {}: {e}",
            src.display(),
            dst.display()
        ))
    })?;
    Ok(())
}

fn ensure_payload_archive(object_dir: &Path) -> Result<PathBuf> {
    let payload = payload_dir_for_object(object_dir);
    if !payload.is_dir() {
        return Err(Error::msg(format!(
            "checkpoint payload dir missing: {}",
            payload.display()
        )));
    }
    let archive = payload_archive_for_object(object_dir);
    if archive.is_file() {
        return Ok(archive);
    }

    let status = Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(object_dir)
        .arg("payload")
        .status()
        .map_err(|e| Error::msg(format!("failed to spawn tar: {e}")))?;
    if !status.success() {
        return Err(Error::msg(format!(
            "failed to archive checkpoint payload with tar (status: {status})"
        )));
    }
    Ok(archive)
}

fn read_manifest(path: &Path) -> Result<CheckpointManifest> {
    let raw = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
    serde_json::from_str::<CheckpointManifest>(&raw)
        .map_err(|e| Error::msg(format!("failed to parse {}: {e}", path.display())))
}

fn write_manifest(path: &Path, m: &CheckpointManifest) -> Result<()> {
    let body = serde_json::to_string_pretty(m)
        .map_err(|e| Error::msg(format!("failed to encode checkpoint manifest: {e}")))?;
    atomic_write_text(path, &body)
        .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))
}

#[derive(Debug, Clone)]
enum BackendResolved<'a> {
    S3(String, &'a S3BackendConfig),
    Http(String, &'a HttpBackendConfig),
    Ssh(String, &'a SshBackendConfig),
}

fn resolve_backend<'a>(
    cfg: &'a CheckpointsConfig,
    backend_ref: &str,
) -> Result<BackendResolved<'a>> {
    let backend_ref = backend_ref.trim();
    if backend_ref.is_empty() {
        return Err(Error::msg("empty checkpoint backend reference"));
    }

    if let Some((kind, name)) = backend_ref.split_once(':') {
        let k = kind.trim();
        let n = name.trim();
        return match k {
            "s3" => cfg
                .backends
                .s3
                .get(n)
                .map(|v| BackendResolved::S3(n.to_string(), v))
                .ok_or_else(|| Error::msg(format!("unknown checkpoints backend '{}': {}", k, n))),
            "http" => cfg
                .backends
                .http
                .get(n)
                .map(|v| BackendResolved::Http(n.to_string(), v))
                .ok_or_else(|| Error::msg(format!("unknown checkpoints backend '{}': {}", k, n))),
            "ssh" => cfg
                .backends
                .ssh
                .get(n)
                .map(|v| BackendResolved::Ssh(n.to_string(), v))
                .ok_or_else(|| Error::msg(format!("unknown checkpoints backend '{}': {}", k, n))),
            _ => Err(Error::msg(format!(
                "unknown checkpoints backend kind '{}'; expected s3/http/ssh",
                k
            ))),
        };
    }

    let mut hits = Vec::<BackendResolved<'a>>::new();
    if let Some(v) = cfg.backends.s3.get(backend_ref) {
        hits.push(BackendResolved::S3(backend_ref.to_string(), v));
    }
    if let Some(v) = cfg.backends.http.get(backend_ref) {
        hits.push(BackendResolved::Http(backend_ref.to_string(), v));
    }
    if let Some(v) = cfg.backends.ssh.get(backend_ref) {
        hits.push(BackendResolved::Ssh(backend_ref.to_string(), v));
    }

    if hits.is_empty() {
        return Err(Error::msg(format!(
            "unknown checkpoints backend '{}'",
            backend_ref
        )));
    }
    if hits.len() > 1 {
        return Err(Error::msg(format!(
            "ambiguous checkpoints backend '{}'; use kind:name",
            backend_ref
        )));
    }
    Ok(hits.remove(0))
}

fn run_command_capture(mut cmd: Command) -> Result<()> {
    let out = run_command_output(&mut cmd)?;
    if out.status.success() {
        return Ok(());
    }
    let msg = command_summary(&out);
    Err(Error::msg(format!("command failed: {msg}")))
}

fn run_command_output(cmd: &mut Command) -> Result<Output> {
    cmd.output()
        .map_err(|e| Error::msg(format!("failed to run command {:?}: {e}", cmd)))
}

fn command_summary(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    if !stdout.is_empty() {
        return stdout;
    }
    format!("status {}", out.status)
}

fn is_not_found_text(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("not found")
        || m.contains("404")
        || m.contains("no such")
        || m.contains("does not exist")
        || m.contains("could not be found")
}

fn resolve_env_ref(env_key: Option<&str>) -> Option<String> {
    env_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn resolve_string_field(literal: Option<&str>, env_key: Option<&str>) -> Option<String> {
    let direct = literal
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    direct.or_else(|| resolve_env_ref(env_key))
}

fn resolve_required_string_field(
    cfg_path: &str,
    literal: Option<&str>,
    env_key: Option<&str>,
) -> Result<String> {
    resolve_string_field(literal, env_key).ok_or_else(|| {
        if let Some(k) = env_key.map(str::trim).filter(|s| !s.is_empty()) {
            Error::msg(format!("{cfg_path} is empty (also checked env var '{k}')"))
        } else {
            Error::msg(format!("{cfg_path} is empty"))
        }
    })
}

fn resolve_http_base_url(cfg_name: &str, cfg: &HttpBackendConfig) -> Result<String> {
    resolve_required_string_field(
        &format!("checkpoints.backends.http.{}.base_url", cfg_name),
        Some(cfg.base_url.as_str()),
        cfg.base_url_env.as_deref(),
    )
}

fn resolve_http_token(cfg: &HttpBackendConfig) -> Option<String> {
    resolve_string_field(cfg.token.as_deref(), cfg.token_env.as_deref())
}

#[derive(Debug, Clone)]
struct S3ResolvedConfig {
    bucket: String,
    region: Option<String>,
    prefix: Option<String>,
    endpoint_url: Option<String>,
    profile: Option<String>,
    command_env: BTreeMap<String, String>,
}

fn resolve_s3_config(cfg_name: &str, cfg: &S3BackendConfig) -> Result<S3ResolvedConfig> {
    let bucket = resolve_required_string_field(
        &format!("checkpoints.backends.s3.{}.bucket", cfg_name),
        Some(cfg.bucket.as_str()),
        cfg.bucket_env.as_deref(),
    )?;
    let region = resolve_string_field(cfg.region.as_deref(), cfg.region_env.as_deref());
    let prefix = resolve_string_field(cfg.prefix.as_deref(), cfg.prefix_env.as_deref());
    let endpoint_url =
        resolve_string_field(cfg.endpoint_url.as_deref(), cfg.endpoint_url_env.as_deref());
    let profile = resolve_string_field(cfg.profile.as_deref(), cfg.profile_env.as_deref());

    let mut command_env = BTreeMap::<String, String>::new();
    for (dst, src) in [
        ("AWS_ACCESS_KEY_ID", cfg.aws_access_key_id_env.as_deref()),
        (
            "AWS_SECRET_ACCESS_KEY",
            cfg.aws_secret_access_key_env.as_deref(),
        ),
        ("AWS_SESSION_TOKEN", cfg.aws_session_token_env.as_deref()),
        (
            "AWS_SHARED_CREDENTIALS_FILE",
            cfg.aws_shared_credentials_file_env.as_deref(),
        ),
        ("AWS_CONFIG_FILE", cfg.aws_config_file_env.as_deref()),
        ("AWS_CA_BUNDLE", cfg.aws_ca_bundle_env.as_deref()),
    ] {
        if let Some(v) = resolve_env_ref(src) {
            command_env.insert(dst.to_string(), v);
        }
    }

    Ok(S3ResolvedConfig {
        bucket,
        region,
        prefix,
        endpoint_url,
        profile,
        command_env,
    })
}

#[derive(Debug, Clone)]
struct SshResolvedConfig {
    host: String,
    base_path: String,
    port: Option<u16>,
    identity_file: Option<String>,
    known_hosts_file: Option<String>,
    strict_host_key_checking: Option<bool>,
}

fn resolve_ssh_config(cfg_name: &str, cfg: &SshBackendConfig) -> Result<SshResolvedConfig> {
    let target = resolve_required_string_field(
        &format!("checkpoints.backends.ssh.{}.target", cfg_name),
        Some(cfg.target.as_str()),
        cfg.target_env.as_deref(),
    )?;
    let (host, base_path) = target.split_once(':').ok_or_else(|| {
        Error::msg(format!(
            "checkpoints.backends.ssh.{}.target must be 'host:/path'",
            cfg_name
        ))
    })?;
    let host = host.trim().to_string();
    let base_path = base_path.trim().trim_end_matches('/').to_string();

    let port = if let Some(v) = cfg.port {
        Some(v)
    } else {
        resolve_env_ref(cfg.port_env.as_deref())
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<u16>().map_err(|e| {
                    Error::msg(format!(
                        "checkpoints.backends.ssh.{}.port_env parse failed: {e}",
                        cfg_name
                    ))
                })
            })
            .transpose()?
    };

    let identity_file = resolve_string_field(
        cfg.identity_file.as_deref(),
        cfg.identity_file_env.as_deref(),
    );
    let known_hosts_file = resolve_string_field(
        cfg.known_hosts_file.as_deref(),
        cfg.known_hosts_file_env.as_deref(),
    );

    Ok(SshResolvedConfig {
        host,
        base_path,
        port,
        identity_file,
        known_hosts_file,
        strict_host_key_checking: cfg.strict_host_key_checking,
    })
}

fn s3_key_prefix(cfg: &S3ResolvedConfig, point_id: &str, fingerprint: &str) -> String {
    let mut key_prefix = String::new();
    if let Some(prefix) = cfg
        .prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        key_prefix.push_str(prefix.trim_matches('/'));
        key_prefix.push('/');
    }
    key_prefix.push_str(point_id);
    key_prefix.push('/');
    key_prefix.push_str(fingerprint);
    key_prefix
}

fn configure_s3_cli(cmd: &mut Command, cfg: &S3ResolvedConfig) {
    if let Some(profile) = cfg
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("--profile").arg(profile);
    }
    if let Some(region) = cfg
        .region
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("--region").arg(region);
    }
    if let Some(endpoint) = cfg
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("--endpoint-url").arg(endpoint);
    }
    for (k, v) in &cfg.command_env {
        cmd.env(k, v);
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn configure_ssh_cmd(cmd: &mut Command, ssh: &SshResolvedConfig, scp_style: bool) {
    if let Some(port) = ssh.port {
        if scp_style {
            cmd.arg("-P").arg(port.to_string());
        } else {
            cmd.arg("-p").arg(port.to_string());
        }
    }
    if let Some(id) = ssh
        .identity_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("-i").arg(id);
    }
    if let Some(kh) = ssh
        .known_hosts_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("-o").arg(format!("UserKnownHostsFile={kh}"));
    }
    if matches!(ssh.strict_host_key_checking, Some(false)) {
        cmd.arg("-o").arg("StrictHostKeyChecking=no");
        if ssh.known_hosts_file.is_none() {
            cmd.arg("-o").arg("UserKnownHostsFile=/dev/null");
        }
    }
}

fn upload_with_s3(
    cfg_name: &str,
    cfg: &S3BackendConfig,
    point_id: &str,
    fingerprint: &str,
    manifest: &Path,
    archive: &Path,
) -> Result<()> {
    let resolved = resolve_s3_config(cfg_name, cfg)?;

    let key_prefix = s3_key_prefix(&resolved, point_id, fingerprint);
    let bucket = resolved.bucket.as_str();

    let remote_manifest = format!("s3://{bucket}/{key_prefix}/manifest.json");
    let remote_archive = format!("s3://{bucket}/{key_prefix}/payload.tar");

    let mut cmd_manifest = Command::new("aws");
    configure_s3_cli(&mut cmd_manifest, &resolved);
    cmd_manifest
        .arg("s3")
        .arg("cp")
        .arg(manifest)
        .arg(&remote_manifest);
    run_command_capture(cmd_manifest)?;

    let mut cmd_archive = Command::new("aws");
    configure_s3_cli(&mut cmd_archive, &resolved);
    cmd_archive
        .arg("s3")
        .arg("cp")
        .arg(archive)
        .arg(&remote_archive);
    run_command_capture(cmd_archive)?;
    Ok(())
}

fn upload_with_http(
    cfg_name: &str,
    cfg: &HttpBackendConfig,
    point_id: &str,
    fingerprint: &str,
    manifest: &Path,
    archive: &Path,
) -> Result<()> {
    let base = resolve_http_base_url(cfg_name, cfg)?;

    let token = resolve_http_token(cfg);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| Error::msg(format!("failed to build HTTP client: {e}")))?;

    let upload_one = |filename: &str, path: &Path| -> Result<()> {
        let url = format!(
            "{}/{}/{}/{}",
            base.trim_end_matches('/'),
            point_id,
            fingerprint,
            filename
        );
        let body = fs::read(path)
            .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
        let mut req = client.put(url).body(body);
        if let Some(t) = token.as_deref() {
            req = req.bearer_auth(t);
        }
        let res = req
            .send()
            .map_err(|e| Error::msg(format!("HTTP upload failed: {e}")))?;
        if res.status().is_success() {
            return Ok(());
        }
        Err(Error::msg(format!(
            "HTTP upload failed with status {}",
            res.status()
        )))
    };

    upload_one("manifest.json", manifest)?;
    upload_one("payload.tar", archive)?;
    Ok(())
}

fn upload_with_ssh(
    cfg_name: &str,
    cfg: &SshBackendConfig,
    point_id: &str,
    fingerprint: &str,
    manifest: &Path,
    archive: &Path,
) -> Result<()> {
    let ssh = resolve_ssh_config(cfg_name, cfg)?;
    let remote_dir = format!("{}/{point_id}/{fingerprint}", ssh.base_path);

    let mut mkdir_cmd = Command::new("ssh");
    configure_ssh_cmd(&mut mkdir_cmd, &ssh, false);
    mkdir_cmd
        .arg(&ssh.host)
        .arg(format!("mkdir -p {}", sh_quote(&remote_dir)));
    run_command_capture(mkdir_cmd)?;

    let mut scp_manifest = Command::new("scp");
    configure_ssh_cmd(&mut scp_manifest, &ssh, true);
    scp_manifest
        .arg(manifest)
        .arg(format!("{}:{}/manifest.json", ssh.host, remote_dir));
    run_command_capture(scp_manifest)?;

    let mut scp_archive = Command::new("scp");
    configure_ssh_cmd(&mut scp_archive, &ssh, true);
    scp_archive
        .arg(archive)
        .arg(format!("{}:{}/payload.tar", ssh.host, remote_dir));
    run_command_capture(scp_archive)?;

    Ok(())
}

fn extract_payload_archive(object_dir: &Path) -> Result<()> {
    let archive = payload_archive_for_object(object_dir);
    if !archive.is_file() {
        return Err(Error::msg(format!(
            "checkpoint payload archive missing: {}",
            archive.display()
        )));
    }

    let payload = payload_dir_for_object(object_dir);
    clear_dir_or_file(&payload)?;

    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(object_dir)
        .status()
        .map_err(|e| Error::msg(format!("failed to spawn tar: {e}")))?;
    if !status.success() {
        return Err(Error::msg(format!(
            "failed to extract checkpoint payload archive (status: {status})"
        )));
    }
    if !payload.is_dir() {
        return Err(Error::msg(format!(
            "checkpoint archive extracted without payload dir: {}",
            payload.display()
        )));
    }
    Ok(())
}

fn ensure_payload_tree(object_dir: &Path) -> Result<()> {
    let payload = payload_dir_for_object(object_dir);
    if payload.is_dir() {
        return Ok(());
    }
    if payload_archive_for_object(object_dir).is_file() {
        return extract_payload_archive(object_dir);
    }
    Err(Error::msg(format!(
        "checkpoint payload missing under {}",
        object_dir.display()
    )))
}

fn probe_s3_exists(
    cfg_name: &str,
    cfg: &S3BackendConfig,
    point_id: &str,
    fingerprint: &str,
) -> Result<bool> {
    let resolved = resolve_s3_config(cfg_name, cfg)?;
    let key = format!(
        "{}/manifest.json",
        s3_key_prefix(&resolved, point_id, fingerprint)
    );
    let mut cmd = Command::new("aws");
    configure_s3_cli(&mut cmd, &resolved);
    cmd.arg("s3api")
        .arg("head-object")
        .arg("--bucket")
        .arg(&resolved.bucket)
        .arg("--key")
        .arg(&key);
    let out = run_command_output(&mut cmd)?;
    if out.status.success() {
        return Ok(true);
    }
    let msg = command_summary(&out);
    if is_not_found_text(&msg) {
        return Ok(false);
    }
    Err(Error::msg(format!("S3 probe failed: {msg}")))
}

fn probe_http_exists(
    cfg_name: &str,
    cfg: &HttpBackendConfig,
    point_id: &str,
    fingerprint: &str,
) -> Result<bool> {
    let base = resolve_http_base_url(cfg_name, cfg)?;
    let token = resolve_http_token(cfg);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::msg(format!("failed to build HTTP client: {e}")))?;
    let url = format!(
        "{}/{}/{}/manifest.json",
        base.trim_end_matches('/'),
        point_id,
        fingerprint
    );
    let mut req = client.head(url);
    if let Some(t) = token.as_deref() {
        req = req.bearer_auth(t);
    }
    let res = req
        .send()
        .map_err(|e| Error::msg(format!("HTTP probe failed: {e}")))?;
    if res.status().is_success() {
        return Ok(true);
    }
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    Err(Error::msg(format!(
        "HTTP probe failed with status {}",
        res.status()
    )))
}

fn probe_ssh_exists(
    cfg_name: &str,
    cfg: &SshBackendConfig,
    point_id: &str,
    fingerprint: &str,
) -> Result<bool> {
    let ssh = resolve_ssh_config(cfg_name, cfg)?;
    let remote_manifest = format!("{}/{point_id}/{fingerprint}/manifest.json", ssh.base_path);
    let mut cmd = Command::new("ssh");
    configure_ssh_cmd(&mut cmd, &ssh, false);
    cmd.arg(&ssh.host)
        .arg(format!("test -f {}", sh_quote(&remote_manifest)));
    let out = run_command_output(&mut cmd)?;
    if out.status.success() {
        return Ok(true);
    }
    let code = out.status.code().unwrap_or(-1);
    if code == 1 {
        return Ok(false);
    }
    let msg = command_summary(&out);
    Err(Error::msg(format!("SSH probe failed: {msg}")))
}

fn probe_backend_exists(
    cfg: &CheckpointsConfig,
    backend_ref: &str,
    point_id: &str,
    fingerprint: &str,
) -> Result<bool> {
    match resolve_backend(cfg, backend_ref)? {
        BackendResolved::S3(name, b) => probe_s3_exists(&name, b, point_id, fingerprint),
        BackendResolved::Http(name, b) => probe_http_exists(&name, b, point_id, fingerprint),
        BackendResolved::Ssh(name, b) => probe_ssh_exists(&name, b, point_id, fingerprint),
    }
}

fn download_with_s3(
    cfg_name: &str,
    cfg: &S3BackendConfig,
    point_id: &str,
    fingerprint: &str,
    object_dir: &Path,
) -> Result<bool> {
    let resolved = resolve_s3_config(cfg_name, cfg)?;
    let key_prefix = s3_key_prefix(&resolved, point_id, fingerprint);
    let bucket = resolved.bucket.as_str();
    let manifest_remote = format!("s3://{bucket}/{key_prefix}/manifest.json");
    let archive_remote = format!("s3://{bucket}/{key_prefix}/payload.tar");
    let manifest_local = manifest_path_for_object(object_dir);
    let archive_local = payload_archive_for_object(object_dir);
    if let Some(parent) = manifest_local.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    }

    let mut cp_manifest = Command::new("aws");
    configure_s3_cli(&mut cp_manifest, &resolved);
    cp_manifest
        .arg("s3")
        .arg("cp")
        .arg(&manifest_remote)
        .arg(&manifest_local);
    let out = run_command_output(&mut cp_manifest)?;
    if !out.status.success() {
        let msg = command_summary(&out);
        if is_not_found_text(&msg) {
            return Ok(false);
        }
        return Err(Error::msg(format!("S3 download failed: {msg}")));
    }

    let mut cp_archive = Command::new("aws");
    configure_s3_cli(&mut cp_archive, &resolved);
    cp_archive
        .arg("s3")
        .arg("cp")
        .arg(&archive_remote)
        .arg(&archive_local);
    run_command_capture(cp_archive)?;
    extract_payload_archive(object_dir)?;
    Ok(true)
}

fn download_with_http(
    cfg_name: &str,
    cfg: &HttpBackendConfig,
    point_id: &str,
    fingerprint: &str,
    object_dir: &Path,
) -> Result<bool> {
    let base = resolve_http_base_url(cfg_name, cfg)?;
    let token = resolve_http_token(cfg);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| Error::msg(format!("failed to build HTTP client: {e}")))?;

    let fetch = |name: &str| -> Result<Option<Vec<u8>>> {
        let url = format!(
            "{}/{}/{}/{}",
            base.trim_end_matches('/'),
            point_id,
            fingerprint,
            name
        );
        let mut req = client.get(url);
        if let Some(t) = token.as_deref() {
            req = req.bearer_auth(t);
        }
        let res = req
            .send()
            .map_err(|e| Error::msg(format!("HTTP download failed: {e}")))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(Error::msg(format!(
                "HTTP download failed with status {}",
                res.status()
            )));
        }
        let bytes = res
            .bytes()
            .map_err(|e| Error::msg(format!("HTTP body read failed: {e}")))?;
        Ok(Some(bytes.to_vec()))
    };

    let Some(manifest_body) = fetch("manifest.json")? else {
        return Ok(false);
    };
    let Some(payload_body) = fetch("payload.tar")? else {
        return Err(Error::msg(
            "HTTP checkpoint payload.tar missing while manifest exists",
        ));
    };

    fs::create_dir_all(object_dir)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", object_dir.display())))?;
    fs::write(manifest_path_for_object(object_dir), manifest_body)
        .map_err(|e| Error::msg(format!("failed to write manifest: {e}")))?;
    fs::write(payload_archive_for_object(object_dir), payload_body)
        .map_err(|e| Error::msg(format!("failed to write payload archive: {e}")))?;
    extract_payload_archive(object_dir)?;
    Ok(true)
}

fn download_with_ssh(
    cfg_name: &str,
    cfg: &SshBackendConfig,
    point_id: &str,
    fingerprint: &str,
    object_dir: &Path,
) -> Result<bool> {
    let ssh = resolve_ssh_config(cfg_name, cfg)?;
    let remote_prefix = format!("{}/{point_id}/{fingerprint}", ssh.base_path);
    let remote_manifest = format!("{remote_prefix}/manifest.json");
    let remote_archive = format!("{remote_prefix}/payload.tar");

    let mut probe = Command::new("ssh");
    configure_ssh_cmd(&mut probe, &ssh, false);
    probe
        .arg(&ssh.host)
        .arg(format!("test -f {}", sh_quote(&remote_manifest)));
    let out = run_command_output(&mut probe)?;
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        if code == 1 {
            return Ok(false);
        }
        return Err(Error::msg(format!(
            "SSH manifest probe failed: {}",
            command_summary(&out)
        )));
    }

    fs::create_dir_all(object_dir)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", object_dir.display())))?;
    let local_manifest = manifest_path_for_object(object_dir);
    let local_archive = payload_archive_for_object(object_dir);

    let mut scp_manifest = Command::new("scp");
    configure_ssh_cmd(&mut scp_manifest, &ssh, true);
    scp_manifest
        .arg(format!("{}:{remote_manifest}", ssh.host))
        .arg(&local_manifest);
    run_command_capture(scp_manifest)?;

    let mut scp_archive = Command::new("scp");
    configure_ssh_cmd(&mut scp_archive, &ssh, true);
    scp_archive
        .arg(format!("{}:{remote_archive}", ssh.host))
        .arg(&local_archive);
    run_command_capture(scp_archive)?;

    extract_payload_archive(object_dir)?;
    Ok(true)
}

fn download_checkpoint_object(
    cfg: &CheckpointsConfig,
    backend_ref: &str,
    point_id: &str,
    fingerprint: &str,
    object_dir: &Path,
) -> Result<bool> {
    match resolve_backend(cfg, backend_ref)? {
        BackendResolved::S3(name, b) => {
            download_with_s3(&name, b, point_id, fingerprint, object_dir)
        }
        BackendResolved::Http(name, b) => {
            download_with_http(&name, b, point_id, fingerprint, object_dir)
        }
        BackendResolved::Ssh(name, b) => {
            download_with_ssh(&name, b, point_id, fingerprint, object_dir)
        }
    }
}

fn upload_checkpoint_object(
    cfg: &CheckpointsConfig,
    backend_ref: &str,
    point_id: &str,
    fingerprint: &str,
    object_dir: &Path,
) -> Result<()> {
    let manifest = manifest_path_for_object(object_dir);
    let archive = ensure_payload_archive(object_dir)?;

    match resolve_backend(cfg, backend_ref)? {
        BackendResolved::S3(name, b) => {
            upload_with_s3(&name, b, point_id, fingerprint, &manifest, &archive)
        }
        BackendResolved::Http(name, b) => {
            upload_with_http(&name, b, point_id, fingerprint, &manifest, &archive)
        }
        BackendResolved::Ssh(name, b) => {
            upload_with_ssh(&name, b, point_id, fingerprint, &manifest, &archive)
        }
    }
}

fn resolve_index_manifest_path(ws: &WorkspacePaths, rel_or_abs: &str) -> PathBuf {
    let p = PathBuf::from(rel_or_abs);
    if p.is_absolute() {
        return p;
    }
    store_root(ws).join(p)
}

fn diff_fingerprint_inputs(
    old: &BTreeMap<String, serde_json::Value>,
    new: &BTreeMap<String, serde_json::Value>,
) -> Vec<String> {
    let mut keys = BTreeSet::<String>::new();
    keys.extend(old.keys().cloned());
    keys.extend(new.keys().cloned());

    let mut changed = Vec::<String>::new();
    for k in keys {
        if old.get(&k) != new.get(&k) {
            changed.push(k);
        }
    }
    changed
}

fn summarize_changed_inputs(changed: &[String]) -> String {
    if changed.is_empty() {
        return "fingerprint_changed".to_string();
    }
    let preview = changed
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    if changed.len() > 3 {
        return format!("inputs_changed:{}(+{})", preview, changed.len() - 3);
    }
    format!("inputs_changed:{preview}")
}

fn point_status(
    ws: &WorkspacePaths,
    cfg: &CheckpointsConfig,
    rt: &PointRuntime<'_>,
    doc: &ConfigDoc,
) -> Result<CheckpointStatus> {
    let id = safe_id(rt.point.id.trim())?;
    let anchor = rt.point.anchor_task.trim().to_string();
    let selected_inputs = selected_fingerprint_inputs(doc, rt.point);
    let fingerprint = compute_fingerprint_for_selected(rt.point, &selected_inputs)?;
    let lineage = compute_lineage(&anchor, &fingerprint);
    let object_dir = object_dir_for(ws, &id, &fingerprint)?;
    let manifest_path = manifest_path_for_object(&object_dir);

    let mut exists = false;
    let mut reason = String::from("missing");
    if manifest_path.is_file() {
        let m = read_manifest(&manifest_path)?;
        let lineage_ok = m.lineage == lineage && m.fingerprint == fingerprint;
        if lineage_ok || rt.trust_mode == CheckpointTrustMode::Permissive {
            exists = true;
            reason = if lineage_ok {
                "hit".into()
            } else {
                "hit_permissive".into()
            };
        } else {
            reason = "lineage_mismatch".into();
        }
    } else {
        let idx = load_index(ws)?;
        if let Some(ent) = idx.points.get(&id) {
            reason = "fingerprint_changed".into();
            let prev_manifest_path = resolve_index_manifest_path(ws, &ent.latest_manifest_rel);
            if prev_manifest_path.is_file()
                && let Ok(prev_manifest) = read_manifest(&prev_manifest_path)
            {
                let changed =
                    diff_fingerprint_inputs(&prev_manifest.fingerprint_inputs, &selected_inputs);
                reason = summarize_changed_inputs(&changed);
            }
        }
    }

    let mut remote_exists = None::<bool>;
    let mut remote_error = None::<String>;
    if !exists
        && !matches!(rt.use_policy, CheckpointUsePolicy::Off)
        && let Some(back) = rt
            .point
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        match probe_backend_exists(cfg, back, &id, &fingerprint) {
            Ok(true) => {
                remote_exists = Some(true);
                reason = "remote_hit".to_string();
            }
            Ok(false) => {
                remote_exists = Some(false);
                if reason == "missing" {
                    reason = "remote_missing".to_string();
                }
            }
            Err(e) => {
                remote_error = Some(e.to_string());
                reason = "remote_probe_error".to_string();
            }
        }
    }

    let pending = pending_upload_exists(ws, cfg, &id)?;
    let available_for_restore = exists || remote_exists == Some(true);

    let (will_use, will_rebuild, reason) = match rt.use_policy {
        CheckpointUsePolicy::Off => (false, true, "policy_off".to_string()),
        CheckpointUsePolicy::Auto => {
            if available_for_restore {
                (true, false, reason)
            } else {
                (false, true, reason)
            }
        }
        CheckpointUsePolicy::Required => {
            if available_for_restore {
                (true, false, reason)
            } else {
                (false, true, "required_missing".to_string())
            }
        }
    };
    let will_download = will_use && !exists && remote_exists == Some(true);

    let will_upload = will_rebuild
        && !rt.point.anchor_task.trim().is_empty()
        && !matches!(rt.upload_policy, CheckpointUploadPolicy::Off)
        && rt
            .point
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some();

    Ok(CheckpointStatus {
        id,
        anchor_task: anchor,
        use_policy: rt.use_policy,
        upload_policy: rt.upload_policy,
        backend: rt
            .point
            .backend
            .as_deref()
            .map(str::trim)
            .map(ToOwned::to_owned),
        fingerprint,
        exists,
        remote_exists,
        remote_error,
        will_use,
        will_download,
        will_rebuild,
        will_upload,
        pending_upload: pending,
        reason,
    })
}

pub fn format_status_report(items: &[CheckpointStatus]) -> String {
    if items.is_empty() {
        return "checkpoints: none configured".into();
    }
    let mut out = String::new();
    out.push_str("checkpoints status:\n");
    for s in items {
        let remote = s
            .remote_exists
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let remote_err = s
            .remote_error
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("-");
        out.push_str(&format!(
            "- id={} anchor={} use_policy={:?} upload_policy={:?} backend={} exists={} remote_exists={} will_use={} will_download={} will_rebuild={} will_upload={} pending_upload={} reason={} remote_error={} fingerprint={}\n",
            s.id,
            s.anchor_task,
            s.use_policy,
            s.upload_policy,
            s.backend.as_deref().unwrap_or("-"),
            s.exists,
            remote,
            s.will_use,
            s.will_download,
            s.will_rebuild,
            s.will_upload,
            s.pending_upload,
            s.reason,
            remote_err,
            s.fingerprint
        ));
    }
    out
}

pub fn status_for_doc(doc: &ConfigDoc) -> Result<Vec<CheckpointStatus>> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled || cfg.points.is_empty() {
        return Ok(Vec::new());
    }
    let ws = workspace_paths_for_doc(doc)?;

    let mut out = Vec::new();
    for p in &cfg.points {
        let rt = effective_point_runtime(&cfg, p);
        out.push(point_status(&ws, &cfg, &rt, doc)?);
    }
    Ok(out)
}

fn local_fingerprints_for_point(ws: &WorkspacePaths, point_id: &str) -> Result<Vec<String>> {
    let root = points_root(ws).join(point_id);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::<String>::new();
    for ent in fs::read_dir(&root)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", root.display())))?
    {
        let ent = ent.map_err(|e| Error::msg(format!("read_dir entry error: {e}")))?;
        let p = ent.path();
        if !p.is_dir() {
            continue;
        }
        if manifest_path_for_object(&p).is_file() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn list_s3_fingerprints(
    cfg_name: &str,
    cfg: &S3BackendConfig,
    point_id: &str,
) -> Result<Vec<String>> {
    let resolved = resolve_s3_config(cfg_name, cfg)?;
    let mut prefix = String::new();
    if let Some(p) = resolved
        .prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        prefix.push_str(p.trim_matches('/'));
        prefix.push('/');
    }
    prefix.push_str(point_id);
    prefix.push('/');

    let mut cmd = Command::new("aws");
    configure_s3_cli(&mut cmd, &resolved);
    cmd.arg("s3api")
        .arg("list-objects-v2")
        .arg("--bucket")
        .arg(&resolved.bucket)
        .arg("--prefix")
        .arg(&prefix)
        .arg("--output")
        .arg("json");
    let out = run_command_output(&mut cmd)?;
    if !out.status.success() {
        let msg = command_summary(&out);
        if is_not_found_text(&msg) {
            return Ok(Vec::new());
        }
        return Err(Error::msg(format!("S3 list failed: {msg}")));
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| Error::msg(format!("S3 list JSON parse failed: {e}")))?;
    let mut set = BTreeSet::<String>::new();
    if let Some(arr) = v.get("Contents").and_then(|x| x.as_array()) {
        for item in arr {
            let Some(key) = item.get("Key").and_then(|x| x.as_str()) else {
                continue;
            };
            if !key.ends_with("/manifest.json") {
                continue;
            }
            let stripped = key.trim_start_matches(&prefix);
            let Some((fp, _)) = stripped.split_once('/') else {
                continue;
            };
            if !fp.trim().is_empty() {
                set.insert(fp.to_string());
            }
        }
    }
    Ok(set.into_iter().collect())
}

fn list_http_fingerprints(
    cfg_name: &str,
    cfg: &HttpBackendConfig,
    point_id: &str,
) -> Result<Vec<String>> {
    let base = resolve_http_base_url(cfg_name, cfg)?;
    let token = resolve_http_token(cfg);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::msg(format!("failed to build HTTP client: {e}")))?;
    let url = format!("{}/{}/?list=1", base.trim_end_matches('/'), point_id);
    let mut req = client.get(url);
    if let Some(t) = token.as_deref() {
        req = req.bearer_auth(t);
    }
    let res = req
        .send()
        .map_err(|e| Error::msg(format!("HTTP list failed: {e}")))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    if !res.status().is_success() {
        return Err(Error::msg(format!(
            "HTTP list failed with status {}",
            res.status()
        )));
    }
    let v: serde_json::Value = res
        .json()
        .map_err(|e| Error::msg(format!("HTTP list JSON parse failed: {e}")))?;
    let mut out = Vec::<String>::new();
    match v {
        serde_json::Value::Array(arr) => {
            for e in arr {
                if let Some(s) = e.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    out.push(s.to_string());
                }
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(arr) = map.get("fingerprints").and_then(|x| x.as_array()) {
                for e in arr {
                    if let Some(s) = e.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        out.push(s.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn list_ssh_fingerprints(
    cfg_name: &str,
    cfg: &SshBackendConfig,
    point_id: &str,
) -> Result<Vec<String>> {
    let ssh = resolve_ssh_config(cfg_name, cfg)?;
    let remote_root = format!("{}/{}", ssh.base_path, point_id);
    let cmdline = format!(
        "if [ -d {root} ]; then find {root} -mindepth 1 -maxdepth 1 -type d -print; fi",
        root = sh_quote(&remote_root)
    );
    let mut cmd = Command::new("ssh");
    configure_ssh_cmd(&mut cmd, &ssh, false);
    cmd.arg(&ssh.host).arg(cmdline);
    let out = run_command_output(&mut cmd)?;
    if !out.status.success() {
        let msg = command_summary(&out);
        if is_not_found_text(&msg) {
            return Ok(Vec::new());
        }
        return Err(Error::msg(format!("SSH list failed: {msg}")));
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let mut set = BTreeSet::<String>::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = Path::new(trimmed).file_name().and_then(|s| s.to_str()) {
            set.insert(name.to_string());
        }
    }
    Ok(set.into_iter().collect())
}

fn list_backend_fingerprints(
    cfg: &CheckpointsConfig,
    backend_ref: &str,
    point_id: &str,
) -> Result<Vec<String>> {
    match resolve_backend(cfg, backend_ref)? {
        BackendResolved::S3(name, b) => list_s3_fingerprints(&name, b, point_id),
        BackendResolved::Http(name, b) => list_http_fingerprints(&name, b, point_id),
        BackendResolved::Ssh(name, b) => list_ssh_fingerprints(&name, b, point_id),
    }
}

pub fn list_for_doc(
    doc: &ConfigDoc,
    include_remote: bool,
    id_filter: Option<&str>,
) -> Result<Vec<CheckpointInventory>> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled || cfg.points.is_empty() {
        return Ok(Vec::new());
    }
    let ws = workspace_paths_for_doc(doc)?;
    let id_filter = id_filter
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let mut out = Vec::<CheckpointInventory>::new();
    for p in &cfg.points {
        let id = safe_id(p.id.trim())?;
        if let Some(filter) = id_filter.as_deref()
            && id != filter
        {
            continue;
        }
        let current_fingerprint = compute_point_fingerprint(doc, p)?;
        let local_fingerprints = local_fingerprints_for_point(&ws, &id)?;
        let local_latest = {
            let idx = load_index(&ws)?;
            idx.points.get(&id).map(|e| e.latest_fingerprint.clone())
        };

        let mut remote_fingerprints = Vec::<String>::new();
        let mut remote_error = None::<String>;
        if include_remote
            && let Some(back) = p
                .backend
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            match list_backend_fingerprints(&cfg, back, &id) {
                Ok(v) => remote_fingerprints = v,
                Err(e) => remote_error = Some(e.to_string()),
            }
        }

        out.push(CheckpointInventory {
            id,
            anchor_task: p.anchor_task.trim().to_string(),
            backend: p.backend.as_deref().map(str::trim).map(ToOwned::to_owned),
            current_fingerprint,
            local_fingerprints,
            local_latest,
            remote_fingerprints,
            remote_error,
        });
    }

    Ok(out)
}

pub fn format_list_report(items: &[CheckpointInventory], include_remote: bool) -> String {
    if items.is_empty() {
        return "checkpoints list: none configured".to_string();
    }
    let mut out = String::new();
    out.push_str("checkpoints list:\n");
    for i in items {
        out.push_str(&format!(
            "- id={} anchor={} backend={} current_fingerprint={} local_count={} local_latest={}\n",
            i.id,
            i.anchor_task,
            i.backend.as_deref().unwrap_or("-"),
            i.current_fingerprint,
            i.local_fingerprints.len(),
            i.local_latest.as_deref().unwrap_or("-")
        ));
        if !i.local_fingerprints.is_empty() {
            out.push_str(&format!(
                "  local_fingerprints={}\n",
                i.local_fingerprints.join(",")
            ));
        }
        if include_remote {
            if !i.remote_fingerprints.is_empty() {
                out.push_str(&format!(
                    "  remote_fingerprints={}\n",
                    i.remote_fingerprints.join(",")
                ));
            } else {
                out.push_str("  remote_fingerprints=-\n");
            }
            if let Some(e) = i.remote_error.as_deref() {
                out.push_str(&format!("  remote_error={e}\n"));
            }
        }
    }
    out
}

pub fn validate_against_plan(doc: &ConfigDoc, plan: &Plan) -> Result<()> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled {
        return Ok(());
    }

    let mut ids = BTreeSet::<String>::new();
    let mut anchors = BTreeSet::<String>::new();
    let known_task_ids = plan.tasks().map(|t| t.id.clone()).collect::<BTreeSet<_>>();

    for p in &cfg.points {
        let id = safe_id(p.id.trim())?;
        if !ids.insert(id.clone()) {
            return Err(Error::msg(format!("duplicate checkpoint id '{}'", id)));
        }

        let anchor = p.anchor_task.trim();
        if anchor.is_empty() {
            return Err(Error::msg(format!(
                "checkpoints.points id '{}' has empty anchor_task",
                id
            )));
        }
        if !known_task_ids.contains(anchor) {
            return Err(Error::msg(format!(
                "checkpoints.points id '{}' references unknown anchor_task '{}'",
                id, anchor
            )));
        }
        if !SUPPORTED_ANCHORS.contains(&anchor) {
            return Err(Error::msg(format!(
                "checkpoints anchor '{}' is not supported yet; currently supported: {}",
                anchor,
                SUPPORTED_ANCHORS.join(", ")
            )));
        }
        if !anchors.insert(anchor.to_string()) {
            return Err(Error::msg(format!(
                "multiple checkpoints share anchor_task '{}'; only one checkpoint per anchor is currently supported",
                anchor
            )));
        }

        if let Some(back) = p
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let _ = resolve_backend(&cfg, back)?;
        }
    }

    Ok(())
}

pub fn anchor_restored(doc: &ConfigDoc, ctx: &ExecCtx, anchor_task: &str) -> Result<bool> {
    let p = marker_path(doc, ctx, anchor_task)?;
    if !p.is_file() {
        return Ok(false);
    }

    let cfg = load_cfg(doc)?;
    if !cfg.enabled {
        return Ok(false);
    }
    let Some(rt) = find_point_for_anchor(&cfg, anchor_task)? else {
        return Ok(false);
    };
    let expected = compute_point_fingerprint(doc, rt.point)?;

    let raw = fs::read_to_string(&p).map_err(|e| {
        Error::msg(format!(
            "failed to read restore marker {}: {e}",
            p.display()
        ))
    })?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        Error::msg(format!(
            "failed to parse restore marker {}: {e}",
            p.display()
        ))
    })?;
    let seen = v
        .get("fingerprint")
        .and_then(|x| x.as_str())
        .unwrap_or_default();
    Ok(seen == expected)
}

fn write_anchor_marker(
    doc: &ConfigDoc,
    ctx: &ExecCtx,
    anchor_task: &str,
    point_id: &str,
    fingerprint: &str,
) -> Result<()> {
    let p = marker_path(doc, ctx, anchor_task)?;
    let body = serde_json::to_string_pretty(&serde_json::json!({
        "anchor_task": anchor_task,
        "point_id": point_id,
        "fingerprint": fingerprint,
        "restored_at": chrono::Utc::now().to_rfc3339(),
    }))
    .map_err(|e| Error::msg(format!("failed to encode restore marker: {e}")))?;
    atomic_write_text(&p, &body).map_err(|e| {
        Error::msg(format!(
            "failed to write restore marker {}: {e}",
            p.display()
        ))
    })
}

pub fn maybe_restore_anchor(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    anchor_task: &str,
    targets: &[CheckpointTarget],
) -> Result<bool> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled {
        return Ok(false);
    }

    let Some(rt) = find_point_for_anchor(&cfg, anchor_task)? else {
        return Ok(false);
    };

    let ws = ctx.workspace_paths_or_init(doc)?;
    let mp = marker_path(doc, ctx, anchor_task)?;
    clear_dir_or_file(&mp)?;

    let status = point_status(&ws, &cfg, &rt, doc)?;
    let mut local_hit = status.exists;
    let point_id = status.id.clone();
    let fingerprint = status.fingerprint.clone();
    let object_dir = object_dir_for(&ws, &point_id, &fingerprint)?;
    let backend_ref = rt
        .point
        .backend
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let mut can_use = status.will_use;

    if !local_hit
        && !matches!(rt.use_policy, CheckpointUsePolicy::Off)
        && let Some(back) = backend_ref.as_deref()
    {
        match download_checkpoint_object(&cfg, back, &point_id, &fingerprint, &object_dir) {
            Ok(true) => {
                ctx.log(&format!(
                    "checkpoint:{} anchor={} downloaded via backend '{}'",
                    point_id, status.anchor_task, back
                ));
                local_hit = true;
                can_use = true;
            }
            Ok(false) => {
                ctx.log(&format!(
                    "checkpoint:{} anchor={} remote miss via backend '{}'",
                    point_id, status.anchor_task, back
                ));
            }
            Err(e) => {
                let msg = e.to_string();
                ctx.log(&format!(
                    "WARN: checkpoint:{} anchor={} remote fetch failed via '{}': {}",
                    point_id, status.anchor_task, back, msg
                ));
                if rt.use_policy == CheckpointUsePolicy::Required {
                    return Err(Error::msg(format!(
                        "required checkpoint '{}' download failed via '{}': {}",
                        point_id, back, msg
                    )));
                }
            }
        }
    }

    if rt.use_policy == CheckpointUsePolicy::Required && !local_hit {
        return Err(Error::msg(format!(
            "required checkpoint '{}' missing for anchor '{}' (reason: {})",
            status.id, status.anchor_task, status.reason
        )));
    }

    if !local_hit || !can_use {
        ctx.log(&format!(
            "checkpoint:{} anchor={} restore miss (reason={})",
            status.id, status.anchor_task, status.reason
        ));
        return Ok(false);
    }

    let manifest_path = manifest_path_for_object(&object_dir);
    let manifest = read_manifest(&manifest_path)?;
    let expected_lineage = compute_lineage(anchor_task, &status.fingerprint);
    if rt.trust_mode == CheckpointTrustMode::Verify
        && (manifest.lineage != expected_lineage
            || manifest.fingerprint != status.fingerprint
            || manifest.anchor_task.trim() != anchor_task
            || manifest.id.trim() != status.id)
    {
        return Err(Error::msg(format!(
            "checkpoint '{}' verification failed for anchor '{}' (lineage/fingerprint/id mismatch)",
            status.id, anchor_task
        )));
    }
    ensure_payload_tree(&object_dir)?;
    let payload = payload_dir_for_object(&object_dir);

    let mut target_map = BTreeMap::<String, PathBuf>::new();
    for t in targets {
        target_map.insert(t.name.clone(), t.path.clone());
    }

    for ent in &manifest.targets {
        let Some(dst) = target_map.get(&ent.name) else {
            return Err(Error::msg(format!(
                "checkpoint '{}' target '{}' not provided by anchor '{}'",
                status.id, ent.name, anchor_task
            )));
        };
        let src = payload.join(&ent.payload_rel);
        if !src.exists() {
            return Err(Error::msg(format!(
                "checkpoint '{}' payload entry missing: {}",
                status.id,
                src.display()
            )));
        }

        clear_dir_or_file(dst)?;
        copy_path(&src, dst)?;
    }

    write_anchor_marker(doc, ctx, anchor_task, &status.id, &status.fingerprint)?;
    ctx.log(&format!(
        "checkpoint:{} anchor={} restored (fingerprint={})",
        status.id, status.anchor_task, status.fingerprint
    ));
    Ok(true)
}

pub fn capture_anchor(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    anchor_task: &str,
    targets: &[CheckpointTarget],
) -> Result<()> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled {
        return Ok(());
    }
    let Some(rt) = find_point_for_anchor(&cfg, anchor_task)? else {
        return Ok(());
    };

    let ws = ctx.workspace_paths_or_init(doc)?;
    let _lock = acquire_store_lock(&ws)?;
    let point_id = safe_id(rt.point.id.trim())?;
    let fingerprint_inputs = selected_fingerprint_inputs(doc, rt.point);
    let fingerprint = compute_fingerprint_for_selected(rt.point, &fingerprint_inputs)?;
    let lineage = compute_lineage(anchor_task, &fingerprint);
    let object_dir = object_dir_for(&ws, &point_id, &fingerprint)?;

    if object_dir.exists() {
        fs::remove_dir_all(&object_dir)
            .map_err(|e| Error::msg(format!("failed to replace {}: {e}", object_dir.display())))?;
    }
    fs::create_dir_all(&object_dir)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", object_dir.display())))?;

    let payload = payload_dir_for_object(&object_dir);
    fs::create_dir_all(&payload)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", payload.display())))?;

    let mut manifest_targets = Vec::<CheckpointManifestTarget>::new();
    for t in targets {
        let name = t.name.trim();
        if name.is_empty() {
            return Err(Error::msg("checkpoint capture target name is empty"));
        }
        if !t.path.exists() {
            return Err(Error::msg(format!(
                "checkpoint capture target '{}' path missing: {}",
                name,
                t.path.display()
            )));
        }
        let rel = name.replace('/', "_");
        let dst = payload.join(&rel);
        copy_path(&t.path, &dst)?;
        manifest_targets.push(CheckpointManifestTarget {
            name: name.to_string(),
            payload_rel: rel,
        });
    }

    let manifest = CheckpointManifest {
        version: 1,
        id: point_id.clone(),
        anchor_task: anchor_task.to_string(),
        fingerprint: fingerprint.clone(),
        lineage,
        created_at: chrono::Utc::now().to_rfc3339(),
        trust_mode: rt.trust_mode,
        fingerprint_inputs,
        targets: manifest_targets,
    };
    write_manifest(&manifest_path_for_object(&object_dir), &manifest)?;
    let _ = ensure_payload_archive(&object_dir)?;

    let mut idx = load_index(&ws)?;
    let manifest_rel = manifest_path_for_object(&object_dir)
        .strip_prefix(store_root(&ws))
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            manifest_path_for_object(&object_dir)
                .to_string_lossy()
                .to_string()
        });
    idx.points.insert(
        point_id.clone(),
        CheckpointIndexEntry {
            id: point_id.clone(),
            anchor_task: anchor_task.to_string(),
            latest_fingerprint: fingerprint.clone(),
            latest_manifest_rel: manifest_rel,
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    save_index(&ws, &idx)?;

    ctx.log(&format!(
        "checkpoint:{} anchor={} captured (fingerprint={})",
        point_id, anchor_task, fingerprint
    ));

    if !matches!(rt.upload_policy, CheckpointUploadPolicy::Off)
        && let Some(backend_ref) = rt
            .point
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        match upload_checkpoint_object(&cfg, backend_ref, &point_id, &fingerprint, &object_dir) {
            Ok(()) => {
                ctx.log(&format!(
                    "checkpoint:{} uploaded via backend '{}'",
                    point_id, backend_ref
                ));

                let qpath = queue_path(&ws, &cfg)?;
                let mut q = load_queue(&qpath)?;
                for e in &mut q.entries {
                    if e.id == point_id
                        && e.fingerprint == fingerprint
                        && e.backend_ref == backend_ref
                    {
                        e.state = UploadState::Uploaded;
                        e.last_error = None;
                        e.updated_at = chrono::Utc::now().to_rfc3339();
                    }
                }
                save_queue(&qpath, &q)?;
            }
            Err(e) => {
                let msg = e.to_string();
                ctx.log(&format!(
                    "WARN: checkpoint:{} upload failed via '{}': {} (queued for retry)",
                    point_id, backend_ref, msg
                ));
                enqueue_upload(
                    &ws,
                    &cfg,
                    rt.point,
                    &fingerprint,
                    backend_ref,
                    &object_dir,
                    &msg,
                )?;
            }
        }
    }

    Ok(())
}

pub fn retry_pending_uploads(doc: &ConfigDoc, max: Option<usize>) -> Result<RetryReport> {
    let cfg = load_cfg(doc)?;
    if !cfg.enabled {
        return Ok(RetryReport {
            attempted: 0,
            uploaded: 0,
            failed: 0,
        });
    }
    let ws = workspace_paths_for_doc(doc)?;
    let _lock = acquire_store_lock(&ws)?;
    let qpath = queue_path(&ws, &cfg)?;
    let mut q = load_queue(&qpath)?;

    let mut attempted = 0usize;
    let mut uploaded = 0usize;
    let mut failed = 0usize;

    for e in &mut q.entries {
        if matches!(e.state, UploadState::Uploaded) {
            continue;
        }
        if let Some(m) = max
            && attempted >= m
        {
            break;
        }

        attempted = attempted.saturating_add(1);

        let object_dir = store_root(&ws).join(&e.object_rel_dir);
        if !object_dir.is_dir() {
            e.state = UploadState::Failed;
            e.last_error = Some(format!(
                "checkpoint object dir missing: {}",
                object_dir.display()
            ));
            e.attempts = e.attempts.saturating_add(1);
            e.updated_at = chrono::Utc::now().to_rfc3339();
            failed = failed.saturating_add(1);
            continue;
        }

        match upload_checkpoint_object(&cfg, &e.backend_ref, &e.id, &e.fingerprint, &object_dir) {
            Ok(()) => {
                e.state = UploadState::Uploaded;
                e.last_error = None;
                e.attempts = e.attempts.saturating_add(1);
                e.updated_at = chrono::Utc::now().to_rfc3339();
                uploaded = uploaded.saturating_add(1);
            }
            Err(err) => {
                e.state = UploadState::Failed;
                e.last_error = Some(err.to_string());
                e.attempts = e.attempts.saturating_add(1);
                e.updated_at = chrono::Utc::now().to_rfc3339();
                failed = failed.saturating_add(1);
            }
        }
    }

    save_queue(&qpath, &q)?;
    Ok(RetryReport {
        attempted,
        uploaded,
        failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn mk_doc(tmp: &Path, extra: &str) -> ConfigDoc {
        let raw = format!(
            r#"
[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
use_policy = "auto"
{}"#,
            tmp.display(),
            extra
        );
        let value: toml::Value = toml::from_str(&raw).expect("parse toml");
        ConfigDoc {
            path: tmp.join("build.toml"),
            value,
        }
    }

    #[test]
    fn fingerprint_changes_on_package_change() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d1 = mk_doc(tmp.path(), "");
        let mut d2 = d1.clone();
        if let Some(tbl) = d2
            .value
            .as_table_mut()
            .and_then(|t| t.get_mut("buildroot"))
            .and_then(|v| v.as_table_mut())
        {
            tbl.insert(
                "packages".into(),
                toml::Value::Table({
                    let mut t = toml::map::Map::new();
                    t.insert("htop".into(), toml::Value::Boolean(true));
                    t
                }),
            );
        }

        let cfg1 = load_cfg(&d1).expect("load cfg 1");
        let cfg2 = load_cfg(&d2).expect("load cfg 2");
        let p1 = cfg1.points.first().expect("point1");
        let p2 = cfg2.points.first().expect("point2");
        let f1 = compute_point_fingerprint(&d1, p1).expect("f1");
        let f2 = compute_point_fingerprint(&d2, p2).expect("f2");
        assert_ne!(f1, f2);
    }

    #[test]
    fn default_fingerprint_includes_starting_point_inputs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let doc = mk_doc(tmp.path(), "");
        let cfg = load_cfg(&doc).expect("cfg");
        let point = cfg.points.first().expect("point");
        let inputs = selected_fingerprint_inputs(&doc, point);
        assert!(inputs.contains_key("buildroot.starting_point"));
    }

    #[test]
    fn status_missing_then_hit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let doc = mk_doc(tmp.path(), "");

        let st = status_for_doc(&doc).expect("status");
        assert_eq!(st.len(), 1);
        assert!(!st[0].exists);

        let ws = workspace_paths_for_doc(&doc).expect("ws");
        let cfg = load_cfg(&doc).expect("cfg");
        let p = cfg.points.first().expect("point");
        let fp = compute_point_fingerprint(&doc, p).expect("fp");
        let obj = object_dir_for(&ws, p.id.trim(), &fp).expect("obj");
        fs::create_dir_all(payload_dir_for_object(&obj)).expect("payload dir");
        write_manifest(
            &manifest_path_for_object(&obj),
            &CheckpointManifest {
                version: 1,
                id: p.id.clone(),
                anchor_task: p.anchor_task.clone(),
                fingerprint: fp.clone(),
                lineage: compute_lineage(p.anchor_task.trim(), &fp),
                created_at: chrono::Utc::now().to_rfc3339(),
                trust_mode: CheckpointTrustMode::Verify,
                fingerprint_inputs: BTreeMap::new(),
                targets: vec![],
            },
        )
        .expect("write manifest");

        let st2 = status_for_doc(&doc).expect("status 2");
        assert_eq!(st2.len(), 1);
        assert!(st2[0].exists);
        assert!(st2[0].will_use);
        assert!(!st2[0].will_rebuild);
    }

    #[test]
    fn status_reason_lists_changed_fingerprint_inputs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let doc1 = mk_doc(tmp.path(), "");
        let mut doc2 = doc1.clone();
        if let Some(tbl) = doc2
            .value
            .as_table_mut()
            .and_then(|t| t.get_mut("buildroot"))
            .and_then(|v| v.as_table_mut())
        {
            tbl.insert(
                "packages".into(),
                toml::Value::Table({
                    let mut t = toml::map::Map::new();
                    t.insert("nano".into(), toml::Value::Boolean(true));
                    t
                }),
            );
        }

        let ws = workspace_paths_for_doc(&doc1).expect("ws");
        let cfg = load_cfg(&doc1).expect("cfg");
        let p = cfg.points.first().expect("point");
        let fp1 = compute_point_fingerprint(&doc1, p).expect("fp1");
        let obj = object_dir_for(&ws, p.id.trim(), &fp1).expect("obj");
        fs::create_dir_all(payload_dir_for_object(&obj)).expect("payload dir");
        let inputs1 = selected_fingerprint_inputs(&doc1, p);
        write_manifest(
            &manifest_path_for_object(&obj),
            &CheckpointManifest {
                version: 1,
                id: p.id.clone(),
                anchor_task: p.anchor_task.clone(),
                fingerprint: fp1.clone(),
                lineage: compute_lineage(p.anchor_task.trim(), &fp1),
                created_at: chrono::Utc::now().to_rfc3339(),
                trust_mode: CheckpointTrustMode::Verify,
                fingerprint_inputs: inputs1,
                targets: vec![],
            },
        )
        .expect("manifest");

        let manifest_rel = manifest_path_for_object(&obj)
            .strip_prefix(store_root(&ws))
            .expect("rel")
            .to_string_lossy()
            .to_string();
        let mut idx = CheckpointIndexDoc::default();
        idx.points.insert(
            p.id.trim().to_string(),
            CheckpointIndexEntry {
                id: p.id.trim().to_string(),
                anchor_task: p.anchor_task.clone(),
                latest_fingerprint: fp1,
                latest_manifest_rel: manifest_rel,
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        save_index(&ws, &idx).expect("save index");

        let st = status_for_doc(&doc2).expect("status");
        assert_eq!(st.len(), 1);
        assert!(st[0].reason.contains("inputs_changed"));
        assert!(st[0].reason.contains("buildroot.packages"));
    }

    fn spawn_http_file_server(
        root: PathBuf,
        request_limit: usize,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            for _ in 0..request_limit {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).expect("read request");
                let req = String::from_utf8_lossy(&buf[..n]);
                let mut parts = req
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .collect::<Vec<_>>();
                if parts.len() < 2 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let method = parts.remove(0);
                let path = parts.remove(0);
                let rel = path.trim_start_matches('/');
                let fpath = root.join(rel);
                if fpath.is_file() {
                    let body = fs::read(&fpath).expect("read fixture");
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(hdr.as_bytes()).expect("write hdr");
                    if method != "HEAD" {
                        stream.write_all(&body).expect("write body");
                    }
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });
        (format!("http://{}", addr), handle)
    }

    fn build_http_fixture(
        tmp: &Path,
        point_id: &str,
        anchor_task: &str,
        fingerprint: &str,
    ) -> (PathBuf, CheckpointManifest) {
        let object_dir = tmp.join("fixture-object");
        let payload = payload_dir_for_object(&object_dir);
        fs::create_dir_all(payload.join("buildroot_out_dir")).expect("payload dir");
        fs::write(
            payload.join("buildroot_out_dir").join("marker.txt"),
            "fixture",
        )
        .expect("payload file");

        let manifest = CheckpointManifest {
            version: 1,
            id: point_id.to_string(),
            anchor_task: anchor_task.to_string(),
            fingerprint: fingerprint.to_string(),
            lineage: compute_lineage(anchor_task, fingerprint),
            created_at: chrono::Utc::now().to_rfc3339(),
            trust_mode: CheckpointTrustMode::Verify,
            fingerprint_inputs: BTreeMap::new(),
            targets: vec![CheckpointManifestTarget {
                name: "buildroot_out_dir".to_string(),
                payload_rel: "buildroot_out_dir".to_string(),
            }],
        };
        write_manifest(&manifest_path_for_object(&object_dir), &manifest).expect("manifest");
        let _ = ensure_payload_archive(&object_dir).expect("archive");
        (object_dir, manifest)
    }

    #[test]
    fn probe_http_exists_hit_and_miss() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let point_id = "base";
        let fingerprint = "abc123";
        let (fixture_obj, _manifest) =
            build_http_fixture(tmp.path(), point_id, "buildroot.build", fingerprint);

        let root = tmp.path().join("server");
        let remote_dir = root.join(point_id).join(fingerprint);
        fs::create_dir_all(&remote_dir).expect("remote dir");
        fs::copy(
            manifest_path_for_object(&fixture_obj),
            remote_dir.join("manifest.json"),
        )
        .expect("copy manifest");
        fs::copy(
            payload_archive_for_object(&fixture_obj),
            remote_dir.join("payload.tar"),
        )
        .expect("copy archive");

        let (base_url, handle) = spawn_http_file_server(root, 2);
        let cfg = HttpBackendConfig {
            base_url,
            base_url_env: None,
            token: None,
            token_env: None,
        };
        assert!(probe_http_exists("test", &cfg, point_id, fingerprint).expect("probe hit"));
        assert!(!probe_http_exists("test", &cfg, point_id, "missing").expect("probe miss"));
        handle.join().expect("join");
    }

    #[test]
    fn download_http_object_extracts_payload() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let point_id = "base";
        let fingerprint = "abc999";
        let (fixture_obj, _manifest) =
            build_http_fixture(tmp.path(), point_id, "buildroot.build", fingerprint);

        let root = tmp.path().join("server");
        let remote_dir = root.join(point_id).join(fingerprint);
        fs::create_dir_all(&remote_dir).expect("remote dir");
        fs::copy(
            manifest_path_for_object(&fixture_obj),
            remote_dir.join("manifest.json"),
        )
        .expect("copy manifest");
        fs::copy(
            payload_archive_for_object(&fixture_obj),
            remote_dir.join("payload.tar"),
        )
        .expect("copy archive");

        let (base_url, handle) = spawn_http_file_server(root, 2);
        let cfg = HttpBackendConfig {
            base_url,
            base_url_env: None,
            token: None,
            token_env: None,
        };
        let out_object = tmp.path().join("downloaded");
        let ok = download_with_http("test", &cfg, point_id, fingerprint, &out_object)
            .expect("download should succeed");
        assert!(ok);
        assert!(
            out_object
                .join("payload")
                .join("buildroot_out_dir")
                .join("marker.txt")
                .is_file()
        );
        handle.join().expect("join");
    }

    #[test]
    fn status_reports_remote_hit_for_missing_local_object() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let point_id = "base";

        let root = tmp.path().join("server");
        fs::create_dir_all(&root).expect("server root");
        let (base_url, handle) = spawn_http_file_server(root.clone(), 1);
        let raw = format!(
            r#"
[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
fingerprint_from = ["buildroot.version"]
backend = "http:default"

[checkpoints.backends.http.default]
base_url = "{}"
"#,
            tmp.path().display(),
            base_url
        );
        let value: toml::Value = toml::from_str(&raw).expect("parse toml");
        let doc = ConfigDoc {
            path: tmp.path().join("build.toml"),
            value,
        };

        let cfg = load_cfg(&doc).expect("cfg");
        let point = cfg.points.first().expect("point");
        let fingerprint = compute_point_fingerprint(&doc, point).expect("fingerprint");
        let (fixture_obj, _manifest) =
            build_http_fixture(tmp.path(), point_id, "buildroot.build", &fingerprint);
        let remote_dir = root.join(point_id).join(&fingerprint);
        fs::create_dir_all(&remote_dir).expect("remote dir");
        fs::copy(
            manifest_path_for_object(&fixture_obj),
            remote_dir.join("manifest.json"),
        )
        .expect("copy manifest");

        let items = status_for_doc(&doc).expect("status");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].remote_exists, Some(true));
        assert!(items[0].will_use);
        assert!(items[0].will_download);
        handle.join().expect("join");
    }

    #[test]
    fn s3_resolve_uses_env_fields() {
        unsafe {
            std::env::set_var("GAIA_TEST_CP_BUCKET_ENV", "bucket-from-env");
            std::env::set_var("GAIA_TEST_CP_KEY_ENV", "key-from-env");
        }
        let cfg = S3BackendConfig {
            bucket: String::new(),
            bucket_env: Some("GAIA_TEST_CP_BUCKET_ENV".into()),
            region: None,
            region_env: None,
            prefix: Some("gaia".into()),
            prefix_env: None,
            endpoint_url: None,
            endpoint_url_env: None,
            profile: None,
            profile_env: None,
            aws_access_key_id_env: Some("GAIA_TEST_CP_KEY_ENV".into()),
            aws_secret_access_key_env: None,
            aws_session_token_env: None,
            aws_shared_credentials_file_env: None,
            aws_config_file_env: None,
            aws_ca_bundle_env: None,
        };
        let resolved = resolve_s3_config("test", &cfg).expect("resolve");
        assert_eq!(resolved.bucket, "bucket-from-env");
        assert_eq!(
            resolved.command_env.get("AWS_ACCESS_KEY_ID"),
            Some(&"key-from-env".to_string())
        );
    }

    #[test]
    fn ssh_resolve_uses_env_fields() {
        unsafe {
            std::env::set_var("GAIA_TEST_CP_SSH_TARGET", "u@h:/srv/checkpoints");
            std::env::set_var("GAIA_TEST_CP_SSH_PORT", "2222");
            std::env::set_var("GAIA_TEST_CP_SSH_KEY", "/tmp/key");
        }
        let cfg = SshBackendConfig {
            target: String::new(),
            target_env: Some("GAIA_TEST_CP_SSH_TARGET".into()),
            port: None,
            port_env: Some("GAIA_TEST_CP_SSH_PORT".into()),
            identity_file: None,
            identity_file_env: Some("GAIA_TEST_CP_SSH_KEY".into()),
            known_hosts_file: None,
            known_hosts_file_env: None,
            strict_host_key_checking: Some(false),
        };
        let resolved = resolve_ssh_config("test", &cfg).expect("resolve");
        assert_eq!(resolved.host, "u@h");
        assert_eq!(resolved.base_path, "/srv/checkpoints");
        assert_eq!(resolved.port, Some(2222));
        assert_eq!(resolved.identity_file.as_deref(), Some("/tmp/key"));
        assert_eq!(resolved.strict_host_key_checking, Some(false));
    }
}
