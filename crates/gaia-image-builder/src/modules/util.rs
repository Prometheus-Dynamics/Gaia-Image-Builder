use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::ExecCtx;

pub fn build_name(doc: &ConfigDoc) -> String {
    doc.path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("build")
        .to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct BuildMetaConfig {
    version: Option<String>,
}

pub fn build_version(doc: &ConfigDoc) -> Option<String> {
    let cfg: BuildMetaConfig = doc
        .deserialize_path("build")
        .ok()
        .flatten()
        .unwrap_or_default();
    cfg.version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub fn expand_build_template(doc: &ConfigDoc, raw: &str) -> Result<String> {
    let mut out = raw.to_string();
    out = out.replace("{build}", &build_name(doc));
    if out.contains("{version}") {
        let Some(version) = build_version(doc) else {
            return Err(Error::msg(
                "path template uses '{version}' but [build].version is not set",
            ));
        };
        out = out.replace("{version}", &version);
    }
    Ok(out)
}

pub fn gaia_run_dir(doc: &ConfigDoc, ctx: &ExecCtx) -> Result<PathBuf> {
    let ws = ctx.workspace_paths_or_init(doc)?;
    Ok(ws.out_dir.join(build_name(doc)).join("gaia"))
}

pub fn stage_root_dir(doc: &ConfigDoc, ctx: &ExecCtx) -> Result<PathBuf> {
    let ws = ctx.workspace_paths_or_init(doc)?;
    Ok(ws
        .build_dir
        .join("stage")
        .join(build_name(doc))
        .join("rootfs"))
}

pub fn artifact_registry_dir(doc: &ConfigDoc, ctx: &ExecCtx) -> Result<PathBuf> {
    let ws = ctx.workspace_paths_or_init(doc)?;
    Ok(ws.build_dir.join("artifacts"))
}

pub fn module_dir(doc: &ConfigDoc, ctx: &ExecCtx, module_id: &str) -> Result<PathBuf> {
    // Nested folders read nicer than dotted ids.
    let mut out = gaia_run_dir(doc, ctx)?.join("modules");
    for seg in module_id.split('.').filter(|s| !s.is_empty()) {
        out = out.join(seg);
    }
    Ok(out)
}

pub fn validate_rel_like_path(p: &str) -> Result<()> {
    let path = p.trim();
    if path.is_empty() {
        return Err(Error::msg("path is empty"));
    }
    let pb = Path::new(path);
    if pb.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::msg(format!("path '{}' contains '..'", path)));
    }
    Ok(())
}

pub fn stage_path(doc: &ConfigDoc, ctx: &ExecCtx, image_abs_path: &str) -> Result<PathBuf> {
    let abs = image_abs_path.trim();
    if !abs.starts_with('/') {
        return Err(Error::msg(format!(
            "expected absolute image path, got '{}'",
            image_abs_path
        )));
    }
    let rel = abs.trim_start_matches('/');
    validate_rel_like_path(rel)?;
    Ok(stage_root_dir(doc, ctx)?.join(rel))
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    fs::create_dir_all(p)
        .map_err(|e| Error::msg(format!("failed to create dir {}: {e}", p.display())))
}

pub fn write_text(p: &Path, s: &str) -> Result<()> {
    if let Some(parent) = p.parent() {
        ensure_dir(parent)?;
    }
    fs::write(p, s).map_err(|e| Error::msg(format!("failed to write {}: {e}", p.display())))
}

pub fn write_json_pretty(p: &Path, v: &serde_json::Value) -> Result<()> {
    let s = serde_json::to_string_pretty(v)
        .map_err(|e| Error::msg(format!("json encode error: {e}")))?;
    write_text(p, &s)
}
