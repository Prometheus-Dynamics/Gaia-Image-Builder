use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

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
pub struct BuildrootRpiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub arch: String,
    pub board: Option<String>,
    pub defconfig: Option<String>,
    pub overlay: Option<String>,
    pub config_file: Option<String>,
    pub cmdline_file: Option<String>,
}

impl Default for BuildrootRpiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            arch: "aarch64".into(),
            board: None,
            defconfig: None,
            overlay: None,
            config_file: None,
            cmdline_file: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct BuildrootBaseConfig {
    defconfig: Option<String>,
}

impl Default for BuildrootBaseConfig {
    fn default() -> Self {
        Self { defconfig: None }
    }
}

#[Task(
    id = "buildroot.rpi.validate",
    module = "buildroot.rpi",
    phase = "preflight",
    provides = ["buildroot:target-validated"],
    after = ["core.init"],
    default_label = "Validate target config",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ValidateTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for ValidateTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl ValidateTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let cfg: BuildrootRpiConfig = doc.deserialize_path("buildroot.rpi")?.unwrap_or_default();
        let br: BuildrootBaseConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
        let ws = ctx.workspace_paths_or_init(doc)?;
        let out_dir = util::module_dir(doc, ctx, "buildroot.rpi")?;
        util::ensure_dir(&out_dir)?;

        match cfg.arch.trim() {
            "aarch64" | "arm64" => {}
            other => {
                return Err(Error::msg(format!(
                    "buildroot.rpi.arch='{}' is unsupported (expected 'aarch64' or 'arm64')",
                    other
                )));
            }
        }

        let defconfig = cfg
            .defconfig
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                br.defconfig
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .ok_or_else(|| {
                Error::msg("buildroot.rpi.defconfig (or buildroot.defconfig) is required")
            })?;

        let mut checks = Vec::new();
        if let Some(overlay) = cfg
            .overlay
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let p = resolve_input_path(&ws, overlay)?;
            if !p.is_dir() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.overlay must be a directory: {}",
                    p.display()
                )));
            }
            checks.push(serde_json::json!({
                "kind": "overlay",
                "path": p.display().to_string(),
            }));
        }
        if let Some(config_file) = cfg
            .config_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let p = resolve_input_path(&ws, config_file)?;
            if !p.is_file() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.config_file not found: {}",
                    p.display()
                )));
            }
            checks.push(serde_json::json!({
                "kind": "config_file",
                "path": p.display().to_string(),
            }));
        }
        if let Some(cmdline_file) = cfg
            .cmdline_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let p = resolve_input_path(&ws, cmdline_file)?;
            if !p.is_file() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.cmdline_file not found: {}",
                    p.display()
                )));
            }
            checks.push(serde_json::json!({
                "kind": "cmdline_file",
                "path": p.display().to_string(),
            }));
        }

        util::write_json_pretty(
            &out_dir.join("validate.json"),
            &serde_json::json!({
                "arch": cfg.arch,
                "board": cfg.board,
                "defconfig": defconfig,
                "checks": checks,
            }),
        )?;
        ctx.log(&format!("validated buildroot.rpi (defconfig={defconfig})"));
        Ok(())
    }
}

#[Task(
    id = "buildroot.rpi.prepare",
    module = "buildroot.rpi",
    phase = "prepare",
    provides = ["buildroot:target-prepared"],
    after = ["buildroot.rpi.validate"],
    default_label = "Prepare target assets",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrepareTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for PrepareTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl PrepareTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let cfg: BuildrootRpiConfig = doc.deserialize_path("buildroot.rpi")?.unwrap_or_default();
        let ws = ctx.workspace_paths_or_init(doc)?;
        let dir = util::module_dir(doc, ctx, "buildroot.rpi")?;
        util::ensure_dir(&dir)?;
        let stage_root = util::stage_root_dir(doc, ctx)?;
        util::ensure_dir(&stage_root)?;

        let mut actions = Vec::new();

        if let Some(overlay) = cfg
            .overlay
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let src = resolve_input_path(&ws, overlay)?;
            if !src.is_dir() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.overlay must be a directory: {}",
                    src.display()
                )));
            }
            copy_dir_all(&src, &stage_root)?;
            actions.push(serde_json::json!({
                "action": "copy-overlay",
                "src": src.display().to_string(),
                "dst": stage_root.display().to_string(),
            }));
        }

        if let Some(config_file) = cfg
            .config_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let src = resolve_input_path(&ws, config_file)?;
            if !src.is_file() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.config_file not found: {}",
                    src.display()
                )));
            }
            let dst = util::stage_path(doc, ctx, "/boot/config.txt")?;
            copy_file(&src, &dst)?;
            actions.push(serde_json::json!({
                "action": "copy-boot-config",
                "src": src.display().to_string(),
                "dst": "/boot/config.txt",
            }));
        }

        if let Some(cmdline_file) = cfg
            .cmdline_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let src = resolve_input_path(&ws, cmdline_file)?;
            if !src.is_file() {
                return Err(Error::msg(format!(
                    "buildroot.rpi.cmdline_file not found: {}",
                    src.display()
                )));
            }
            let dst = util::stage_path(doc, ctx, "/boot/cmdline.txt")?;
            copy_file(&src, &dst)?;
            actions.push(serde_json::json!({
                "action": "copy-cmdline",
                "src": src.display().to_string(),
                "dst": "/boot/cmdline.txt",
            }));
        }

        util::write_json_pretty(
            &dir.join("manifest.json"),
            &serde_json::json!({
                "stage_root": stage_root.display().to_string(),
                "actions": actions,
            }),
        )?;
        Ok(())
    }
}

#[Module(
    id = "buildroot.rpi",
    config = BuildrootRpiConfig,
    config_path = "buildroot.rpi",
    tasks = [ValidateTask, PrepareTask]
)]
pub struct BuildrootRpiModule;

fn resolve_input_path(ws: &WorkspacePaths, raw: &str) -> Result<PathBuf> {
    ws.resolve_config_path(raw)
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
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

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
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
            copy_file(p, &out)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
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
