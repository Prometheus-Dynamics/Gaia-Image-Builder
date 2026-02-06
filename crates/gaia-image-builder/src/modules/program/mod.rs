use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::ExecCtx;
use crate::modules::util;
use crate::workspace::WorkspacePaths;

pub mod custom;
pub mod install;
pub mod java;
pub mod lint;
pub mod rust;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckPolicy {
    Required,
    Warn,
}

impl Default for CheckPolicy {
    fn default() -> Self {
        Self::Required
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProgramProfile {
    pub target: Option<String>,
    pub linker: Option<String>,
    pub container_image: Option<String>,
    pub sysroot: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProgramCheck {
    pub id: String,
    pub run: Vec<String>,
    pub applies_to: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl Default for ProgramCheck {
    fn default() -> Self {
        Self {
            id: String::new(),
            run: Vec::new(),
            applies_to: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProgramConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub default_profile: Option<String>,
    pub check_policy: CheckPolicy,
    pub checks: Vec<ProgramCheck>,
    pub profiles: BTreeMap<String, ProgramProfile>,
}

impl Default for ProgramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_profile: None,
            check_policy: CheckPolicy::Required,
            checks: Vec::new(),
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactMode {
    Build,
    Prebuilt,
    Auto,
}

impl Default for ArtifactMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct GenericArtifactRef {
    pub id: String,
    pub profile: Option<String>,
    pub check_ids: Vec<String>,
}

impl Default for GenericArtifactRef {
    fn default() -> Self {
        Self {
            id: String::new(),
            profile: None,
            check_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: String,
    pub producer: String,
    pub kind: String,
    pub abs_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactBuildState {
    pub id: String,
    pub producer: String,
    pub fingerprint: String,
    pub output_abs_path: String,
    pub updated_at: String,
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn load_program_cfg(doc: &ConfigDoc) -> Result<ProgramConfig> {
    Ok(doc.deserialize_path("program")?.unwrap_or_default())
}

pub fn check_ids(cfg: &ProgramConfig) -> BTreeSet<String> {
    cfg.checks.iter().map(|c| c.id.clone()).collect()
}

fn load_artifact_refs_for(
    doc: &ConfigDoc,
    path: &str,
    builder: &str,
) -> Result<Vec<GenericArtifactRef>> {
    let refs: Vec<GenericArtifactRef> = doc.deserialize_path(path)?.unwrap_or_default();
    for r in &refs {
        if r.id.trim().is_empty() {
            return Err(Error::msg(format!("{}.artifacts[].id is empty", builder)));
        }
    }
    Ok(refs)
}

pub fn validate_program_definitions(doc: &ConfigDoc) -> Result<()> {
    let cfg = load_program_cfg(doc)?;
    if !cfg.enabled {
        return Ok(());
    }

    let mut check_ids = BTreeSet::new();
    for c in &cfg.checks {
        let id = c.id.trim();
        if id.is_empty() {
            return Err(Error::msg("program.checks[].id is empty"));
        }
        if !check_ids.insert(id.to_string()) {
            return Err(Error::msg(format!("duplicate program check id '{}'", id)));
        }
        if c.run.is_empty() {
            return Err(Error::msg(format!(
                "program.checks id '{}' has empty run command",
                id
            )));
        }
    }

    let mut seen = BTreeMap::<String, String>::new();
    let builder_refs = [
        ("program.rust.artifacts", "program.rust"),
        ("program.java.artifacts", "program.java"),
        ("program.custom.artifacts", "program.custom"),
    ];

    for (path, builder) in builder_refs {
        for a in load_artifact_refs_for(doc, path, builder)? {
            if let Some(existing) = seen.insert(a.id.clone(), builder.to_string()) {
                return Err(Error::msg(format!(
                    "artifact id '{}' is defined in both '{}' and '{}'",
                    a.id, existing, builder
                )));
            }

            let profile_name = a
                .profile
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| cfg.default_profile.clone());

            if let Some(name) = profile_name
                && !cfg.profiles.contains_key(&name)
            {
                return Err(Error::msg(format!(
                    "artifact '{}' references unknown program profile '{}'",
                    a.id, name
                )));
            }

            for id in &a.check_ids {
                if !check_ids.contains(id) {
                    return Err(Error::msg(format!(
                        "artifact '{}' references unknown check id '{}'",
                        a.id, id
                    )));
                }
            }
        }
    }

    Ok(())
}

pub fn resolve_profile<'a>(
    cfg: &'a ProgramConfig,
    name: Option<&str>,
) -> Result<Option<&'a ProgramProfile>> {
    let chosen = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| cfg.default_profile.clone());

    if let Some(name) = chosen {
        let p = cfg
            .profiles
            .get(&name)
            .ok_or_else(|| Error::msg(format!("unknown program profile '{}'", name)))?;
        return Ok(Some(p));
    }

    Ok(None)
}

pub fn effective_check_ids(default_ids: &[String], artifact_ids: &[String]) -> Vec<String> {
    if artifact_ids.is_empty() {
        return default_ids.to_vec();
    }
    artifact_ids.to_vec()
}

pub fn run_checks(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    applies_as: &str,
    cwd_fallback: &Path,
    selected_check_ids: &[String],
) -> Result<()> {
    let cfg = load_program_cfg(doc)?;
    if !cfg.enabled || selected_check_ids.is_empty() {
        return Ok(());
    }

    let mut checks = BTreeMap::<String, ProgramCheck>::new();
    for c in cfg.checks {
        checks.insert(c.id.clone(), c);
    }

    for cid in selected_check_ids {
        let Some(check) = checks.get(cid) else {
            return Err(Error::msg(format!("unknown check id '{}'", cid)));
        };

        if !check.applies_to.is_empty() && !check.applies_to.iter().any(|x| x == applies_as) {
            continue;
        }

        if check.run.is_empty() {
            return Err(Error::msg(format!("check '{}' has empty run command", cid)));
        }

        let mut cmd = Command::new(&check.run[0]);
        if check.run.len() > 1 {
            cmd.args(&check.run[1..]);
        }

        let cwd = check
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd_fallback.to_path_buf());
        cmd.current_dir(cwd);

        for (k, v) in &check.env {
            cmd.env(k, v);
        }

        ctx.log(&format!("check:{} => {:?}", cid, check.run));
        match ctx.run_cmd(cmd) {
            Ok(()) => {}
            Err(e) => match cfg.check_policy {
                CheckPolicy::Required => {
                    return Err(Error::msg(format!("check '{}' failed: {}", cid, e)));
                }
                CheckPolicy::Warn => {
                    ctx.log(&format!("WARN: check '{}' failed: {}", cid, e));
                }
            },
        }
    }

    Ok(())
}

pub fn resolve_from_workspace_root(ws: &WorkspacePaths, raw: &str) -> Result<PathBuf> {
    ws.resolve_config_path(raw)
}

pub fn artifact_record_path(doc: &ConfigDoc, ctx: &ExecCtx, artifact_id: &str) -> Result<PathBuf> {
    let id = artifact_id.trim();
    if id.is_empty() {
        return Err(Error::msg("artifact id is empty"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(Error::msg(format!(
            "artifact id '{}' contains invalid characters",
            artifact_id
        )));
    }
    Ok(util::artifact_registry_dir(doc, ctx)?.join(format!("{}.json", id)))
}

pub fn write_artifact_record(doc: &ConfigDoc, ctx: &ExecCtx, rec: &ArtifactRecord) -> Result<()> {
    let p = artifact_record_path(doc, ctx, &rec.id)?;
    let abs = Path::new(&rec.abs_path);
    if !abs.exists() {
        return Err(Error::msg(format!(
            "artifact '{}' path does not exist: {}",
            rec.id,
            abs.display()
        )));
    }
    util::ensure_dir(p.parent().expect("artifact record parent"))?;
    util::write_json_pretty(
        &p,
        &serde_json::json!({
            "id": rec.id,
            "producer": rec.producer,
            "kind": rec.kind,
            "abs_path": rec.abs_path,
        }),
    )
}

pub fn read_artifact_record(
    doc: &ConfigDoc,
    ctx: &ExecCtx,
    artifact_id: &str,
) -> Result<ArtifactRecord> {
    let p = artifact_record_path(doc, ctx, artifact_id)?;
    let data = fs::read_to_string(&p).map_err(|e| {
        Error::msg(format!(
            "failed to read artifact record {}: {e}",
            p.display()
        ))
    })?;
    serde_json::from_str::<ArtifactRecord>(&data).map_err(|e| {
        Error::msg(format!(
            "failed to decode artifact record {}: {e}",
            p.display()
        ))
    })
}

fn artifact_state_path(doc: &ConfigDoc, ctx: &ExecCtx, artifact_id: &str) -> Result<PathBuf> {
    let id = artifact_id.trim();
    if id.is_empty() {
        return Err(Error::msg("artifact id is empty"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(Error::msg(format!(
            "artifact id '{}' contains invalid characters",
            artifact_id
        )));
    }
    Ok(util::artifact_registry_dir(doc, ctx)?.join(format!("{id}.state.json")))
}

pub fn write_artifact_state(
    doc: &ConfigDoc,
    ctx: &ExecCtx,
    state: &ArtifactBuildState,
) -> Result<()> {
    let p = artifact_state_path(doc, ctx, &state.id)?;
    util::ensure_dir(p.parent().expect("artifact state parent"))?;
    util::write_json_pretty(
        &p,
        &serde_json::json!({
            "id": state.id,
            "producer": state.producer,
            "fingerprint": state.fingerprint,
            "output_abs_path": state.output_abs_path,
            "updated_at": state.updated_at,
        }),
    )
}

pub fn read_artifact_state(
    doc: &ConfigDoc,
    ctx: &ExecCtx,
    artifact_id: &str,
) -> Result<Option<ArtifactBuildState>> {
    let p = artifact_state_path(doc, ctx, artifact_id)?;
    if !p.is_file() {
        return Ok(None);
    }
    let data = fs::read_to_string(&p).map_err(|e| {
        Error::msg(format!(
            "failed to read artifact state {}: {e}",
            p.display()
        ))
    })?;
    let parsed = serde_json::from_str::<ArtifactBuildState>(&data).map_err(|e| {
        Error::msg(format!(
            "failed to decode artifact state {}: {e}",
            p.display()
        ))
    })?;
    Ok(Some(parsed))
}

pub fn compute_path_tree_stamp(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut entries = Vec::<(String, String)>::new();
    if !root.exists() {
        return Ok("missing".into());
    }

    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap_or(p);
            rel.as_os_str().is_empty() || !should_skip_tree_component(rel)
        });

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                if e.io_error()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
                {
                    continue;
                }
                return Err(Error::msg(format!("walkdir error: {e}")));
            }
        };
        let p = entry.path();
        let rel = p.strip_prefix(root).unwrap_or(p);
        if rel.as_os_str().is_empty() {
            continue;
        }
        if should_skip_tree_component(rel) {
            continue;
        }

        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let meta = fs::symlink_metadata(p)
            .map_err(|e| Error::msg(format!("failed to stat {}: {e}", p.display())))?;
        let typ = if meta.file_type().is_symlink() {
            "l"
        } else if meta.is_dir() {
            "d"
        } else {
            "f"
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| format!("{}.{:09}", d.as_secs(), d.subsec_nanos()))
            .unwrap_or_else(|| "0.000000000".into());
        let target = if meta.file_type().is_symlink() {
            fs::read_link(p)
                .ok()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        entries.push((rel_s, format!("{typ}|{size}|{modified}|{target}")));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (p, sig) in entries {
        hasher.update(p.as_bytes());
        hasher.update(b"\n");
        hasher.update(sig.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

fn should_skip_tree_component(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        matches!(
            s.as_ref(),
            ".git"
                | "target"
                | ".gaia-target"
                | "build"
                | "out"
                | "output"
                | ".cache"
                | "node_modules"
        )
    })
}

pub fn compute_artifact_fingerprint(
    payload: &serde_json::Value,
    input_root: &Path,
) -> Result<String> {
    use sha2::{Digest, Sha256};

    let payload_str = serde_json::to_string(payload).map_err(|e| {
        Error::msg(format!(
            "failed to encode artifact fingerprint payload: {e}"
        ))
    })?;
    let tree = compute_path_tree_stamp(input_root)?;
    let mut hasher = Sha256::new();
    hasher.update(payload_str.as_bytes());
    hasher.update(b"\n");
    hasher.update(tree.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

pub fn path_kind(path: &Path) -> Result<&'static str> {
    if path.is_file() {
        return Ok("file");
    }
    if path.is_dir() {
        return Ok("dir");
    }
    Err(Error::msg(format!(
        "artifact path '{}' is neither file nor dir",
        path.display()
    )))
}
