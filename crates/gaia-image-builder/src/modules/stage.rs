use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use gaia_image_builder_macros::{Module, Task};

use crate::config::ConfigDoc;
use crate::executor::ExecCtx;
use crate::modules::util;
use crate::workspace::WorkspacePaths;
use crate::{Error, Result};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub files: Vec<StageFile>,
    pub env: StageEnvConfig,
    pub services: StageServicesConfig,
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            files: Vec::new(),
            env: StageEnvConfig::default(),
            services: StageServicesConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageFile {
    pub src: Option<String>,
    pub dst: String,
    pub mode: Option<u32>,
    pub content: Option<String>,
}

impl Default for StageFile {
    fn default() -> Self {
        Self {
            src: None,
            dst: String::new(),
            mode: None,
            content: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageEnvConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub sets: BTreeMap<String, BTreeMap<String, String>>,
    pub files: Vec<StageEnvFile>,
}

impl Default for StageEnvConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sets: BTreeMap::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageEnvFile {
    pub path: String,
    pub vars: BTreeMap<String, String>,
    pub mode: Option<u32>,
}

impl Default for StageEnvFile {
    fn default() -> Self {
        Self {
            path: String::new(),
            vars: BTreeMap::new(),
            mode: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageServicesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub allowed_prefixes: Vec<String>,
    pub units: BTreeMap<String, StageUnitConfig>,
}

impl Default for StageServicesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_prefixes: Vec::new(),
            units: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StageUnitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub vendor: bool,
    pub src: Option<String>,
    pub unit: Option<String>,
    pub targets: Vec<String>,
    pub env_set: Option<String>,
    pub env_file: Option<String>,
    pub env: BTreeMap<String, String>,
    pub assets: Vec<StageAssetConfig>,
}

impl Default for StageUnitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            vendor: false,
            src: None,
            unit: None,
            targets: Vec::new(),
            env_set: None,
            env_file: None,
            env: BTreeMap::new(),
            assets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StageAssetConfig {
    pub src: String,
    pub dst: String,
    pub mode: Option<u32>,
}

#[Task(
    id = "stage.render",
    module = "stage",
    phase = "render",
    provides = ["stage:content", "stage:services"],
    after = ["core.init", "stage:program-install?"],
    default_label = "Render stage content",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RenderTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for RenderTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl RenderTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let cfg: StageConfig = doc.deserialize_path("stage")?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }

        let ws = ctx.workspace_paths_or_init(doc)?;
        let dir = util::module_dir(doc, ctx, "stage")?;
        util::ensure_dir(&dir)?;
        let stage_root = util::stage_root_dir(doc, ctx)?;
        util::ensure_dir(&stage_root)?;
        cleanup_previous_service_outputs(doc, ctx, &dir.join("manifest.json"))?;

        let mut files_manifest = Vec::new();
        let mut dir_copy_dsts = BTreeSet::<PathBuf>::new();
        for f in &cfg.files {
            let Some(src) = f.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let src_abs = resolve_source_path(&ws, src)?;
            let src_meta = fs::symlink_metadata(&src_abs)
                .map_err(|e| Error::msg(format!("failed to stat {}: {e}", src_abs.display())))?;
            if !src_meta.is_dir() {
                continue;
            }
            let dst_abs = util::stage_path(doc, ctx, f.dst.trim())?;
            dir_copy_dsts.insert(dst_abs);
        }
        for dst in dir_copy_dsts {
            remove_path_if_exists(&dst)?;
        }

        for f in &cfg.files {
            let dst_abs = util::stage_path(doc, ctx, f.dst.trim())?;
            if let Some(content) = f.content.as_ref() {
                util::write_text(&dst_abs, content)?;
            } else if let Some(src) = f.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let src_abs = resolve_source_path(&ws, src)?;
                copy_path(&src_abs, &dst_abs)?;
            } else {
                return Err(Error::msg(format!(
                    "stage.files dst='{}' requires either src or content",
                    f.dst
                )));
            }
            if let Some(mode) = f.mode {
                set_mode(&dst_abs, mode)?;
            }
            files_manifest.push(serde_json::json!({
                "src": f.src,
                "dst": f.dst,
                "mode": f.mode,
                "content_inline": f.content.is_some(),
            }));
        }

        let mut env_file_manifest = Vec::new();
        if cfg.env.enabled {
            for ef in &cfg.env.files {
                let path = ef.path.trim();
                if path.is_empty() {
                    return Err(Error::msg("stage.env.files[].path is empty"));
                }
                let dst = util::stage_path(doc, ctx, path)?;
                write_env_file(&dst, &ef.vars)?;
                if let Some(mode) = ef.mode {
                    set_mode(&dst, mode)?;
                }
                env_file_manifest.push(serde_json::json!({
                    "path": ef.path,
                    "vars": ef.vars,
                    "mode": ef.mode,
                }));
            }
        }

        let mut unit_manifest = Vec::new();
        let mut unit_asset_manifest = Vec::new();
        let mut unit_env_manifest = Vec::new();
        let mut enable_links = Vec::new();
        if cfg.services.enabled {
            for (name, u) in &cfg.services.units {
                if !u.enabled {
                    continue;
                }
                validate_unit_name(name)?;
                let unit_name = infer_unit_name(name, u)?;
                let unit_dst =
                    util::stage_path(doc, ctx, &format!("/etc/systemd/system/{unit_name}"))?;

                let unit_src = match u.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(src) => {
                        let resolved = resolve_source_path(&ws, src)?;
                        if !resolved.is_file() {
                            return Err(Error::msg(format!(
                                "stage.services.units '{}' src is not a file: {}",
                                name,
                                resolved.display()
                            )));
                        }
                        copy_file(&resolved, &unit_dst)?;
                        Some(resolved)
                    }
                    None => {
                        if !u.vendor {
                            return Err(Error::msg(format!(
                                "stage.services.units '{}' requires src when vendor=false",
                                name
                            )));
                        }
                        None
                    }
                };

                if let Some(env_file) = build_env_file(name, u, &cfg.env)? {
                    let dst = util::stage_path(doc, ctx, &env_file.path)?;
                    write_env_file(&dst, &env_file.vars)?;
                    unit_env_manifest.push(serde_json::json!({
                        "unit": name,
                        "env_file": env_file.path,
                        "vars": env_file.vars,
                    }));
                }

                for asset in &u.assets {
                    let src =
                        resolve_asset_source_path(&ws, unit_src.as_deref(), asset.src.trim())?;
                    let dst = util::stage_path(doc, ctx, asset.dst.trim())?;
                    copy_path(&src, &dst)?;
                    if let Some(mode) = asset.mode {
                        set_mode(&dst, mode)?;
                    }
                    unit_asset_manifest.push(serde_json::json!({
                        "unit": name,
                        "src": src.display().to_string(),
                        "dst": asset.dst,
                        "mode": asset.mode,
                    }));
                }

                let link_target = if unit_src.is_some() {
                    format!("../{unit_name}")
                } else {
                    format!("/usr/lib/systemd/system/{unit_name}")
                };

                for target in &u.targets {
                    let target_unit = normalize_target_unit(target)?;
                    let wants_dir = util::stage_path(
                        doc,
                        ctx,
                        &format!("/etc/systemd/system/{target_unit}.wants"),
                    )?;
                    util::ensure_dir(&wants_dir)?;
                    let link_path = wants_dir.join(&unit_name);
                    ensure_symlink(&link_target, &link_path)?;
                    enable_links.push(serde_json::json!({
                        "unit": name,
                        "target": target_unit,
                        "to": link_target,
                    }));
                }

                unit_manifest.push(serde_json::json!({
                    "name": name,
                    "unit": unit_name,
                    "vendor": u.vendor,
                    "src": unit_src.as_ref().map(|p| p.display().to_string()),
                    "targets": u.targets,
                    "env_set": u.env_set,
                }));
            }
        }

        let manifest = serde_json::json!({
            "stage_root": stage_root.display().to_string(),
            "files": files_manifest,
            "env_files": env_file_manifest,
            "services": {
                "units": unit_manifest,
                "assets": unit_asset_manifest,
                "env_files": unit_env_manifest,
                "enable_links": enable_links,
            }
        });
        util::write_json_pretty(&dir.join("manifest.json"), &manifest)?;
        ctx.log(&format!("wrote {}", dir.join("manifest.json").display()));
        Ok(())
    }
}

#[Module(id = "stage", config = StageConfig, config_path = "stage", tasks = [RenderTask])]
pub struct StageModule;

#[derive(Debug, Clone)]
struct EnvFileOut {
    path: String,
    vars: BTreeMap<String, String>,
}

fn build_env_file(
    unit_name: &str,
    unit: &StageUnitConfig,
    env_cfg: &StageEnvConfig,
) -> Result<Option<EnvFileOut>> {
    let env_file = default_env_file_path(unit_name, unit);
    let env_file = unit
        .env_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(env_file.as_str())
        .to_string();
    let vars = merge_unit_env(unit_name, unit, env_cfg)?;
    if vars.is_empty() {
        return Ok(None);
    }
    Ok(Some(EnvFileOut {
        path: env_file,
        vars,
    }))
}

fn default_env_file_path(name: &str, unit: &StageUnitConfig) -> String {
    let mut base = unit
        .unit
        .as_deref()
        .unwrap_or(name)
        .trim()
        .trim_end_matches(".service")
        .trim_end_matches(".socket")
        .trim_end_matches(".timer")
        .trim_end_matches(".target")
        .trim()
        .to_string();
    if base.is_empty() {
        base = "service".into();
    }
    base = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("/etc/default/{base}")
}

fn merge_unit_env(
    unit_name: &str,
    unit: &StageUnitConfig,
    env_cfg: &StageEnvConfig,
) -> Result<BTreeMap<String, String>> {
    let mut merged = BTreeMap::<String, String>::new();

    if let Some(set_name) = unit
        .env_set
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !env_cfg.enabled {
            return Err(Error::msg(format!(
                "stage.services unit '{}' requests env_set='{}' but stage.env.enabled=false",
                unit_name, set_name
            )));
        }
        let set = env_cfg.sets.get(set_name).ok_or_else(|| {
            Error::msg(format!(
                "stage.services unit '{}' references unknown env set '{}'",
                unit_name, set_name
            ))
        })?;
        for (k, v) in set {
            merged.insert(k.clone(), v.clone());
        }
    }

    for (k, v) in &unit.env {
        merged.insert(k.clone(), v.clone());
    }

    Ok(merged)
}

fn validate_unit_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::msg("stage.services unit key is empty"));
    }
    if name.contains('/') {
        return Err(Error::msg(format!(
            "stage.services unit key '{}' must not contain '/'",
            name
        )));
    }
    Ok(())
}

fn infer_unit_name(name: &str, unit: &StageUnitConfig) -> Result<String> {
    if let Some(explicit) = unit
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return validate_unit_file_name(explicit);
    }

    if let Some(src) = unit.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let base = Path::new(src)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::msg(format!("invalid unit src path '{}'", src)))?;
        return validate_unit_file_name(base);
    }

    if name.contains('.') {
        return validate_unit_file_name(name);
    }
    validate_unit_file_name(&format!("{name}.service"))
}

fn validate_unit_file_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::msg("unit name is empty"));
    }
    if name.contains('/') {
        return Err(Error::msg(format!(
            "unit name '{}' must not contain '/'",
            name
        )));
    }
    Ok(name.to_string())
}

fn normalize_target_unit(target: &str) -> Result<String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(Error::msg(
            "stage.services.units[].targets contains an empty value",
        ));
    }
    if target.contains('/') {
        return Err(Error::msg(format!(
            "invalid systemd target '{}': must not contain '/'",
            target
        )));
    }
    if target.contains('.') {
        Ok(target.to_string())
    } else {
        Ok(format!("{target}.target"))
    }
}

fn cleanup_previous_service_outputs(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    manifest_path: &Path,
) -> Result<()> {
    if !manifest_path.is_file() {
        return Ok(());
    }

    let raw = fs::read_to_string(manifest_path).map_err(|e| {
        Error::msg(format!(
            "failed to read previous stage manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let manifest: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            ctx.log(&format!(
                "stage manifest parse warning (skipping stale service cleanup): {}: {e}",
                manifest_path.display()
            ));
            return Ok(());
        }
    };

    let mut stale_paths = BTreeSet::<PathBuf>::new();
    collect_previous_service_paths(doc, ctx, &manifest, &mut stale_paths)?;
    for p in stale_paths {
        remove_path_if_exists(&p)?;
    }
    Ok(())
}

fn collect_previous_service_paths(
    doc: &ConfigDoc,
    ctx: &ExecCtx,
    manifest: &serde_json::Value,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let mut push_image_path = |raw: &str| -> Result<()> {
        let abs = raw.trim();
        if abs.is_empty() || !abs.starts_with('/') {
            return Ok(());
        }
        out.insert(util::stage_path(doc, ctx, abs)?);
        Ok(())
    };

    if let Some(units) = manifest
        .pointer("/services/units")
        .and_then(serde_json::Value::as_array)
    {
        for unit in units {
            if let Some(name) = unit.get("unit").and_then(serde_json::Value::as_str) {
                push_image_path(&format!("/etc/systemd/system/{name}"))?;
            }
        }
    }

    if let Some(assets) = manifest
        .pointer("/services/assets")
        .and_then(serde_json::Value::as_array)
    {
        for asset in assets {
            if let Some(dst) = asset.get("dst").and_then(serde_json::Value::as_str) {
                push_image_path(dst)?;
            }
        }
    }

    if let Some(env_files) = manifest
        .pointer("/services/env_files")
        .and_then(serde_json::Value::as_array)
    {
        for env_file in env_files {
            if let Some(path) = env_file.get("env_file").and_then(serde_json::Value::as_str) {
                push_image_path(path)?;
            }
        }
    }

    if let Some(links) = manifest
        .pointer("/services/enable_links")
        .and_then(serde_json::Value::as_array)
    {
        for link in links {
            let Some(target) = link.get("target").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(to) = link.get("to").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(link_name) = Path::new(to).file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            push_image_path(&format!("/etc/systemd/system/{target}.wants/{link_name}"))?;
        }
    }

    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
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
                fs::remove_file(path).map_err(|e| {
                    Error::msg(format!("failed to remove file {}: {e}", path.display()))
                })?;
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

fn resolve_source_path(ws: &WorkspacePaths, raw: &str) -> Result<PathBuf> {
    let path = ws.resolve_config_path(raw)?;
    if !path.exists() {
        return Err(Error::msg(format!(
            "source path not found: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn resolve_asset_source_path(
    ws: &WorkspacePaths,
    unit_src: Option<&Path>,
    raw: &str,
) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::msg("stage.services.units[].assets[].src is empty"));
    }
    validate_no_parent_dirs(raw)?;

    if Path::new(raw).is_absolute() {
        let p = resolve_source_path(ws, raw)?;
        return Ok(p);
    }

    let direct = resolve_source_path(ws, raw);
    if let Ok(p) = direct {
        return Ok(p);
    }

    if let Some(unit_src) = unit_src
        && let Some(parent) = unit_src.parent()
    {
        let candidate = parent.join(raw);
        ensure_under_root_and_exists(&ws.root, &candidate)?;
        return Ok(candidate);
    }

    direct
}

fn validate_no_parent_dirs(raw: &str) -> Result<()> {
    let p = Path::new(raw);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::msg(format!("path '{}' contains '..'", raw)));
    }
    Ok(())
}

fn ensure_under_root_and_exists(root: &Path, p: &Path) -> Result<()> {
    if !p.exists() {
        return Err(Error::msg(format!(
            "source path not found: {}",
            p.display()
        )));
    }
    let root_can = root
        .canonicalize()
        .map_err(|e| Error::msg(format!("failed to canonicalize {}: {e}", root.display())))?;
    let p_can = p
        .canonicalize()
        .map_err(|e| Error::msg(format!("failed to canonicalize {}: {e}", p.display())))?;
    if !p_can.starts_with(&root_can) {
        return Err(Error::msg(format!(
            "refusing source path outside workspace root: {}",
            p.display()
        )));
    }
    Ok(())
}

fn write_env_file(path: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    let mut content = String::new();
    for (k, v) in env {
        content.push_str(k);
        content.push('=');
        content.push_str(&format_env_value(v));
        content.push('\n');
    }
    util::write_text(path, &content)
}

fn format_env_value(v: &str) -> String {
    if !env_value_needs_quote(v) {
        return v.to_string();
    }
    shell_single_quote(v)
}

fn env_value_needs_quote(v: &str) -> bool {
    if v.is_empty() {
        return true;
    }
    !v.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '+' | ',' | '@')
    })
}

fn shell_single_quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('\'');
    for ch in v.chars() {
        if ch == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_env_value_leaves_safe_values_unquoted() {
        assert_eq!(format_env_value("AUTO"), "AUTO");
        assert_eq!(format_env_value("0xF001"), "0xF001");
        assert_eq!(format_env_value("a_b-c.d/e:f+g,h@i"), "a_b-c.d/e:f+g,h@i");
    }

    #[test]
    fn format_env_value_quotes_shell_sensitive_values() {
        assert_eq!(
            format_env_value("Prometheus Dynamics"),
            "'Prometheus Dynamics'"
        );
        assert_eq!(format_env_value(""), "''");
        assert_eq!(format_env_value("O'Reilly Labs"), "'O'\"'\"'Reilly Labs'");
    }
}

fn copy_path(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .map_err(|e| Error::msg(format!("failed to stat {}: {e}", src.display())))?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, dst)
    } else if meta.is_dir() {
        copy_dir_all(src, dst)
    } else {
        copy_file(src, dst)
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        util::ensure_dir(parent)?;
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

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Err(Error::msg(format!(
            "source is not a directory: {}",
            src.display()
        )));
    }

    util::ensure_dir(dst)?;
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
            util::ensure_dir(&out)?;
        } else if entry.file_type().is_symlink() {
            copy_symlink(p, &out)?;
        } else {
            copy_file(p, &out)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    use std::os::unix::fs as unix_fs;

    if let Some(parent) = dst.parent() {
        util::ensure_dir(parent)?;
    }
    if fs::symlink_metadata(dst).is_ok() {
        fs::remove_file(dst)
            .map_err(|e| Error::msg(format!("failed to remove {}: {e}", dst.display())))?;
    }
    let target = fs::read_link(src)
        .map_err(|e| Error::msg(format!("failed to read symlink {}: {e}", src.display())))?;
    unix_fs::symlink(&target, dst).map_err(|e| {
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
    copy_file(src, dst)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| Error::msg(format!("failed to set mode on {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn ensure_symlink(target: &str, link_path: &Path) -> Result<()> {
    use std::os::unix::fs as unix_fs;

    if let Some(parent) = link_path.parent() {
        util::ensure_dir(parent)?;
    }

    if let Ok(existing) = fs::read_link(link_path) {
        if existing == PathBuf::from(target) {
            return Ok(());
        }
        fs::remove_file(link_path)
            .map_err(|e| Error::msg(format!("failed to remove {}: {e}", link_path.display())))?;
    } else if link_path.exists() {
        fs::remove_file(link_path)
            .map_err(|e| Error::msg(format!("failed to remove {}: {e}", link_path.display())))?;
    }

    unix_fs::symlink(target, link_path).map_err(|e| {
        Error::msg(format!(
            "failed to create symlink {} -> {}: {e}",
            link_path.display(),
            target
        ))
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_symlink(_target: &str, _link_path: &Path) -> Result<()> {
    Err(Error::msg("symlink creation is only supported on unix"))
}
