use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc};

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
use crate::workspace::WorkspacePaths;

const TASK_ID: &str = "program.rust.artifacts";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RustArtifactKind {
    Bin,
    Cdylib,
    File,
    Dir,
}

impl Default for RustArtifactKind {
    fn default() -> Self {
        Self::Bin
    }
}

fn default_release() -> String {
    "release".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct RustArtifact {
    id: String,
    package: Option<String>,
    kind: RustArtifactKind,
    mode: ArtifactMode,
    profile: Option<String>,
    check_ids: Vec<String>,
    enabled_if: Vec<String>,
    disabled_if: Vec<String>,
    prebuilt_path: Option<String>,
    output_path: Option<String>,
    build_command: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    cargo_profile: String,
}

impl Default for RustArtifact {
    fn default() -> Self {
        Self {
            id: String::new(),
            package: None,
            kind: RustArtifactKind::Bin,
            mode: ArtifactMode::Auto,
            profile: None,
            check_ids: Vec::new(),
            enabled_if: Vec::new(),
            disabled_if: Vec::new(),
            prebuilt_path: None,
            output_path: None,
            build_command: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            cargo_profile: default_release(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct RustBuildConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    workspace_dir: String,
    check_ids: Vec<String>,
    artifacts: Vec<RustArtifact>,
}

impl Default for RustBuildConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_dir: ".".into(),
            check_ids: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

pub struct ProgramRustModule;

fn selected_artifacts(doc: &ConfigDoc, artifacts: &[RustArtifact]) -> Result<Vec<RustArtifact>> {
    let mut out = Vec::new();
    for artifact in artifacts {
        let enabled =
            crate::build_inputs::conditions_match(doc, &artifact.enabled_if, &artifact.disabled_if)
                .map_err(|e| {
                    let id = artifact.id.trim();
                    let id = if id.is_empty() { "<empty>" } else { id };
                    Error::msg(format!(
                        "program.rust.artifacts id='{}' condition evaluation failed: {}",
                        id, e
                    ))
                })?;
        if enabled {
            out.push(artifact.clone());
        }
    }
    Ok(out)
}

impl crate::modules::Module for ProgramRustModule {
    fn id(&self) -> &'static str {
        "program.rust"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path(self.id())
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: RustBuildConfig = doc.deserialize_path(self.id())?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }
        let selected = selected_artifacts(doc, &cfg.artifacts)?;
        if selected.is_empty() {
            return Ok(());
        }

        validate_program_definitions(doc)?;

        plan.add(Task {
            id: TASK_ID.into(),
            label: "Build Rust programs".into(),
            module: self.id().into(),
            phase: "build".into(),
            after: vec!["core.init".into(), "program:linted?".into()],
            provides: vec!["artifacts:rust".into()],
        })
    }
}

impl ModuleExec for ProgramRustModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(TASK_ID, exec)
    }
}

fn infer_default_output(
    target_dir: &Path,
    profile_target: Option<&str>,
    cargo_profile: &str,
    kind: &RustArtifactKind,
    package: Option<&str>,
) -> Result<PathBuf> {
    let pkg = package.ok_or_else(|| Error::msg("missing package for inferred rust output"))?;
    let mut p = target_dir.to_path_buf();
    if let Some(target) = profile_target
        && !target.trim().is_empty()
    {
        p = p.join(target.trim());
    }
    p = p.join(cargo_profile);

    match kind {
        RustArtifactKind::Bin => Ok(p.join(pkg)),
        RustArtifactKind::Cdylib => {
            let lib = format!("lib{}.so", pkg.replace('-', "_"));
            Ok(p.join(lib))
        }
        RustArtifactKind::File | RustArtifactKind::Dir => {
            Err(Error::msg("output_path is required for rust kind=file/dir"))
        }
    }
}

fn run_build_command(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    ws: &WorkspacePaths,
    cwd: &Path,
    profile_container_image: Option<&str>,
    profile_target: Option<&str>,
    artifact: &RustArtifact,
    profile_env: &BTreeMap<String, String>,
) -> Result<()> {
    let mut envs = profile_env.clone();
    for (k, v) in &artifact.env {
        envs.insert(k.clone(), v.clone());
    }
    crate::build_inputs::inject_env_vars(doc, &mut envs, None)?;

    if artifact.build_command.is_empty() {
        let package = artifact.package.as_deref().ok_or_else(|| {
            Error::msg("program.rust.artifacts[].package is required when build_command is not set")
        })?;

        let mut argv = vec![
            "cargo".to_string(),
            "build".to_string(),
            "-p".to_string(),
            package.to_string(),
        ];

        if let Some(target) = profile_target
            && !target.trim().is_empty()
        {
            argv.push("--target".to_string());
            argv.push(target.trim().to_string());
        }

        if artifact.cargo_profile == "release" {
            argv.push("--release".to_string());
        } else if !artifact.cargo_profile.trim().is_empty() {
            argv.push("--profile".to_string());
            argv.push(artifact.cargo_profile.trim().to_string());
        }

        run_with_optional_container(ctx, ws, cwd, profile_container_image, &argv, &envs)
    } else {
        if let Some(target) = profile_target
            && !target.trim().is_empty()
        {
            envs.insert("CARGO_BUILD_TARGET".into(), target.trim().into());
        }
        run_with_optional_container(
            ctx,
            ws,
            cwd,
            profile_container_image,
            &artifact.build_command,
            &envs,
        )
    }
}

fn run_with_optional_container(
    ctx: &mut ExecCtx,
    ws: &WorkspacePaths,
    cwd: &Path,
    container_image: Option<&str>,
    argv: &[String],
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    let image = container_image
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    if let Some(image) = image {
        run_in_container(ctx, ws, cwd, &image, argv, envs)
    } else {
        if argv.is_empty() {
            return Err(Error::msg("build command is empty"));
        }
        let mut cmd = Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }
        cmd.current_dir(cwd);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        ctx.run_cmd(cmd)
    }
}

fn run_in_container(
    ctx: &mut ExecCtx,
    ws: &WorkspacePaths,
    cwd: &Path,
    image: &str,
    argv: &[String],
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    if argv.is_empty() {
        return Err(Error::msg("container command is empty"));
    }

    let engine = pick_container_engine()?;
    ensure_container_image(ctx, ws, &engine, image, envs)?;

    let cwd_abs = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        ws.root.join(cwd)
    };
    if !cwd_abs.exists() {
        return Err(Error::msg(format!(
            "container working directory does not exist: {}",
            cwd_abs.display()
        )));
    }

    let mut cmd = Command::new(&engine);
    cmd.arg("run").arg("--rm").arg("--init");
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        cmd.arg("--user").arg(format!("{uid}:{gid}"));
    }

    let mut mounts = BTreeSet::<PathBuf>::new();
    mounts.insert(ws.root.clone());
    mounts.insert(cwd_abs.clone());
    if let Some(parent) = cwd_abs.parent() {
        mounts.insert(parent.to_path_buf());
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if home.is_dir() {
            mounts.insert(home.clone());
        }
        let cargo_home = home.join(".cargo");
        let rustup_home = home.join(".rustup");
        if cargo_home.is_dir() {
            mounts.insert(cargo_home);
        }
        if rustup_home.is_dir() {
            mounts.insert(rustup_home);
        }
    }
    for mount in mounts {
        cmd.arg("-v")
            .arg(format!("{}:{}", mount.display(), mount.display()));
    }
    cmd.arg("--workdir").arg(&cwd_abs);
    let host_home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = host_home.as_ref() {
        cmd.arg("-e").arg(format!("HOME={}", home.display()));
        cmd.arg("-e")
            .arg(format!("CARGO_HOME={}", home.join(".cargo").display()));
        cmd.arg("-e")
            .arg(format!("RUSTUP_HOME={}", home.join(".rustup").display()));
    } else {
        cmd.arg("-e").arg("HOME=/tmp");
    }
    cmd.arg("-e").arg("TERM=dumb");
    for (k, v) in envs {
        cmd.arg("-e").arg(format!("{k}={v}"));
    }
    cmd.arg(image);
    cmd.args(argv);
    ctx.log(&format!(
        "running in container image='{}' engine='{}' cwd={}",
        image,
        engine,
        cwd_abs.display()
    ));
    ctx.run_cmd(cmd)
}

fn pick_container_engine() -> Result<String> {
    if let Ok(pref) = std::env::var("GAIA_CONTAINER_ENGINE") {
        let pref = pref.trim();
        if pref.is_empty() {
            return Err(Error::msg("GAIA_CONTAINER_ENGINE is set but empty"));
        }
        if command_works(pref, &["--version"]) {
            return Ok(pref.to_string());
        }
        return Err(Error::msg(format!(
            "container engine '{}' from GAIA_CONTAINER_ENGINE is not available",
            pref
        )));
    }

    for candidate in ["docker", "podman"] {
        if command_works(candidate, &["--version"]) {
            return Ok(candidate.to_string());
        }
    }
    Err(Error::msg(
        "no container engine found (tried docker, podman); set GAIA_CONTAINER_ENGINE",
    ))
}

fn command_works(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn image_exists(engine: &str, image: &str) -> bool {
    Command::new(engine)
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ensure_container_image(
    ctx: &mut ExecCtx,
    ws: &WorkspacePaths,
    engine: &str,
    image: &str,
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    // Artifact-level parallel builds may race on the same image bootstrap.
    // Serialize "inspect/build" so we only attempt one build per missing image at a time.
    static IMAGE_BOOTSTRAP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = IMAGE_BOOTSTRAP_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| Error::msg("container image bootstrap lock poisoned"))?;

    if image_exists(engine, image) {
        return Ok(());
    }

    let dockerfile_raw = envs
        .get("DOCKERFILE")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::msg(format!(
                "container image '{}' is missing and profile env DOCKERFILE is not set",
                image
            ))
        })?;

    let dockerfile = ws.resolve_config_path(dockerfile_raw)?;
    if !dockerfile.is_file() {
        return Err(Error::msg(format!(
            "container image '{}' missing and dockerfile not found at {}",
            image,
            dockerfile.display()
        )));
    }

    let context = envs
        .get("DOCKER_CONTEXT")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| ws.resolve_config_path(raw))
        .transpose()?
        .unwrap_or_else(|| ws.root.clone());

    if !context.is_dir() {
        return Err(Error::msg(format!(
            "container image '{}' missing and docker context does not exist: {}",
            image,
            context.display()
        )));
    }

    ctx.log(&format!(
        "building missing container image '{}' using {}",
        image,
        dockerfile.display()
    ));
    let mut cmd = Command::new(engine);
    cmd.arg("build")
        .arg("-f")
        .arg(&dockerfile)
        .arg("-t")
        .arg(image);
    for (k, v) in envs {
        if k == "DOCKERFILE" || k == "DOCKER_CONTEXT" {
            continue;
        }
        cmd.arg("--build-arg").arg(format!("{k}={v}"));
    }
    cmd.arg(&context);
    ctx.run_cmd(cmd)
}

fn resolve_output_path(
    ws_root: &Path,
    cwd: &Path,
    workspace_dir: &Path,
    profile_target: Option<&str>,
    artifact: &RustArtifact,
    effective_env: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    if let Some(raw) = artifact.output_path.as_deref().map(str::trim)
        && !raw.is_empty()
    {
        return Ok(if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            ws_root.join(raw)
        });
    }

    let target_dir = effective_env
        .get("CARGO_TARGET_DIR")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| {
            if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                cwd.join(raw)
            }
        })
        .unwrap_or_else(|| workspace_dir.join("target"));

    infer_default_output(
        &target_dir,
        profile_target,
        artifact.cargo_profile.trim(),
        &artifact.kind,
        artifact.package.as_deref(),
    )
}

fn exec(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(TASK_ID);

    validate_program_definitions(doc)?;

    let cfg: RustBuildConfig = doc.deserialize_path("program.rust")?.unwrap_or_default();
    if !cfg.enabled {
        return Ok(());
    }

    let ws = ctx.workspace_paths_or_init(doc)?;
    let build_cfg = load_program_cfg(doc)?;
    let workspace_dir = resolve_from_workspace_root(&ws, &cfg.workspace_dir)?;
    let module_dir = util::module_dir(doc, ctx, "program.rust")?;
    util::ensure_dir(&module_dir)?;

    let mut manifest = Vec::new();
    let artifacts = selected_artifacts(doc, &cfg.artifacts)?;
    if artifacts.is_empty() {
        ctx.log("program.rust: no artifacts selected by conditions");
    }
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
    ws: &WorkspacePaths,
    build_cfg: &ProgramConfig,
    workspace_dir: &Path,
    default_check_ids: &[String],
    artifact: RustArtifact,
) -> Result<serde_json::Value> {
    let aid = artifact.id.trim();
    if aid.is_empty() {
        return Err(Error::msg("program.rust.artifacts[].id is empty"));
    }

    let profile = resolve_profile(build_cfg, artifact.profile.as_deref())?;
    let profile_target = profile.and_then(|p| p.target.as_deref());
    let profile_container_image = profile.and_then(|p| p.container_image.as_deref());
    let profile_env = profile.map(|p| &p.env).cloned().unwrap_or_default();
    let mut effective_env = profile_env.clone();
    for (k, v) in &artifact.env {
        effective_env.insert(k.clone(), v.clone());
    }

    let prebuilt = artifact
        .prebuilt_path
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
    let output = if matches!(artifact.mode, ArtifactMode::Prebuilt) {
        None
    } else {
        Some(resolve_output_path(
            &ws.root,
            &cwd,
            workspace_dir,
            profile_target,
            &artifact,
            &effective_env,
        )?)
    };

    let fingerprint = if matches!(artifact.mode, ArtifactMode::Prebuilt) {
        None
    } else {
        let fingerprint_payload = serde_json::json!({
            "builder": "program.rust",
            "id": aid,
            "mode": format!("{:?}", artifact.mode),
            "package": artifact.package.clone(),
            "kind": format!("{:?}", artifact.kind),
            "cargo_profile": artifact.cargo_profile.clone(),
            "profile": artifact.profile.clone(),
            "enabled_if": artifact.enabled_if.clone(),
            "disabled_if": artifact.disabled_if.clone(),
            "profile_target": profile_target,
            "profile_container_image": profile_container_image,
            "profile_env": profile_env.clone(),
            "artifact_env": artifact.env.clone(),
            "build_command": artifact.build_command.clone(),
            "cwd": cwd.display().to_string(),
            "output": output.as_ref().map(|x| x.display().to_string()),
            "check_ids": selected.clone(),
        });
        Some(compute_artifact_fingerprint(&fingerprint_payload, &cwd)?)
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
                    state.producer == "program.rust"
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
                output
                    .clone()
                    .ok_or_else(|| Error::msg(format!("artifact '{}' missing output path", aid)))?
            } else {
                run_checks(doc, ctx, "rust", workspace_dir, &selected)?;
                run_build_command(
                    doc,
                    ctx,
                    ws,
                    &cwd,
                    profile_container_image,
                    profile_target,
                    &artifact,
                    &profile_env,
                )?;
                output
                    .clone()
                    .ok_or_else(|| Error::msg(format!("artifact '{}' missing output path", aid)))?
            }
        }
        ArtifactMode::Build => {
            run_checks(doc, ctx, "rust", workspace_dir, &selected)?;
            run_build_command(
                doc,
                ctx,
                ws,
                &cwd,
                profile_container_image,
                profile_target,
                &artifact,
                &profile_env,
            )?;
            output
                .clone()
                .ok_or_else(|| Error::msg(format!("artifact '{}' missing output path", aid)))?
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
            producer: "program.rust".into(),
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
                producer: "program.rust".into(),
                fingerprint: fp,
                output_abs_path: abs.clone(),
                updated_at: now_rfc3339(),
            },
        )?;
    }

    ctx.log(&format!("artifact:{} => {}", aid, produced.display()));
    Ok(serde_json::json!({
        "id": aid,
        "kind": kind,
        "path": abs,
        "mode": format!("{:?}", artifact.mode),
    }))
}
