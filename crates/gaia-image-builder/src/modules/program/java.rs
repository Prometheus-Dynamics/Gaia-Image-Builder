use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

use serde::Deserialize;

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::{ExecCtx, ModuleExec, TaskRegistry};
use crate::modules::program::{
    ArtifactBuildState, ArtifactMode, ArtifactRecord, ProgramConfig, compute_artifact_fingerprint,
    effective_check_ids, load_program_cfg, now_rfc3339, path_kind, read_artifact_state,
    resolve_from_workspace_root, resolve_profile, run_checks, validate_program_definitions,
    write_artifact_record, write_artifact_state,
};
use crate::modules::util;
use crate::planner::{Plan, Task};

const TASK_ID: &str = "program.java.artifacts";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct JavaArtifact {
    id: String,
    mode: ArtifactMode,
    profile: Option<String>,
    check_ids: Vec<String>,
    prebuilt_path: Option<String>,
    output_path: Option<String>,
    build_command: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
}

impl Default for JavaArtifact {
    fn default() -> Self {
        Self {
            id: String::new(),
            mode: ArtifactMode::Auto,
            profile: None,
            check_ids: Vec::new(),
            prebuilt_path: None,
            output_path: None,
            build_command: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct BuildJavaConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    workspace_dir: String,
    check_ids: Vec<String>,
    artifacts: Vec<JavaArtifact>,
}

impl Default for BuildJavaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_dir: ".".into(),
            check_ids: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

pub struct ProgramJavaModule;

impl crate::modules::Module for ProgramJavaModule {
    fn id(&self) -> &'static str {
        "program.java"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path(self.id())
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: BuildJavaConfig = doc.deserialize_path(self.id())?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }

        validate_program_definitions(doc)?;

        plan.add(Task {
            id: TASK_ID.into(),
            label: "Build Java programs".into(),
            module: self.id().into(),
            phase: "build".into(),
            after: vec!["core.init".into(), "program:linted?".into()],
            provides: vec!["artifacts:java".into()],
        })
    }
}

impl ModuleExec for ProgramJavaModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(TASK_ID, exec)
    }
}

fn exec(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(TASK_ID);

    validate_program_definitions(doc)?;

    let cfg: BuildJavaConfig = doc.deserialize_path("program.java")?.unwrap_or_default();
    if !cfg.enabled {
        return Ok(());
    }

    let ws = ctx.workspace_paths_or_init(doc)?;
    let build_cfg = load_program_cfg(doc)?;
    let workspace_dir = resolve_from_workspace_root(&ws, &cfg.workspace_dir)?;
    let module_dir = util::module_dir(doc, ctx, "program.java")?;
    util::ensure_dir(&module_dir)?;

    let mut manifest = Vec::new();
    let artifacts = cfg.artifacts.clone();
    if artifacts.len() <= 1 {
        for artifact in artifacts {
            manifest.push(build_one_artifact(
                doc,
                ctx,
                &ws,
                &build_cfg,
                &workspace_dir,
                &cfg.check_ids,
                artifact,
            )?);
        }
    } else {
        let (tx, rx) = mpsc::channel::<Result<serde_json::Value>>();
        let default_check_ids = cfg.check_ids.clone();
        std::thread::scope(|scope| {
            for artifact in artifacts {
                let tx = tx.clone();
                let ws = ws.clone();
                let build_cfg = build_cfg.clone();
                let workspace_dir = workspace_dir.clone();
                let default_check_ids = default_check_ids.clone();
                let mut local_ctx = ctx.clone();
                let aid = artifact.id.trim().to_string();
                if !aid.is_empty() {
                    local_ctx.set_task(format!("{TASK_ID}:{aid}"));
                }
                scope.spawn(move || {
                    let res = build_one_artifact(
                        doc,
                        &mut local_ctx,
                        &ws,
                        &build_cfg,
                        &workspace_dir,
                        &default_check_ids,
                        artifact,
                    );
                    let _ = tx.send(res);
                });
            }
            drop(tx);
            let mut first_err: Option<Error> = None;
            for res in rx {
                match res {
                    Ok(row) => manifest.push(row),
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
            }
            if let Some(e) = first_err {
                return Err(e);
            }
            Ok::<(), Error>(())
        })?;
    }
    manifest.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .cmp(&b.get("id").and_then(|v| v.as_str()))
    });

    util::write_json_pretty(
        &module_dir.join("manifest.json"),
        &serde_json::json!({
            "workspace_dir": workspace_dir.display().to_string(),
            "artifacts": manifest,
        }),
    )?;

    Ok(())
}

fn build_one_artifact(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    ws: &crate::workspace::WorkspacePaths,
    build_cfg: &ProgramConfig,
    workspace_dir: &Path,
    default_check_ids: &[String],
    artifact: JavaArtifact,
) -> Result<serde_json::Value> {
    let aid = artifact.id.trim();
    if aid.is_empty() {
        return Err(Error::msg("program.java.artifacts[].id is empty"));
    }

    let profile = resolve_profile(build_cfg, artifact.profile.as_deref())?;
    let prebuilt = artifact
        .prebuilt_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| resolve_from_workspace_root(ws, p))
        .transpose()?;

    let output = artifact
        .output_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| resolve_from_workspace_root(ws, p))
        .transpose()?;
    let cwd = artifact
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| resolve_from_workspace_root(ws, p))
        .transpose()?
        .unwrap_or_else(|| workspace_dir.to_path_buf());
    let selected = effective_check_ids(default_check_ids, &artifact.check_ids);

    let fingerprint = if matches!(artifact.mode, ArtifactMode::Prebuilt) {
        None
    } else {
        let payload = serde_json::json!({
            "builder": "program.java",
            "id": aid,
            "mode": format!("{:?}", artifact.mode),
            "profile": artifact.profile.clone(),
            "target": profile.and_then(|p| p.target.clone()),
            "profile_env": profile.map(|p| p.env.clone()),
            "artifact_env": artifact.env.clone(),
            "build_command": artifact.build_command.clone(),
            "cwd": cwd.display().to_string(),
            "output": output.as_ref().map(|x| x.display().to_string()),
            "check_ids": selected.clone(),
        });
        Some(compute_artifact_fingerprint(&payload, &cwd)?)
    };

    let can_skip_auto = if matches!(artifact.mode, ArtifactMode::Auto) {
        if let Some(out) = output.as_ref() {
            let has_usable_prebuilt = prebuilt.as_ref().is_some_and(|p| p.exists());
            if !has_usable_prebuilt && out.exists() {
                if let (Some(state), Some(fp)) =
                    (read_artifact_state(doc, ctx, aid)?, fingerprint.as_ref())
                {
                    let expected = out
                        .canonicalize()
                        .unwrap_or_else(|_| out.to_path_buf())
                        .display()
                        .to_string();
                    let actual = PathBuf::from(&state.output_abs_path)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(&state.output_abs_path))
                        .display()
                        .to_string();
                    state.producer == "program.java"
                        && state.fingerprint == *fp
                        && actual == expected
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let run_build = |ctx: &mut ExecCtx| -> Result<()> {
        if artifact.build_command.is_empty() {
            return Err(Error::msg(format!(
                "artifact '{}' requires build_command for build mode",
                aid
            )));
        }
        let mut cmd = Command::new(&artifact.build_command[0]);
        if artifact.build_command.len() > 1 {
            cmd.args(&artifact.build_command[1..]);
        }
        cmd.current_dir(&cwd);
        if let Some(p) = profile {
            for (k, v) in &p.env {
                cmd.env(k, v);
            }
            if let Some(target) = p.target.as_deref() {
                cmd.env("BUILD_TARGET", target);
            }
        }
        for (k, v) in &artifact.env {
            cmd.env(k, v);
        }
        ctx.run_cmd(cmd)
    };

    let produced = match artifact.mode {
        ArtifactMode::Prebuilt => prebuilt.ok_or_else(|| {
            Error::msg(format!(
                "artifact '{}' mode=prebuilt requires prebuilt_path",
                aid
            ))
        })?,
        ArtifactMode::Auto => {
            if let Some(p) = prebuilt
                && p.exists()
            {
                p
            } else if can_skip_auto {
                ctx.log(&format!("artifact:{} unchanged; skipping build", aid));
                output.clone().ok_or_else(|| {
                    Error::msg(format!(
                        "artifact '{}' mode=auto requires output_path when no usable prebuilt exists",
                        aid
                    ))
                })?
            } else {
                let out = output.clone().ok_or_else(|| {
                    Error::msg(format!(
                        "artifact '{}' mode=auto requires output_path when no usable prebuilt exists",
                        aid
                    ))
                })?;
                run_checks(doc, ctx, "java", workspace_dir, &selected)?;
                run_build(ctx)?;
                out
            }
        }
        ArtifactMode::Build => {
            let out = output.clone().ok_or_else(|| {
                Error::msg(format!(
                    "artifact '{}' mode=build requires output_path",
                    aid
                ))
            })?;
            run_checks(doc, ctx, "java", workspace_dir, &selected)?;
            run_build(ctx)?;
            out
        }
    };

    if !produced.exists() {
        return Err(Error::msg(format!(
            "artifact '{}' output not found at {}",
            aid,
            produced.display()
        )));
    }

    let kind = path_kind(&produced)?;
    let abs = produced
        .canonicalize()
        .unwrap_or_else(|_| produced.clone())
        .display()
        .to_string();

    write_artifact_record(
        doc,
        ctx,
        &ArtifactRecord {
            id: aid.to_string(),
            producer: "program.java".into(),
            kind: kind.into(),
            abs_path: abs.clone(),
        },
    )?;

    if let Some(fp) = fingerprint {
        write_artifact_state(
            doc,
            ctx,
            &ArtifactBuildState {
                id: aid.to_string(),
                producer: "program.java".into(),
                fingerprint: fp,
                output_abs_path: abs.clone(),
                updated_at: now_rfc3339(),
            },
        )?;
    }

    Ok(serde_json::json!({
        "id": aid,
        "kind": kind,
        "path": abs,
        "mode": format!("{:?}", artifact.mode),
    }))
}
