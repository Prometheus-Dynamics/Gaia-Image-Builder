use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::{ExecCtx, ModuleExec, TaskRegistry};
use crate::modules::program::read_artifact_record;
use crate::modules::util;
use crate::planner::{Plan, Task};

const TASK_ID: &str = "program.install.stage";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProgramInstallConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub items: Vec<ProgramInstallItem>,
}

impl Default for ProgramInstallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProgramInstallItem {
    pub artifact: String,
    pub dest: String,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl Default for ProgramInstallItem {
    fn default() -> Self {
        Self {
            artifact: String::new(),
            dest: String::new(),
            mode: None,
            owner: None,
            group: None,
        }
    }
}

pub struct ProgramInstallModule;

impl crate::modules::Module for ProgramInstallModule {
    fn id(&self) -> &'static str {
        "program.install"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path(self.id())
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: ProgramInstallConfig = doc.deserialize_path(self.id())?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }

        for item in &cfg.items {
            if item.artifact.trim().is_empty() {
                return Err(Error::msg("program.install.items[].artifact is empty"));
            }
            if item.dest.trim().is_empty() {
                return Err(Error::msg("program.install.items[].dest is empty"));
            }
            if !item.dest.trim().starts_with('/') {
                return Err(Error::msg(format!(
                    "program.install.items[].dest must be an absolute image path, got '{}'",
                    item.dest
                )));
            }
        }

        plan.add(Task {
            id: TASK_ID.into(),
            label: "Install program artifacts".into(),
            module: self.id().into(),
            phase: "stage".into(),
            after: vec![
                "core.init".into(),
                "artifacts:rust?".into(),
                "artifacts:java?".into(),
                "artifacts:custom?".into(),
            ],
            provides: vec!["stage:program-install".into()],
        })
    }
}

impl ModuleExec for ProgramInstallModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(TASK_ID, exec)
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Err(Error::msg(format!(
            "source is not a directory: {}",
            src.display()
        )));
    }
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

    Ok(())
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

fn exec(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(TASK_ID);

    let cfg: ProgramInstallConfig = doc.deserialize_path("program.install")?.unwrap_or_default();
    if !cfg.enabled {
        return Ok(());
    }

    let stage_root = util::stage_root_dir(doc, ctx)?;
    util::ensure_dir(&stage_root)?;

    let mut installed = Vec::new();

    for item in &cfg.items {
        let artifact_id = item.artifact.trim();
        if artifact_id.is_empty() {
            return Err(Error::msg("program.install.items[].artifact is empty"));
        }

        let rec = read_artifact_record(doc, ctx, artifact_id)?;
        let src = PathBuf::from(&rec.abs_path);
        if !src.exists() {
            return Err(Error::msg(format!(
                "artifact '{}' points to missing path {}",
                artifact_id,
                src.display()
            )));
        }

        let dst = util::stage_path(doc, ctx, item.dest.trim())?;
        if src.is_dir() {
            copy_dir_all(&src, &dst)?;
        } else {
            if let Some(parent) = dst.parent() {
                util::ensure_dir(parent)?;
            }
            fs::copy(&src, &dst).map_err(|e| {
                Error::msg(format!(
                    "failed to install {} -> {}: {e}",
                    src.display(),
                    dst.display()
                ))
            })?;
        }

        if let Some(mode) = item.mode {
            set_mode(&dst, mode)?;
        }

        apply_owner_group(ctx, &dst, item.owner.as_deref(), item.group.as_deref())?;

        installed.push(serde_json::json!({
            "artifact": artifact_id,
            "producer": rec.producer,
            "src": rec.abs_path,
            "dst": item.dest,
            "mode": item.mode,
            "owner": item.owner,
            "group": item.group,
        }));
    }

    let dir = util::module_dir(doc, ctx, "program.install")?;
    util::ensure_dir(&dir)?;
    util::write_json_pretty(
        &dir.join("manifest.json"),
        &serde_json::json!({
            "stage_root": stage_root.display().to_string(),
            "items": installed,
        }),
    )?;

    Ok(())
}

#[cfg(unix)]
fn apply_owner_group(
    ctx: &ExecCtx,
    path: &Path,
    owner: Option<&str>,
    group: Option<&str>,
) -> Result<()> {
    let owner = owner.map(str::trim).filter(|s| !s.is_empty());
    let group = group.map(str::trim).filter(|s| !s.is_empty());
    if owner.is_none() && group.is_none() {
        return Ok(());
    }

    let mut spec = String::new();
    if let Some(o) = owner {
        spec.push_str(o);
    }
    if let Some(g) = group {
        spec.push(':');
        spec.push_str(g);
    }

    let mut cmd = Command::new("chown");
    if path.is_dir() {
        cmd.arg("-R");
    }
    cmd.arg(spec).arg(path);
    ctx.run_cmd(cmd)
}

#[cfg(not(unix))]
fn apply_owner_group(
    _ctx: &ExecCtx,
    _path: &Path,
    owner: Option<&str>,
    group: Option<&str>,
) -> Result<()> {
    if owner.is_some() || group.is_some() {
        return Err(Error::msg(
            "program.install owner/group is only supported on unix",
        ));
    }
    Ok(())
}
