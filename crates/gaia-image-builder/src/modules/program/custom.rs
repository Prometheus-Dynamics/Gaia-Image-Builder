use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

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

const TASK_ID: &str = "program.custom.artifacts";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct CustomArtifact {
    id: String,
    mode: ArtifactMode,
    profile: Option<String>,
    check_ids: Vec<String>,
    enabled_if: Vec<String>,
    disabled_if: Vec<String>,
    after_artifacts: Vec<String>,
    prebuilt_path: Option<String>,
    output_path: Option<String>,
    build_command: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
}

impl Default for CustomArtifact {
    fn default() -> Self {
        Self {
            id: String::new(),
            mode: ArtifactMode::Auto,
            profile: None,
            check_ids: Vec::new(),
            enabled_if: Vec::new(),
            disabled_if: Vec::new(),
            after_artifacts: Vec::new(),
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
struct BuildCustomConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    workspace_dir: String,
    check_ids: Vec<String>,
    artifacts: Vec<CustomArtifact>,
}

impl Default for BuildCustomConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_dir: ".".into(),
            check_ids: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

pub struct ProgramCustomModule;

fn selected_artifacts(
    doc: &ConfigDoc,
    artifacts: &[CustomArtifact],
) -> Result<Vec<CustomArtifact>> {
    let mut out = Vec::new();
    for artifact in artifacts {
        let enabled =
            crate::build_inputs::conditions_match(doc, &artifact.enabled_if, &artifact.disabled_if)
                .map_err(|e| {
                    let id = artifact.id.trim();
                    let id = if id.is_empty() { "<empty>" } else { id };
                    Error::msg(format!(
                        "program.custom.artifacts id='{}' condition evaluation failed: {}",
                        id, e
                    ))
                })?;
        if enabled {
            out.push(artifact.clone());
        }
    }
    Ok(out)
}

fn ordered_artifacts(artifacts: Vec<CustomArtifact>) -> Result<Vec<CustomArtifact>> {
    if artifacts.len() <= 1 {
        return Ok(artifacts);
    }

    let mut by_id = BTreeMap::<String, CustomArtifact>::new();
    let mut order = BTreeMap::<String, usize>::new();
    for (idx, artifact) in artifacts.into_iter().enumerate() {
        let id = artifact.id.trim().to_string();
        if id.is_empty() {
            return Err(Error::msg("program.custom.artifacts[].id is empty"));
        }
        if by_id.contains_key(&id) {
            return Err(Error::msg(format!(
                "duplicate program.custom artifact id '{}'",
                id
            )));
        }
        order.insert(id.clone(), idx);
        by_id.insert(id, artifact);
    }

    let mut incoming = BTreeMap::<String, usize>::new();
    let mut outgoing = BTreeMap::<String, BTreeSet<String>>::new();
    for id in by_id.keys() {
        incoming.insert(id.clone(), 0);
        outgoing.entry(id.clone()).or_default();
    }

    for (id, artifact) in &by_id {
        for dep_raw in &artifact.after_artifacts {
            let dep_raw = dep_raw.trim();
            if dep_raw.is_empty() {
                continue;
            }
            let (dep, optional) = dep_raw
                .strip_suffix('?')
                .map(|d| (d.trim(), true))
                .unwrap_or((dep_raw, false));
            if dep.is_empty() {
                return Err(Error::msg(format!(
                    "program.custom artifact '{}' has empty dependency entry in after_artifacts",
                    id
                )));
            }
            if !incoming.contains_key(dep) {
                if optional {
                    continue;
                }
                return Err(Error::msg(format!(
                    "program.custom artifact '{}' depends on unknown artifact '{}'",
                    id, dep
                )));
            }
            outgoing
                .entry(dep.to_string())
                .or_default()
                .insert(id.clone());
            *incoming.get_mut(id).expect("id exists") += 1;
        }
    }

    let mut ready = incoming
        .iter()
        .filter_map(|(id, in_deg)| (*in_deg == 0).then_some(id.clone()))
        .collect::<Vec<_>>();
    ready.sort_by_key(|id| *order.get(id).unwrap_or(&usize::MAX));
    let mut q = VecDeque::from(ready);
    let mut out_ids = Vec::<String>::new();

    while let Some(id) = q.pop_front() {
        out_ids.push(id.clone());
        if let Some(children) = outgoing.get(&id) {
            for child in children {
                let slot = incoming.get_mut(child).expect("child exists");
                *slot -= 1;
                if *slot == 0 {
                    q.push_back(child.clone());
                }
            }
            if q.len() > 1 {
                let mut drained = q.drain(..).collect::<Vec<_>>();
                drained.sort_by_key(|tid| *order.get(tid).unwrap_or(&usize::MAX));
                q = VecDeque::from(drained);
            }
        }
    }

    if out_ids.len() != by_id.len() {
        let remaining = incoming
            .iter()
            .filter_map(|(id, in_deg)| (*in_deg > 0).then_some(id.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::msg(format!(
            "program.custom artifact dependency cycle detected; remaining: {}",
            remaining
        )));
    }

    let mut out = Vec::with_capacity(out_ids.len());
    for id in out_ids {
        out.push(
            by_id
                .remove(&id)
                .ok_or_else(|| Error::msg(format!("missing artifact '{}'", id)))?,
        );
    }
    Ok(out)
}

impl crate::modules::Module for ProgramCustomModule {
    fn id(&self) -> &'static str {
        "program.custom"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path(self.id())
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: BuildCustomConfig = doc.deserialize_path(self.id())?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }
        let selected = selected_artifacts(doc, &cfg.artifacts)?;
        if selected.is_empty() {
            return Ok(());
        }
        let _ordered = ordered_artifacts(selected)?;

        validate_program_definitions(doc)?;

        plan.add(Task {
            id: TASK_ID.into(),
            label: "Build custom programs".into(),
            module: self.id().into(),
            phase: "build".into(),
            after: vec!["core.init".into(), "program:linted?".into()],
            provides: vec!["artifacts:custom".into()],
        })
    }
}

impl ModuleExec for ProgramCustomModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(TASK_ID, exec)
    }
}

fn exec(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(TASK_ID);

    validate_program_definitions(doc)?;

    let cfg: BuildCustomConfig = doc.deserialize_path("program.custom")?.unwrap_or_default();
    if !cfg.enabled {
        return Ok(());
    }

    let ws = ctx.workspace_paths_or_init(doc)?;
    let build_cfg = load_program_cfg(doc)?;
    let workspace_dir = resolve_from_workspace_root(&ws, &cfg.workspace_dir)?;
    let module_dir = util::module_dir(doc, ctx, "program.custom")?;
    util::ensure_dir(&module_dir)?;

    let mut manifest = Vec::new();
    let selected = selected_artifacts(doc, &cfg.artifacts)?;
    let artifacts = ordered_artifacts(selected)?;
    if artifacts.is_empty() {
        ctx.log("program.custom: no artifacts selected by conditions");
    }
    for artifact in artifacts {
        let aid = artifact.id.trim().to_string();
        if !aid.is_empty() {
            ctx.set_task(format!("{TASK_ID}:{aid}"));
        }
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
    ctx.set_task(TASK_ID);
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
    artifact: CustomArtifact,
) -> Result<serde_json::Value> {
    let aid = artifact.id.trim();
    if aid.is_empty() {
        return Err(Error::msg("program.custom.artifacts[].id is empty"));
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
            "builder": "program.custom",
            "id": aid,
            "mode": format!("{:?}", artifact.mode),
            "profile": artifact.profile.clone(),
            "enabled_if": artifact.enabled_if.clone(),
            "disabled_if": artifact.disabled_if.clone(),
            "after_artifacts": artifact.after_artifacts.clone(),
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

    let build_state = if matches!(artifact.mode, ArtifactMode::Prebuilt) {
        None
    } else {
        read_artifact_state(doc, ctx, aid)?
    };
    let has_build_state = build_state.is_some();

    let state_matches_path = |path: &Path| {
        if let (Some(state), Some(fp)) = (build_state.as_ref(), fingerprint.as_ref()) {
            let expected = path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .display()
                .to_string();
            let actual = PathBuf::from(&state.output_abs_path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&state.output_abs_path))
                .display()
                .to_string();
            state.producer == "program.custom" && state.fingerprint == *fp && actual == expected
        } else {
            false
        }
    };

    let can_skip_auto = if matches!(artifact.mode, ArtifactMode::Auto) {
        output
            .as_ref()
            .is_some_and(|out| out.exists() && state_matches_path(out))
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
        let mut input_envs = BTreeMap::new();
        crate::build_inputs::inject_env_vars(doc, &mut input_envs, None)?;
        for (k, v) in input_envs {
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
            if can_skip_auto {
                ctx.log(&format!("artifact:{} unchanged; skipping build", aid));
                output.clone().ok_or_else(|| {
                    Error::msg(format!(
                        "artifact '{}' mode=auto requires output_path when no usable prebuilt exists",
                        aid
                    ))
                })?
            } else if let Some(p) = prebuilt
                && p.exists()
                && (!has_build_state || state_matches_path(&p))
            {
                if has_build_state {
                    ctx.log(&format!("artifact:{} unchanged; using prebuilt", aid));
                } else {
                    ctx.log(&format!(
                        "artifact:{} using prebuilt (no prior build state found)",
                        aid
                    ));
                }
                p
            } else {
                let out = output.clone().ok_or_else(|| {
                    Error::msg(format!(
                        "artifact '{}' mode=auto requires output_path when no usable prebuilt exists",
                        aid
                    ))
                })?;
                run_checks(doc, ctx, "custom", workspace_dir, &selected)?;
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
            run_checks(doc, ctx, "custom", workspace_dir, &selected)?;
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
            producer: "program.custom".into(),
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
                producer: "program.custom".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(src: &str) -> ConfigDoc {
        ConfigDoc {
            path: "inline.toml".into(),
            value: toml::from_str(src).expect("valid toml"),
        }
    }

    #[test]
    fn selected_artifacts_honors_conditions() {
        let mut doc = make_doc(
            r#"
[inputs.values]
jar_mode = "release"
"#,
        );
        crate::build_inputs::apply_cli_overrides(&mut doc, &[]).expect("resolve inputs");

        let mut release = CustomArtifact {
            id: "release-jar".into(),
            ..CustomArtifact::default()
        };
        release.enabled_if = vec!["jar_mode=release".into()];

        let mut repo = CustomArtifact {
            id: "repo-jar".into(),
            ..CustomArtifact::default()
        };
        repo.enabled_if = vec!["jar_mode=repo".into()];

        let selected = selected_artifacts(&doc, &[release, repo]).expect("selected");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "release-jar");
    }

    #[test]
    fn ordered_artifacts_respects_after_dependencies() {
        let base = CustomArtifact {
            id: "base".into(),
            ..CustomArtifact::default()
        };
        let mut app = CustomArtifact {
            id: "app".into(),
            ..CustomArtifact::default()
        };
        app.after_artifacts = vec!["base".into()];

        let ordered = ordered_artifacts(vec![app, base]).expect("ordered");
        let ids = ordered.into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids, vec!["base", "app"]);
    }

    #[test]
    fn ordered_artifacts_detects_cycle() {
        let mut a = CustomArtifact {
            id: "a".into(),
            ..CustomArtifact::default()
        };
        let mut b = CustomArtifact {
            id: "b".into(),
            ..CustomArtifact::default()
        };
        a.after_artifacts = vec!["b".into()];
        b.after_artifacts = vec!["a".into()];

        let err = ordered_artifacts(vec![a, b]).expect_err("cycle");
        assert!(err.to_string().contains("dependency cycle"));
    }
}
