use serde::{Deserialize, Serialize};

use gaia_image_builder_macros::{Module, Task};

use crate::config::ConfigDoc;
use crate::executor::ExecCtx;
use crate::modules::stage::{StageConfig, StageUnitConfig};
use crate::workspace::WorkspacePaths;
use crate::{Error, Result};

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::{
    collections::BTreeSet,
    os::unix::fs::{PermissionsExt, symlink},
};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BuildrootConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub repo: String,
    pub version: Option<String>,

    // Legacy fields (pre-workspace). If present, they override workspace-based paths.
    pub build_dir: Option<String>,
    pub output_dir: Option<String>,

    // New workspace-relative fields.
    pub src_dir: Option<String>,
    pub br_output_dir: Option<String>,

    // Build behavior.
    pub performance_profile: Option<BuildrootPerformanceProfile>,
    pub defconfig: Option<String>,
    pub threads: Option<usize>,
    pub top_level_jobs: Option<usize>,
    pub top_level_load: Option<f64>,
    pub per_package_dirs: Option<bool>,
    pub use_ccache: Option<bool>,
    pub ccache_dir: Option<String>,
    pub compression: Option<String>,
    pub expand_size_mb: Option<u32>,
    pub collect_out_dir: Option<String>,
    pub collect_refresh_post_image: Option<bool>,
    pub shrink_ext: Option<bool>,
    pub archive_format: Option<String>,
    pub archive_mode: Option<BuildrootArchiveMode>,
    pub archive_name: Option<String>,
    pub report: Option<bool>,
    pub report_hashes: Option<bool>,
    pub download_dir: Option<String>,
    pub git_http_low_speed_limit: Option<u32>,
    pub git_http_low_speed_time: Option<u32>,
    pub git_http_version: Option<String>,
    pub external: Vec<String>,
    pub packages: BTreeMap<String, bool>,
    pub package_versions: BTreeMap<String, String>,
    pub symbols: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildrootPerformanceProfile {
    Max,
    Balanced,
    Safe,
}

impl Default for BuildrootPerformanceProfile {
    fn default() -> Self {
        Self::Max
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildrootArchiveMode {
    All,
    Image,
}

impl Default for BuildrootArchiveMode {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone, Copy)]
enum RawImageCompressor {
    Xz,
    Gzip,
    Zstd,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct BuildrootRpiRef {
    defconfig: Option<String>,
}

impl Default for BuildrootConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: "https://github.com/buildroot/buildroot.git".into(),
            version: None,
            build_dir: None,
            output_dir: None,
            src_dir: Some("buildroot/src".into()),
            br_output_dir: Some("buildroot/output".into()),
            performance_profile: Some(BuildrootPerformanceProfile::Max),
            defconfig: None,
            threads: None,
            top_level_jobs: None,
            top_level_load: None,
            per_package_dirs: None,
            use_ccache: Some(true),
            ccache_dir: None,
            compression: None,
            expand_size_mb: None,
            collect_out_dir: None,
            collect_refresh_post_image: Some(false),
            shrink_ext: Some(false),
            archive_format: Some("none".into()),
            archive_mode: Some(BuildrootArchiveMode::All),
            archive_name: None,
            report: Some(true),
            report_hashes: Some(true),
            download_dir: None,
            git_http_low_speed_limit: Some(1024),
            git_http_low_speed_time: Some(60),
            git_http_version: None,
            external: Vec::new(),
            packages: BTreeMap::new(),
            package_versions: BTreeMap::new(),
            symbols: BTreeMap::new(),
        }
    }
}

#[Task(
    id = "buildroot.fetch",
    module = "buildroot",
    phase = "fetch",
    provides = ["buildroot:source"],
    after = ["core.init", "buildroot:target-prepared?"],
    default_label = "Fetch Buildroot",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FetchTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for FetchTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl FetchTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let ws_paths = ctx.workspace_paths_or_init(doc)?;
        let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
        let src_dir = resolve_buildroot_src_dir(&ws_paths, &br)?;
        let git_env = resolve_buildroot_git_env(&br);

        ctx.log(&format!("buildroot.repo = {}", br.repo));
        if let Some(v) = br.version.as_ref().filter(|s| !s.trim().is_empty()) {
            ctx.log(&format!("buildroot.version = {v}"));
        }
        ctx.log(&format!("buildroot.src_dir = {}", src_dir.display()));
        if let (Some(limit), Some(time)) = (
            git_env.get("GIT_HTTP_LOW_SPEED_LIMIT"),
            git_env.get("GIT_HTTP_LOW_SPEED_TIME"),
        ) {
            ctx.log(&format!(
                "git transfer stall guard: {limit} B/s for {time}s"
            ));
        }
        if let Some(http_version) = git_env.get("GIT_HTTP_VERSION") {
            ctx.log(&format!("git http version override: {http_version}"));
        }

        if !src_dir.exists() {
            if let Some(parent) = src_dir.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Error::msg(format!(
                        "failed to create buildroot parent dir {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            ctx.log("cloning buildroot repo...");
            let mut cmd = Command::new("git");
            cmd.arg("clone").arg(&br.repo).arg(&src_dir);
            apply_command_env(&mut cmd, &git_env);
            ctx.run_cmd(cmd)?;
        } else if !src_dir.join(".git").exists() {
            return Err(Error::msg(format!(
                "buildroot dir exists but is not a git repo: {}",
                src_dir.display()
            )));
        }

        ctx.log("fetching updates...");
        let mut fetch = Command::new("git");
        fetch
            .arg("-C")
            .arg(&src_dir)
            .arg("fetch")
            .arg("--tags")
            .arg("--prune");
        apply_command_env(&mut fetch, &git_env);
        ctx.run_cmd(fetch)?;

        if let Some(v) = br.version.as_ref().filter(|s| !s.trim().is_empty()) {
            let want = git_rev_parse(&src_dir, &format!("{v}^{{commit}}"))?;
            let current = git_rev_parse(&src_dir, "HEAD")?;
            if current == want {
                ctx.log(&format!("already at buildroot.version '{v}' ({want})"));
            } else {
                ctx.log(&format!("checking out {v} ({want})..."));
                let mut co = Command::new("git");
                co.arg("-C")
                    .arg(&src_dir)
                    .arg("checkout")
                    .arg("--force")
                    .arg(v);
                apply_command_env(&mut co, &git_env);
                ctx.run_cmd(co)?;

                let mut reset = Command::new("git");
                reset
                    .arg("-C")
                    .arg(&src_dir)
                    .arg("reset")
                    .arg("--hard")
                    .arg(&want);
                apply_command_env(&mut reset, &git_env);
                ctx.run_cmd(reset)?;
            }
        }
        Ok(())
    }
}

fn git_rev_parse(repo: &Path, rev: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--verify")
        .arg(rev)
        .output()
        .map_err(|e| {
            Error::msg(format!(
                "failed to run git rev-parse in {}: {e}",
                repo.display()
            ))
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::msg(format!(
            "git rev-parse '{}' failed in {}: {}",
            rev,
            repo.display(),
            stderr.trim()
        )));
    }
    let parsed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if parsed.is_empty() {
        return Err(Error::msg(format!(
            "git rev-parse '{}' returned empty output in {}",
            rev,
            repo.display()
        )));
    }
    Ok(parsed)
}

#[Task(
    id = "buildroot.configure",
    module = "buildroot",
    phase = "configure",
    provides = ["buildroot:config"],
    after = ["buildroot.fetch", "buildroot:target-prepared?"],
    default_label = "Configure Buildroot",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ConfigureTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for ConfigureTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl ConfigureTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let ws = ctx.workspace_paths_or_init(doc)?;
        let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
        let perf = resolve_performance_settings(&br);
        let src_dir = resolve_buildroot_src_dir(&ws, &br)?;
        let out_dir = resolve_buildroot_out_dir(&ws, &src_dir, &br)?;
        let make_common_env = resolve_buildroot_common_env(&ws, &br, &src_dir)?;
        let tweaked_codegen_pkgs = apply_buildroot_codegen_speed_tweaks(ctx, &src_dir)?;
        if let Some(externals) = make_common_env.get("BR2_EXTERNAL") {
            ctx.log(&format!("buildroot external trees: {externals}"));
        }

        let br_rpi: BuildrootRpiRef = doc.deserialize_path("buildroot.rpi")?.unwrap_or_default();
        let defconfig = br
            .defconfig
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                br_rpi
                    .defconfig
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .ok_or_else(|| {
                Error::msg("buildroot.defconfig (or buildroot.rpi.defconfig) is required")
            })?;

        fs::create_dir_all(&out_dir)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", out_dir.display())))?;

        run_make(ctx, &src_dir, &out_dir, &[], &[defconfig], &make_common_env)?;
        run_make(
            ctx,
            &src_dir,
            &out_dir,
            &[],
            &["olddefconfig"],
            &make_common_env,
        )?;

        let stage_root = crate::modules::util::stage_root_dir(doc, ctx)?;
        fs::create_dir_all(&stage_root).map_err(|e| {
            Error::msg(format!(
                "failed to create stage root {}: {e}",
                stage_root.display()
            ))
        })?;

        let cfg_path = out_dir.join(".config");
        let mut kcfg = fs::read_to_string(&cfg_path)
            .map_err(|e| Error::msg(format!("failed to read {}: {e}", cfg_path.display())))?;

        set_kv(
            &mut kcfg,
            "BR2_ROOTFS_OVERLAY",
            &format!("\"{}\"", stage_root.display()),
        );
        // Default to non-forced hash mode; callers can still opt-in via buildroot.symbols.
        unset_kv(&mut kcfg, "BR2_DOWNLOAD_FORCE_CHECK_HASHES");

        let dl_dir = br
            .download_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|p| ws.resolve_config_path(p))
            .transpose()?
            .unwrap_or_else(|| ws.build_dir.join(".cache/buildroot/dl"));
        fs::create_dir_all(&dl_dir)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", dl_dir.display())))?;
        set_kv(
            &mut kcfg,
            "BR2_DL_DIR",
            &format!("\"{}\"", dl_dir.display()),
        );

        if perf.use_ccache {
            let ccache_dir = br
                .ccache_dir
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|p| resolve_root_or_abs(&ws, p))
                .transpose()?
                .unwrap_or_else(|| ws.build_dir.join(".ccache"));
            fs::create_dir_all(&ccache_dir).map_err(|e| {
                Error::msg(format!("failed to create {}: {e}", ccache_dir.display()))
            })?;
            set_kv(&mut kcfg, "BR2_CCACHE", "y");
            set_kv(
                &mut kcfg,
                "BR2_CCACHE_DIR",
                &format!("\"{}\"", ccache_dir.display()),
            );
            set_kv(&mut kcfg, "BR2_CCACHE_USE_BASEDIR", "y");
        } else {
            unset_kv(&mut kcfg, "BR2_CCACHE");
            unset_kv(&mut kcfg, "BR2_CCACHE_DIR");
            unset_kv(&mut kcfg, "BR2_CCACHE_USE_BASEDIR");
        }

        if perf.per_package_dirs {
            set_kv(&mut kcfg, "BR2_PER_PACKAGE_DIRECTORIES", "y");
        } else {
            unset_kv(&mut kcfg, "BR2_PER_PACKAGE_DIRECTORIES");
        }

        if let Some(j) = perf.threads {
            set_kv(&mut kcfg, "BR2_JLEVEL", &j.to_string());
        }

        if let Some(size_mb) = br.expand_size_mb
            && size_mb > 0
        {
            set_kv(
                &mut kcfg,
                "BR2_TARGET_ROOTFS_EXT2_SIZE",
                &format!("\"{}M\"", size_mb),
            );
        }

        apply_compression(&mut kcfg, br.compression.as_deref());
        let expected = apply_buildroot_symbol_overrides(&mut kcfg, &br)?;

        fs::write(&cfg_path, &kcfg)
            .map_err(|e| Error::msg(format!("failed to write {}: {e}", cfg_path.display())))?;

        run_make(
            ctx,
            &src_dir,
            &out_dir,
            &[],
            &["olddefconfig"],
            &make_common_env,
        )?;
        let final_kcfg = fs::read_to_string(&cfg_path)
            .map_err(|e| Error::msg(format!("failed to read {}: {e}", cfg_path.display())))?;
        log_symbol_validation(ctx, &expected, &final_kcfg);
        maybe_refresh_provisioning_tool_packages(
            ctx,
            &src_dir,
            &out_dir,
            &make_common_env,
            &final_kcfg,
        )?;
        for tweak in tweaked_codegen_pkgs {
            if tweak.requires_clean {
                let target = format!("{}-dirclean", tweak.pkg);
                ctx.log(&format!(
                    "refreshing {} build dir after introducing speed tweak",
                    tweak.pkg
                ));
                run_make(
                    ctx,
                    &src_dir,
                    &out_dir,
                    &[],
                    &[target.as_str()],
                    &make_common_env,
                )?;
            } else if tweak.changed {
                ctx.log(&format!(
                    "speed tweak formatting updated for {}; skipping dirclean",
                    tweak.pkg
                ));
            }
        }

        let gaia_run_dir = ws.out_dir.join(build_name(doc)).join("gaia");
        fs::create_dir_all(&gaia_run_dir).map_err(|e| {
            Error::msg(format!(
                "failed to create gaia run dir {}: {e}",
                gaia_run_dir.display()
            ))
        })?;

        let resolved =
            toml::to_string_pretty(&doc.value).unwrap_or_else(|_| format!("{:?}", doc.value));
        fs::write(gaia_run_dir.join("resolved.toml"), resolved)
            .map_err(|e| Error::msg(format!("failed to write resolved.toml: {e}")))?;
        fs::write(
            gaia_run_dir.join("configure.marker"),
            format!("configured_at={}\n", chrono::Utc::now()),
        )
        .map_err(|e| Error::msg(format!("failed to write configure.marker: {e}")))?;

        ctx.log(&format!(
            "configured buildroot: src={} O={}",
            src_dir.display(),
            out_dir.display()
        ));
        ctx.log(&format!(
            "performance profile='{}' threads={} top_level_jobs={} top_level_load={} per_package_dirs={} ccache={}",
            perf.profile_name,
            perf.threads.map(|v| v.to_string()).unwrap_or_else(|| "unset".into()),
            perf.top_level_jobs.map(|v| v.to_string()).unwrap_or_else(|| "unset".into()),
            perf.top_level_load.map(|v| format!("{v:.1}")).unwrap_or_else(|| "unset".into()),
            perf.per_package_dirs,
            perf.use_ccache
        ));
        Ok(())
    }
}

#[Task(
    id = "buildroot.build",
    module = "buildroot",
    phase = "build",
    provides = ["buildroot:artifacts"],
    after = ["buildroot.configure"],
    default_label = "Build",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BuildTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for BuildTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl BuildTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let ws = ctx.workspace_paths_or_init(doc)?;
        let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
        let perf = resolve_performance_settings(&br);
        let src_dir = resolve_buildroot_src_dir(&ws, &br)?;
        let out_dir = resolve_buildroot_out_dir(&ws, &src_dir, &br)?;

        let mut envs = resolve_buildroot_common_env(&ws, &br, &src_dir)?;
        if let Some(j) = perf.threads {
            envs.insert("BR2_JLEVEL".into(), j.to_string());
        }

        apply_external_change_cleanup(ctx, &ws, &br, &src_dir, &out_dir, &envs)?;
        apply_kernel_change_cleanup(ctx, &src_dir, &out_dir, &envs)?;

        let mut make_opts = Vec::<String>::new();
        if let Some(jobs) = perf.top_level_jobs {
            make_opts.push(format!("-j{jobs}"));
        }
        if let Some(load) = perf.top_level_load {
            make_opts.push(format!("-l{load}"));
        }
        let make_opts_ref = make_opts.iter().map(String::as_str).collect::<Vec<_>>();

        // Build all configured packages and finalize host/staging, but defer
        // rootfs/image generation to collect (after stage:done).
        run_make(
            ctx,
            &src_dir,
            &out_dir,
            &make_opts_ref,
            &["host-finalize"],
            &envs,
        )?;

        let run_dir = ws.out_dir.join(build_name(doc)).join("gaia");
        fs::create_dir_all(&run_dir)
            .map_err(|e| Error::msg(format!("failed to create run dir: {e}")))?;
        fs::write(
            run_dir.join("post-image-needed.marker"),
            format!("requested_at={}\n", chrono::Utc::now()),
        )
        .map_err(|e| Error::msg(format!("failed to write post-image-needed.marker: {e}")))?;
        fs::write(
            run_dir.join("build.marker"),
            format!("built_at={}\n", chrono::Utc::now()),
        )
        .map_err(|e| Error::msg(format!("failed to write build.marker: {e}")))?;
        Ok(())
    }
}

#[Task(
    id = "buildroot.collect",
    module = "buildroot",
    phase = "collect",
    provides = ["artifacts:rootfs"],
    after = ["buildroot.build", "stage:done?"],
    default_label = "Collect artifacts",
    core = true
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CollectTask {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub label: Option<String>,
}

impl Default for CollectTask {
    fn default() -> Self {
        Self {
            enabled: true,
            label: None,
        }
    }
}

impl CollectTask {
    pub fn run(_cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
        let ws = ctx.workspace_paths_or_init(doc)?;
        let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
        let perf = resolve_performance_settings(&br);
        let report_enabled = br.report.unwrap_or(true);
        let report_hashes = br.report_hashes.unwrap_or(true);
        let shrink_ext = br.shrink_ext.unwrap_or(false);
        let refresh_post_image = br.collect_refresh_post_image.unwrap_or(false);
        let src_dir = resolve_buildroot_src_dir(&ws, &br)?;
        let out_dir = resolve_buildroot_out_dir(&ws, &src_dir, &br)?;

        let images_dir = out_dir.join("images");
        let run_dir = ws.out_dir.join(build_name(doc)).join("gaia");
        let post_image_marker = run_dir.join("post-image-needed.marker");
        let post_image_needed = post_image_marker.is_file();
        if refresh_post_image || post_image_needed || !images_dir.is_dir() {
            let stage_root = crate::modules::util::stage_root_dir(doc, ctx)?;
            sync_stage_overlay_into_buildroot_target(doc, ctx, &out_dir, &stage_root)?;

            // Buildroot image generation intentionally happens here (after stage:done)
            // so package builds and stage/program work can overlap.
            let mut envs = resolve_buildroot_common_env(&ws, &br, &src_dir)?;
            if let Some(j) = perf.threads {
                envs.insert("BR2_JLEVEL".into(), j.to_string());
            }
            let mut make_opts = Vec::<String>::new();
            if let Some(jobs) = perf.top_level_jobs {
                make_opts.push(format!("-j{jobs}"));
            }
            if let Some(load) = perf.top_level_load {
                make_opts.push(format!("-l{load}"));
            }
            let make_opts_ref = make_opts.iter().map(String::as_str).collect::<Vec<_>>();
            if refresh_post_image {
                ctx.log("refreshing buildroot post-image outputs...");
            } else if post_image_needed {
                ctx.log("running buildroot target-post-image after stage completion...");
            } else {
                ctx.log("buildroot images missing; running one-time post-image generation...");
            }
            run_make(
                ctx,
                &src_dir,
                &out_dir,
                &make_opts_ref,
                &["target-post-image"],
                &envs,
            )?;
            if post_image_needed {
                fs::remove_file(&post_image_marker).map_err(|e| {
                    Error::msg(format!(
                        "failed to remove {}: {e}",
                        post_image_marker.display()
                    ))
                })?;
            }
        } else {
            ctx.log("skipping post-image refresh (buildroot.collect_refresh_post_image=false)");
        }
        if !images_dir.is_dir() {
            return Err(Error::msg(format!(
                "buildroot images dir not found: {}",
                images_dir.display()
            )));
        }

        let collect_dir = resolve_collect_dir(doc, &ws, &br)?;
        if paths_equivalent(&collect_dir, &images_dir) {
            return Err(Error::msg(format!(
                "buildroot.collect_out_dir resolves to the buildroot images dir; refusing to clean source dir: {}",
                collect_dir.display()
            )));
        }
        let removed_stale = clear_directory_contents(&collect_dir)?;
        fs::create_dir_all(&collect_dir)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", collect_dir.display())))?;
        if removed_stale > 0 {
            ctx.log(&format!(
                "cleared {} stale artifact entr{} from {}",
                removed_stale,
                if removed_stale == 1 { "y" } else { "ies" },
                collect_dir.display()
            ));
        }

        let mut copied_paths = Vec::<PathBuf>::new();
        let mut copied = Vec::new();
        for entry in walkdir::WalkDir::new(&images_dir) {
            let entry = entry.map_err(|e| Error::msg(format!("walkdir error: {e}")))?;
            let p = entry.path();
            let rel = p
                .strip_prefix(&images_dir)
                .map_err(|e| Error::msg(format!("strip_prefix failed: {e}")))?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            let dst = collect_dir.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dst)
                    .map_err(|e| Error::msg(format!("failed to create {}: {e}", dst.display())))?;
                continue;
            }
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Error::msg(format!("failed to create {}: {e}", parent.display()))
                })?;
            }
            fs::copy(p, &dst).map_err(|e| {
                Error::msg(format!(
                    "failed to copy {} -> {}: {e}",
                    p.display(),
                    dst.display()
                ))
            })?;
            copied_paths.push(dst.clone());

            copied.push(serde_json::json!({
                "src": p.display().to_string(),
                "dst": dst.display().to_string(),
                "rel": rel.display().to_string(),
            }));
        }

        let mut shrink_ops = Vec::new();
        if shrink_ext {
            for path in &copied_paths {
                if !is_ext_rootfs(path) {
                    continue;
                }
                let before = file_len(path)?;
                shrink_ext_image(ctx, path)?;
                let after = file_len(path)?;
                ctx.log(&format!(
                    "shrink ext image: {} ({} -> {} bytes)",
                    path.display(),
                    before,
                    after
                ));
                shrink_ops.push(serde_json::json!({
                    "path": path.display().to_string(),
                    "before_bytes": before,
                    "after_bytes": after,
                }));
            }
        }

        let rootfs = ["rootfs.ext4", "rootfs.ext3", "rootfs.ext2"]
            .into_iter()
            .map(|n| collect_dir.join(n))
            .find(|p| p.exists())
            .map(|p| p.display().to_string());

        let mut report_entries = Vec::new();
        let mut total_bytes = 0u64;
        for path in &copied_paths {
            let rel = path
                .strip_prefix(&collect_dir)
                .map_err(|e| Error::msg(format!("strip_prefix failed: {e}")))?;
            let bytes = file_len(path)?;
            total_bytes = total_bytes.saturating_add(bytes);
            let sha256 = if report_hashes {
                Some(sha256_file_hex(path)?)
            } else {
                None
            };
            report_entries.push(serde_json::json!({
                "path": path.display().to_string(),
                "rel": rel.display().to_string(),
                "bytes": bytes,
                "sha256": sha256,
            }));
        }
        report_entries.sort_by(|a, b| {
            a.get("rel")
                .and_then(|v| v.as_str())
                .cmp(&b.get("rel").and_then(|v| v.as_str()))
        });

        let archive = create_collect_archive(doc, ctx, &collect_dir, &br)?;
        let archive_sha256 = if report_hashes {
            archive.as_ref().map(|p| sha256_file_hex(p)).transpose()?
        } else {
            None
        };

        let manifest = serde_json::json!({
            "build": build_name(doc),
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "images_dir": collect_dir.display().to_string(),
            "rootfs": rootfs,
            "total_bytes": total_bytes,
            "archive": archive.as_ref().map(|p| p.display().to_string()),
            "archive_sha256": archive_sha256,
            "shrink_ext": shrink_ext,
            "shrink_ops": shrink_ops,
            "artifacts": copied,
        });
        fs::write(
            run_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".into()),
        )
        .map_err(|e| Error::msg(format!("failed to write manifest.json: {e}")))?;
        ctx.log(&format!(
            "wrote {}",
            run_dir.join("manifest.json").display()
        ));

        if report_enabled {
            let report = serde_json::json!({
                "build": build_name(doc),
                "generated_at": chrono::Utc::now().to_rfc3339(),
                "images_dir": collect_dir.display().to_string(),
                "rootfs": rootfs,
                "total_bytes": total_bytes,
                "hashes": report_hashes,
                "archive": archive.as_ref().map(|p| p.display().to_string()),
                "archive_sha256": archive_sha256,
                "artifacts": report_entries,
            });
            fs::write(
                run_dir.join("image-report.json"),
                serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into()),
            )
            .map_err(|e| Error::msg(format!("failed to write image-report.json: {e}")))?;
            ctx.log(&format!(
                "wrote {}",
                run_dir.join("image-report.json").display()
            ));
        }
        Ok(())
    }
}

#[Module(
    id = "buildroot",
    config = BuildrootConfig,
    config_path = "buildroot",
    tasks = [FetchTask, ConfigureTask, BuildTask, CollectTask]
)]
pub struct BuildrootModule;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StageOverlaySyncManifest {
    entries: Vec<String>,
}

fn sync_stage_overlay_into_buildroot_target(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    out_dir: &Path,
    stage_root: &Path,
) -> Result<()> {
    if !stage_root.is_dir() {
        ctx.log(&format!(
            "stage overlay root missing (skipping buildroot target sync): {}",
            stage_root.display()
        ));
        return Ok(());
    }

    let target_root = out_dir.join("target");
    if !target_root.is_dir() {
        ctx.log(&format!(
            "buildroot target dir missing (skipping stage sync): {}",
            target_root.display()
        ));
        return Ok(());
    }

    let sync_manifest_path = out_dir.join(".gaia-stage-overlay-sync.json");
    let previous_entries = fs::read_to_string(&sync_manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<StageOverlaySyncManifest>(&raw).ok())
        .map(|m| m.entries)
        .unwrap_or_default();

    let mut removed_prev = 0usize;
    for raw in previous_entries {
        let rel = match parse_overlay_rel_path(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let candidate = target_root.join(rel);
        if remove_path_if_exists(&candidate)? {
            removed_prev += 1;
        }
    }

    let removed_service_paths = purge_configured_stage_service_paths(doc, &target_root)?;
    copy_dir_all_preserve_links(stage_root, &target_root)?;

    let entries = collect_overlay_entries(stage_root)?;
    let sync_manifest = StageOverlaySyncManifest {
        entries: entries.clone(),
    };
    fs::write(
        &sync_manifest_path,
        serde_json::to_string_pretty(&sync_manifest)
            .map_err(|e| Error::msg(format!("failed to encode stage sync manifest: {e}")))?,
    )
    .map_err(|e| {
        Error::msg(format!(
            "failed to write stage sync manifest {}: {e}",
            sync_manifest_path.display()
        ))
    })?;

    ctx.log(&format!(
        "synced stage overlay into buildroot target: entries={} removed_previous={} removed_service_paths={}",
        entries.len(),
        removed_prev,
        removed_service_paths
    ));
    Ok(())
}

fn purge_configured_stage_service_paths(doc: &ConfigDoc, target_root: &Path) -> Result<usize> {
    let stage_cfg: StageConfig = doc.deserialize_path("stage")?.unwrap_or_default();
    if !stage_cfg.enabled || !stage_cfg.services.enabled {
        return Ok(0);
    }

    let mut removed = 0usize;
    let systemd_root = target_root.join("etc/systemd/system");
    for (name, unit) in &stage_cfg.services.units {
        let unit_name = infer_stage_unit_name(name, unit)?;
        if remove_path_if_exists(&systemd_root.join(&unit_name))? {
            removed += 1;
        }
        for target in &unit.targets {
            let target_unit = normalize_stage_target_unit(target)?;
            let link = systemd_root
                .join(format!("{target_unit}.wants"))
                .join(&unit_name);
            if remove_path_if_exists(&link)? {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn infer_stage_unit_name(name: &str, unit: &StageUnitConfig) -> Result<String> {
    if let Some(explicit) = unit
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return validate_stage_unit_file_name(explicit);
    }

    if let Some(src) = unit.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let base = Path::new(src)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::msg(format!("invalid unit src path '{}'", src)))?;
        return validate_stage_unit_file_name(base);
    }

    if name.contains('.') {
        return validate_stage_unit_file_name(name);
    }
    validate_stage_unit_file_name(&format!("{name}.service"))
}

fn validate_stage_unit_file_name(name: &str) -> Result<String> {
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

fn normalize_stage_target_unit(target: &str) -> Result<String> {
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

fn collect_overlay_entries(stage_root: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::<String>::new();
    for entry in walkdir::WalkDir::new(stage_root) {
        let entry = entry.map_err(|e| Error::msg(format!("walkdir error: {e}")))?;
        let p = entry.path();
        let rel = p
            .strip_prefix(stage_root)
            .map_err(|e| Error::msg(format!("strip_prefix failed: {e}")))?;
        if rel.as_os_str().is_empty() || entry.file_type().is_dir() {
            continue;
        }
        entries.push(path_to_rel_string(rel)?);
    }
    entries.sort();
    entries.dedup();
    Ok(entries)
}

fn copy_dir_all_preserve_links(src: &Path, dst: &Path) -> Result<()> {
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

fn parse_overlay_rel_path(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::msg("overlay manifest path is empty"));
    }
    let rel = PathBuf::from(trimmed);
    if rel.is_absolute() {
        return Err(Error::msg(format!(
            "overlay manifest path must be relative: {}",
            trimmed
        )));
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::msg(format!(
            "overlay manifest path contains '..': {}",
            trimmed
        )));
    }
    Ok(rel)
}

fn path_to_rel_string(rel: &Path) -> Result<String> {
    if rel.is_absolute() {
        return Err(Error::msg(format!(
            "expected relative path, got {}",
            rel.display()
        )));
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::msg(format!(
            "relative path contains '..': {}",
            rel.display()
        )));
    }
    let s = rel
        .to_str()
        .ok_or_else(|| Error::msg(format!("path is not valid UTF-8: {}", rel.display())))?;
    Ok(s.replace('\\', "/"))
}

fn remove_path_if_exists(path: &Path) -> Result<bool> {
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
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::msg(format!(
            "failed to inspect {} before cleanup: {e}",
            path.display()
        ))),
    }
}

fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn clear_directory_contents(dir: &Path) -> Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    if !dir.is_dir() {
        return Err(Error::msg(format!(
            "collect output path is not a directory: {}",
            dir.display()
        )));
    }

    let mut removed = 0usize;
    for entry in fs::read_dir(dir)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", dir.display())))?
    {
        let entry = entry.map_err(|e| Error::msg(format!("read_dir entry error: {e}")))?;
        if remove_path_if_exists(&entry.path())? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn cleanup_stale_archive_staging(parent: &Path) -> Result<usize> {
    if !parent.is_dir() {
        return Ok(0);
    }

    let mut removed = 0usize;
    for entry in fs::read_dir(parent)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", parent.display())))?
    {
        let entry = entry.map_err(|e| Error::msg(format!("read_dir entry error: {e}")))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".gaia-archive-") {
            continue;
        }
        if remove_path_if_exists(&entry.path())? {
            removed += 1;
        }
    }
    Ok(removed)
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
    copy_file(src, dst)
}

fn build_name(doc: &ConfigDoc) -> String {
    crate::modules::util::build_name(doc)
}

fn resolve_buildroot_src_dir(ws: &WorkspacePaths, br: &BuildrootConfig) -> Result<PathBuf> {
    if let Some(p) = br.build_dir.as_ref().filter(|s| !s.trim().is_empty()) {
        return resolve_root_or_abs(ws, p);
    }
    let rel = br.src_dir.as_deref().unwrap_or("buildroot/src");
    if rel.trim_start().starts_with('@') || Path::new(rel).is_absolute() {
        ws.resolve_config_path(rel)
    } else {
        ws.resolve_under_build(rel)
    }
}

fn resolve_buildroot_out_dir(
    ws: &WorkspacePaths,
    src_dir: &Path,
    br: &BuildrootConfig,
) -> Result<PathBuf> {
    if let Some(p) = br.output_dir.as_ref().filter(|s| !s.trim().is_empty()) {
        let trimmed = p.trim();
        if trimmed.starts_with('@') {
            return ws.resolve_config_path(trimmed);
        }
        let pb = PathBuf::from(trimmed);
        if pb.is_absolute() {
            return Ok(pb);
        }
        return Ok(src_dir.join(pb));
    }
    let rel = br.br_output_dir.as_deref().unwrap_or("buildroot/output");
    if rel.trim_start().starts_with('@') || Path::new(rel).is_absolute() {
        ws.resolve_config_path(rel)
    } else {
        ws.resolve_under_build(rel)
    }
}

fn resolve_collect_dir(
    doc: &ConfigDoc,
    ws: &WorkspacePaths,
    br: &BuildrootConfig,
) -> Result<PathBuf> {
    if let Some(raw) = br.collect_out_dir.as_deref() {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let expanded = crate::modules::util::expand_build_template(doc, trimmed)?;
            return resolve_root_or_abs(ws, &expanded);
        }
    }
    Ok(ws.out_dir.join(build_name(doc)).join("gaia").join("images"))
}

fn create_collect_archive(
    doc: &ConfigDoc,
    ctx: &mut ExecCtx,
    collect_dir: &Path,
    br: &BuildrootConfig,
) -> Result<Option<PathBuf>> {
    let Some(raw_fmt) = br.archive_format.as_deref() else {
        return Ok(None);
    };
    let fmt = raw_fmt.trim().to_ascii_lowercase();
    if fmt.is_empty() || fmt == "none" || fmt == "off" {
        return Ok(None);
    }

    let parent = collect_dir.parent().ok_or_else(|| {
        Error::msg(format!(
            "cannot archive collect dir without parent: {}",
            collect_dir.display()
        ))
    })?;
    let name = collect_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            Error::msg(format!(
                "invalid collect dir name: {}",
                collect_dir.display()
            ))
        })?
        .to_string();

    enum ArchiveFormat {
        Tar {
            normalized_fmt: &'static str,
            ext: &'static str,
        },
        RawImage {
            compressor: Option<RawImageCompressor>,
            ext: &'static str,
        },
    }
    let archive_format = match fmt.as_str() {
        "tar" => ArchiveFormat::Tar {
            normalized_fmt: "tar",
            ext: ".tar",
        },
        "tar.gz" | "tgz" => ArchiveFormat::Tar {
            normalized_fmt: "tar.gz",
            ext: ".tar.gz",
        },
        "tar.xz" | "txz" => ArchiveFormat::Tar {
            normalized_fmt: "tar.xz",
            ext: ".tar.xz",
        },
        "tar.zst" | "tzst" => ArchiveFormat::Tar {
            normalized_fmt: "tar.zst",
            ext: ".tar.zst",
        },
        "img" => ArchiveFormat::RawImage {
            compressor: None,
            ext: ".img",
        },
        "img.xz" | "xz" => ArchiveFormat::RawImage {
            compressor: Some(RawImageCompressor::Xz),
            ext: ".img.xz",
        },
        "img.gz" | "gz" => ArchiveFormat::RawImage {
            compressor: Some(RawImageCompressor::Gzip),
            ext: ".img.gz",
        },
        "img.zst" | "zst" => ArchiveFormat::RawImage {
            compressor: Some(RawImageCompressor::Zstd),
            ext: ".img.zst",
        },
        other => {
            return Err(Error::msg(format!(
                "unsupported buildroot.archive_format '{}'; use one of: none, tar, tar.gz, tar.xz, tar.zst, img, img.xz, img.gz, img.zst",
                other
            )));
        }
    };
    let archive_mode = br.archive_mode.clone().unwrap_or_default();

    if matches!(archive_format, ArchiveFormat::RawImage { .. })
        && !matches!(archive_mode, BuildrootArchiveMode::Image)
    {
        return Err(Error::msg(
            "buildroot.archive_format with img* requires buildroot.archive_mode='image'",
        ));
    }

    let base = br
        .archive_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crate::modules::util::expand_build_template(doc, s))
        .transpose()?
        .unwrap_or_else(|| match archive_format {
            ArchiveFormat::Tar { .. } => "images".to_string(),
            ArchiveFormat::RawImage { .. } => "sdcard".to_string(),
        });
    let ext = match archive_format {
        ArchiveFormat::Tar { ext, .. } => ext,
        ArchiveFormat::RawImage { ext, .. } => ext,
    };
    let archive_file = if base.ends_with(ext) {
        base.to_string()
    } else {
        format!("{base}{ext}")
    };
    let archive_path = parent.join(archive_file);
    if archive_path.exists() {
        fs::remove_file(&archive_path).map_err(|e| {
            Error::msg(format!(
                "failed to remove existing archive {}: {e}",
                archive_path.display()
            ))
        })?;
    }

    match archive_format {
        ArchiveFormat::Tar { normalized_fmt, .. } => {
            let mut cmd = Command::new("tar");
            match archive_mode {
                BuildrootArchiveMode::All => {
                    cmd.arg("-C").arg(parent);
                }
                BuildrootArchiveMode::Image => {
                    cmd.arg("-C").arg(collect_dir);
                }
            }
            match normalized_fmt {
                "tar" => {
                    cmd.arg("-cf");
                }
                "tar.gz" => {
                    cmd.arg("-czf");
                }
                "tar.xz" => {
                    cmd.arg("-cJf");
                }
                "tar.zst" => {
                    cmd.arg("--zstd").arg("-cf");
                }
                _ => unreachable!(),
            }
            cmd.arg(&archive_path);
            match archive_mode {
                BuildrootArchiveMode::All => {
                    cmd.arg(&name);
                }
                BuildrootArchiveMode::Image => {
                    let rel = resolve_primary_collect_img_rel(collect_dir)?;
                    ctx.log(&format!("archive_mode=image using {}", rel.display()));
                    cmd.arg(rel);
                }
            }
            ctx.run_cmd(cmd)?;
        }
        ArchiveFormat::RawImage { compressor, .. } => {
            let rel = resolve_primary_collect_img_rel(collect_dir)?;
            ctx.log(&format!("archive_mode=image using {}", rel.display()));
            let src = collect_dir.join(rel);
            match compressor {
                None => {
                    fs::copy(&src, &archive_path).map_err(|e| {
                        Error::msg(format!(
                            "failed to copy {} -> {}: {e}",
                            src.display(),
                            archive_path.display()
                        ))
                    })?;
                }
                Some(tool) => {
                    compress_raw_image_archive(ctx, &src, &archive_path, tool)?;
                }
            }
        }
    }
    ctx.log(&format!("wrote archive {}", archive_path.display()));
    Ok(Some(archive_path))
}

fn compress_raw_image_archive(
    ctx: &mut ExecCtx,
    src: &Path,
    dst: &Path,
    compressor: RawImageCompressor,
) -> Result<()> {
    let parent = dst
        .parent()
        .ok_or_else(|| Error::msg(format!("invalid destination path: {}", dst.display())))?;
    fs::create_dir_all(parent)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
    let removed_stale = cleanup_stale_archive_staging(parent)?;
    if removed_stale > 0 {
        ctx.log(&format!(
            "removed {} stale archive staging entr{} from {}",
            removed_stale,
            if removed_stale == 1 { "y" } else { "ies" },
            parent.display()
        ));
    }

    let stamp = chrono::Utc::now().timestamp_millis();
    let tmp_img = parent.join(format!(
        ".gaia-archive-{}-{}.img",
        std::process::id(),
        stamp
    ));
    fs::copy(src, &tmp_img).map_err(|e| {
        Error::msg(format!(
            "failed to stage archive source {} -> {}: {e}",
            src.display(),
            tmp_img.display()
        ))
    })?;

    let (prog, args, suffix) = match compressor {
        RawImageCompressor::Xz => ("xz", vec!["-T0", "-f"], ".xz"),
        RawImageCompressor::Gzip => ("gzip", vec!["-f"], ".gz"),
        RawImageCompressor::Zstd => ("zstd", vec!["-f", "-q"], ".zst"),
    };
    let mut cmd = Command::new(prog);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg(&tmp_img);
    if let Err(e) = ctx.run_cmd(cmd) {
        let _ = fs::remove_file(&tmp_img);
        return Err(e);
    }

    let mut compressed = tmp_img.clone().into_os_string();
    compressed.push(suffix);
    let compressed = PathBuf::from(compressed);
    fs::rename(&compressed, dst).map_err(|e| {
        Error::msg(format!(
            "failed to move {} -> {}: {e}",
            compressed.display(),
            dst.display()
        ))
    })?;

    if tmp_img.exists() {
        let _ = fs::remove_file(tmp_img);
    }
    Ok(())
}

fn resolve_primary_collect_img_rel(collect_dir: &Path) -> Result<PathBuf> {
    let preferred = collect_dir.join("sdcard.img");
    if preferred.is_file() {
        return Ok(PathBuf::from("sdcard.img"));
    }

    let mut best: Option<(u64, PathBuf)> = None;
    for entry in walkdir::WalkDir::new(collect_dir) {
        let entry = entry.map_err(|e| Error::msg(format!("walkdir error: {e}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path();
        let is_img = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("img"))
            .unwrap_or(false);
        if !is_img {
            continue;
        }
        let rel = p
            .strip_prefix(collect_dir)
            .map_err(|e| Error::msg(format!("strip_prefix failed: {e}")))?
            .to_path_buf();
        let sz = fs::metadata(p)
            .map_err(|e| Error::msg(format!("failed to stat {}: {e}", p.display())))?
            .len();
        match &best {
            Some((cur, _)) if sz <= *cur => {}
            _ => best = Some((sz, rel)),
        }
    }

    best.map(|(_, rel)| rel).ok_or_else(|| {
        Error::msg(format!(
            "buildroot.archive_mode=image requested but no .img file was found in {}",
            collect_dir.display()
        ))
    })
}

fn file_len(path: &Path) -> Result<u64> {
    fs::metadata(path)
        .map_err(|e| Error::msg(format!("failed to stat {}: {e}", path.display())))
        .map(|m| m.len())
}

fn is_ext_rootfs(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| matches!(ext, "ext2" | "ext3" | "ext4"))
        .unwrap_or(false)
}

fn shrink_ext_image(ctx: &mut ExecCtx, path: &Path) -> Result<()> {
    let mut cmd = Command::new("resize2fs");
    cmd.arg("-M").arg(path);
    ctx.run_cmd(cmd)
}

fn sha256_file_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path)
        .map_err(|e| Error::msg(format!("failed to open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 256];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

fn run_make(
    ctx: &mut ExecCtx,
    src_dir: &Path,
    out_dir: &Path,
    make_opts: &[&str],
    args: &[&str],
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    let mut cmd = Command::new("make");
    cmd.arg("-C").arg(src_dir);
    cmd.arg(format!("O={}", out_dir.display()));
    for a in make_opts {
        cmd.arg(a);
    }
    for a in args {
        cmd.arg(a);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    ctx.run_cmd(cmd)
}

fn maybe_refresh_provisioning_tool_packages(
    ctx: &mut ExecCtx,
    src_dir: &Path,
    out_dir: &Path,
    envs: &BTreeMap<String, String>,
    kcfg: &str,
) -> Result<()> {
    if has_cfg_enabled(kcfg, "BR2_PACKAGE_UTIL_LINUX_BINARIES")
        && !target_binary_exists(out_dir, "blockdev")
    {
        ctx.log(
            "blockdev is missing in target while BR2_PACKAGE_UTIL_LINUX_BINARIES=y; forcing util-linux-dirclean",
        );
        run_make(ctx, src_dir, out_dir, &[], &["util-linux-dirclean"], envs)?;
    }

    if has_cfg_enabled(kcfg, "BR2_PACKAGE_E2FSPROGS_RESIZE2FS")
        && !target_binary_exists(out_dir, "resize2fs")
    {
        ctx.log(
            "resize2fs is missing in target while BR2_PACKAGE_E2FSPROGS_RESIZE2FS=y; forcing e2fsprogs-dirclean",
        );
        run_make(ctx, src_dir, out_dir, &[], &["e2fsprogs-dirclean"], envs)?;
    }
    Ok(())
}

fn target_binary_exists(out_dir: &Path, name: &str) -> bool {
    let target = out_dir.join("target");
    [
        target.join("usr/bin").join(name),
        target.join("bin").join(name),
        target.join("usr/sbin").join(name),
        target.join("sbin").join(name),
    ]
    .iter()
    .any(|p| p.exists())
}

fn resolve_root_or_abs(ws: &WorkspacePaths, raw: &str) -> Result<PathBuf> {
    ws.resolve_config_path(raw)
}

#[derive(Debug, Clone)]
struct EffectivePerformanceSettings {
    profile_name: &'static str,
    threads: Option<usize>,
    top_level_jobs: Option<usize>,
    top_level_load: Option<f64>,
    per_package_dirs: bool,
    use_ccache: bool,
}

#[derive(Debug, Clone)]
enum ExpectedSymbol {
    Enabled,
    Disabled,
    Exists,
}

fn resolve_performance_settings(br: &BuildrootConfig) -> EffectivePerformanceSettings {
    let cores = num_cpus::get().max(1);

    let profile = br.performance_profile.clone().unwrap_or_default();
    let (profile_name, default_threads, default_jobs, default_load, default_ppd, default_ccache) =
        match profile {
            BuildrootPerformanceProfile::Max => (
                "max",
                Some(cores),
                Some(scaled_jobs(cores, 2, 1)),
                None,
                false,
                true,
            ),
            BuildrootPerformanceProfile::Balanced => (
                "balanced",
                Some(scaled_jobs(cores, 3, 4)),
                Some(cores),
                Some((cores as f64 * 0.9).max(1.0)),
                true,
                true,
            ),
            BuildrootPerformanceProfile::Safe => (
                "safe",
                Some((cores / 3).max(1)),
                Some((cores / 2).max(1)),
                Some((cores as f64 * 0.7).max(1.0)),
                true,
                true,
            ),
        };

    let threads = br
        .threads
        .map(|v| normalize_jobs(v, cores))
        .or(default_threads);
    let top_level_jobs = br
        .top_level_jobs
        .map(|v| normalize_jobs(v, cores))
        .or(default_jobs);
    let top_level_load = br
        .top_level_load
        .filter(|v| *v > 0.0)
        .or(default_load.filter(|v| *v > 0.0));
    let per_package_dirs = br.per_package_dirs.unwrap_or(default_ppd);
    let use_ccache = br.use_ccache.unwrap_or(default_ccache);

    EffectivePerformanceSettings {
        profile_name,
        threads,
        top_level_jobs,
        top_level_load,
        per_package_dirs,
        use_ccache,
    }
}

fn normalize_jobs(jobs: usize, cores: usize) -> usize {
    if jobs == 0 { cores } else { jobs.max(1) }
}

fn scaled_jobs(cores: usize, numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return cores.max(1);
    }
    let scaled = cores
        .saturating_mul(numerator)
        .saturating_add(denominator - 1)
        / denominator;
    scaled.max(1)
}

fn resolve_buildroot_common_env(
    ws: &WorkspacePaths,
    br: &BuildrootConfig,
    buildroot_src_dir: &Path,
) -> Result<BTreeMap<String, String>> {
    let mut envs = BTreeMap::new();
    // Compatibility for existing board post-image scripts that reference repo assets.
    envs.insert("HELIOS_REPO_ROOT".into(), ws.root.display().to_string());
    envs.extend(resolve_buildroot_git_env(br));
    let external_paths = resolve_external_paths(ws, br, buildroot_src_dir)?;
    if !external_paths.is_empty() {
        let joined = external_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        envs.insert("BR2_EXTERNAL".into(), joined);
    }
    Ok(envs)
}

fn resolve_buildroot_git_env(br: &BuildrootConfig) -> BTreeMap<String, String> {
    let mut envs = BTreeMap::<String, String>::new();
    // Buildroot invokes git for VCS-backed package downloads; keep those calls
    // non-interactive and fail-fast when the transfer stalls.
    envs.insert("GIT_TERMINAL_PROMPT".into(), "0".into());

    match br.git_http_low_speed_limit {
        Some(0) => {}
        Some(limit) => {
            envs.insert("GIT_HTTP_LOW_SPEED_LIMIT".into(), limit.to_string());
        }
        None => {
            envs.insert("GIT_HTTP_LOW_SPEED_LIMIT".into(), "1024".into());
        }
    }
    match br.git_http_low_speed_time {
        Some(0) => {}
        Some(time) => {
            envs.insert("GIT_HTTP_LOW_SPEED_TIME".into(), time.to_string());
        }
        None => {
            envs.insert("GIT_HTTP_LOW_SPEED_TIME".into(), "60".into());
        }
    }

    if let Some(version) = br
        .git_http_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        envs.insert("GIT_HTTP_VERSION".into(), version.to_string());
    }
    envs
}

fn apply_command_env(cmd: &mut Command, envs: &BTreeMap<String, String>) {
    for (k, v) in envs {
        cmd.env(k, v);
    }
}

fn resolve_external_paths(
    ws: &WorkspacePaths,
    br: &BuildrootConfig,
    buildroot_src_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for (idx, raw) in br.external.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let src = resolve_root_or_abs(ws, trimmed)?;
        if !src.exists() {
            return Err(Error::msg(format!(
                "buildroot.external path not found: {}",
                src.display()
            )));
        }
        if is_valid_br2_external_tree(&src) {
            out.push(src);
            continue;
        }
        out.push(prepare_br2_external_wrapper(
            ws,
            &src,
            idx,
            buildroot_src_dir,
        )?);
    }
    Ok(out)
}

fn is_valid_br2_external_tree(p: &Path) -> bool {
    p.join("external.desc").is_file()
        && p.join("Config.in").is_file()
        && p.join("external.mk").is_file()
}

fn prepare_br2_external_wrapper(
    ws: &WorkspacePaths,
    src: &Path,
    idx: usize,
    buildroot_src_dir: &Path,
) -> Result<PathBuf> {
    #[cfg(not(unix))]
    {
        let _ = (ws, src, idx);
        return Err(Error::msg(
            "buildroot.external wrapper generation currently requires unix symlinks",
        ));
    }

    #[cfg(unix)]
    {
        let name = format!("GAIAEXT{}", idx);
        let wrap_dir = ws
            .build_dir
            .join("buildroot")
            .join("externals")
            .join(format!("gaiaext-{idx}"));
        if wrap_dir.exists() {
            fs::remove_dir_all(&wrap_dir).map_err(|e| {
                Error::msg(format!(
                    "failed to replace existing external wrapper {}: {e}",
                    wrap_dir.display()
                ))
            })?;
        }
        fs::create_dir_all(&wrap_dir).map_err(|e| {
            Error::msg(format!(
                "failed to create external wrapper {}: {e}",
                wrap_dir.display()
            ))
        })?;

        fs::write(
            wrap_dir.join("external.desc"),
            format!(
                "name: {name}\ndesc: Gaia-generated wrapper for {}\n",
                src.display()
            ),
        )
        .map_err(|e| Error::msg(format!("failed to write external.desc: {e}")))?;

        let package_src = src.join("packages");
        let package_dir = wrap_dir.join("package");
        fs::create_dir_all(&package_dir)
            .map_err(|e| Error::msg(format!("failed to create {}: {e}", package_dir.display())))?;

        let mut pkg_entries = BTreeSet::new();
        if package_src.is_dir() {
            for entry in fs::read_dir(&package_src)
                .map_err(|e| Error::msg(format!("failed to read {}: {e}", package_src.display())))?
            {
                let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let pkg_name = entry.file_name().to_string_lossy().to_string();
                if !p.join("Config.in").is_file() {
                    continue;
                }
                if buildroot_src_dir.join("package").join(&pkg_name).is_dir() {
                    // Same-name package in external tree intentionally overrides upstream package.
                    apply_package_override(buildroot_src_dir, &pkg_name, &p)?;
                    continue;
                }
                let dst = package_dir.join(&pkg_name);
                symlink(&p, &dst).map_err(|e| {
                    Error::msg(format!(
                        "failed to link package {} -> {}: {e}",
                        p.display(),
                        dst.display()
                    ))
                })?;
                pkg_entries.insert(pkg_name);
            }
        }

        let mut package_config = String::new();
        for pkg in pkg_entries {
            package_config.push_str(&format!(
                "source \"$BR2_EXTERNAL_{}_PATH/package/{pkg}/Config.in\"\n",
                name
            ));
        }
        fs::write(package_dir.join("Config.in"), package_config)
            .map_err(|e| Error::msg(format!("failed to write package/Config.in: {e}")))?;

        let linux_src = src.join("linux");
        if linux_src.is_dir() {
            symlink(&linux_src, wrap_dir.join("linux")).map_err(|e| {
                Error::msg(format!(
                    "failed to link linux dir {}: {e}",
                    linux_src.display()
                ))
            })?;
        }

        let board_src = src.join("board");
        if board_src.is_dir() {
            apply_board_overrides(buildroot_src_dir, &board_src)?;
        }
        let boards_src = src.join("boards");
        if boards_src.is_dir() {
            apply_board_overrides(buildroot_src_dir, &boards_src)?;
        }

        let cfg_in = format!("source \"$BR2_EXTERNAL_{}_PATH/package/Config.in\"\n", name);
        fs::write(wrap_dir.join("Config.in"), cfg_in)
            .map_err(|e| Error::msg(format!("failed to write Config.in: {e}")))?;

        let mk = format!(
            "include $(sort $(wildcard $(BR2_EXTERNAL_{}_PATH)/package/*/*.mk))\ninclude $(sort $(wildcard $(BR2_EXTERNAL_{}_PATH)/linux/*.mk))\n",
            name, name
        );
        fs::write(wrap_dir.join("external.mk"), mk)
            .map_err(|e| Error::msg(format!("failed to write external.mk: {e}")))?;

        Ok(wrap_dir)
    }
}

#[cfg(unix)]
fn apply_package_override(
    buildroot_src_dir: &Path,
    pkg_name: &str,
    src_pkg_dir: &Path,
) -> Result<()> {
    let dst = buildroot_src_dir.join("package").join(pkg_name);
    if let Ok(meta) = fs::symlink_metadata(&dst) {
        if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(&dst).map_err(|e| {
                Error::msg(format!(
                    "failed to remove upstream package dir {}: {e}",
                    dst.display()
                ))
            })?;
        } else {
            fs::remove_file(&dst).map_err(|e| {
                Error::msg(format!(
                    "failed to remove upstream package path {}: {e}",
                    dst.display()
                ))
            })?;
        }
    }

    symlink(src_pkg_dir, &dst).map_err(|e| {
        Error::msg(format!(
            "failed to install package override {} -> {}: {e}",
            src_pkg_dir.display(),
            dst.display()
        ))
    })?;
    Ok(())
}

#[cfg(unix)]
fn apply_board_overrides(buildroot_src_dir: &Path, boards_root: &Path) -> Result<()> {
    for entry in fs::read_dir(boards_root)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", boards_root.display())))?
    {
        let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
        let src_board_dir = entry.path();
        if !src_board_dir.is_dir() {
            continue;
        }
        let board_name = entry.file_name().to_string_lossy().to_string();
        apply_board_override(buildroot_src_dir, &board_name, &src_board_dir)?;
    }
    Ok(())
}

#[cfg(unix)]
fn apply_board_override(
    buildroot_src_dir: &Path,
    board_name: &str,
    src_board_dir: &Path,
) -> Result<()> {
    if buildroot_src_dir.join(".git").is_dir() {
        // Heal stale board mutations from previous runs before layering overrides.
        let _ = Command::new("git")
            .arg("-C")
            .arg(buildroot_src_dir)
            .arg("checkout")
            .arg("--")
            .arg(format!("board/{board_name}"))
            .status();
    }

    let dst_dir = buildroot_src_dir.join("board").join(board_name);
    fs::create_dir_all(&dst_dir)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", dst_dir.display())))?;

    for entry in fs::read_dir(src_board_dir)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", src_board_dir.display())))?
    {
        let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
        let src = entry.path();
        let dst = dst_dir.join(entry.file_name());

        if let Ok(meta) = fs::symlink_metadata(&dst) {
            if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
                fs::remove_dir_all(&dst).map_err(|e| {
                    Error::msg(format!(
                        "failed to remove board path {}: {e}",
                        dst.display()
                    ))
                })?;
            } else {
                fs::remove_file(&dst).map_err(|e| {
                    Error::msg(format!(
                        "failed to remove board path {}: {e}",
                        dst.display()
                    ))
                })?;
            }
        }

        symlink(&src, &dst).map_err(|e| {
            Error::msg(format!(
                "failed to install board override {} -> {}: {e}",
                src.display(),
                dst.display()
            ))
        })?;

        if src.is_file()
            && src
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("sh"))
                .unwrap_or(false)
        {
            let meta = fs::metadata(&dst)
                .map_err(|e| Error::msg(format!("failed to stat {}: {e}", dst.display())))?;
            let mut perms = meta.permissions();
            let mode = perms.mode();
            let wanted = mode | 0o111;
            if wanted != mode {
                perms.set_mode(wanted);
                fs::set_permissions(&dst, perms).map_err(|e| {
                    Error::msg(format!(
                        "failed to set executable bit on {}: {e}",
                        dst.display()
                    ))
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn board_override_replaces_upstream_board_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let br_src = tmp.path().join("buildroot-src");
        let upstream = br_src.join("board/raspberrypicm5io");
        fs::create_dir_all(&upstream).expect("create upstream board dir");
        fs::write(upstream.join("post-build.sh"), b"#!/bin/sh\necho keep\n")
            .expect("write upstream post-build");
        fs::write(
            upstream.join("post-image.sh"),
            b"#!/bin/sh\necho upstream\n",
        )
        .expect("write upstream file");

        let ext_root = tmp.path().join("external");
        let ext_board = ext_root.join("boards/raspberrypicm5io");
        fs::create_dir_all(&ext_board).expect("create external board dir");
        fs::write(
            ext_board.join("post-image.sh"),
            b"#!/bin/sh\necho external\n",
        )
        .expect("write external file");

        apply_board_overrides(&br_src, &ext_root.join("boards")).expect("apply board override");

        let board_dir = br_src.join("board/raspberrypicm5io");
        assert!(
            board_dir.join("post-build.sh").is_file(),
            "upstream-only files should be preserved"
        );
        let overridden = board_dir.join("post-image.sh");
        let meta = fs::symlink_metadata(&overridden).expect("overridden metadata");
        assert!(
            meta.file_type().is_symlink(),
            "overridden file should be symlink"
        );
        assert_eq!(
            fs::read_link(&overridden).expect("read override symlink"),
            ext_board.join("post-image.sh")
        );
    }

    #[test]
    fn buildroot_git_guard_defaults_are_enabled() {
        let br = BuildrootConfig::default();
        let envs = resolve_buildroot_git_env(&br);
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            envs.get("GIT_HTTP_LOW_SPEED_LIMIT").map(String::as_str),
            Some("1024")
        );
        assert_eq!(
            envs.get("GIT_HTTP_LOW_SPEED_TIME").map(String::as_str),
            Some("60")
        );
        assert_eq!(envs.get("GIT_HTTP_VERSION"), None);
    }

    #[test]
    fn buildroot_git_guard_can_disable_low_speed_limits() {
        let mut br = BuildrootConfig::default();
        br.git_http_low_speed_limit = Some(0);
        br.git_http_low_speed_time = Some(0);
        br.git_http_version = Some("HTTP/1.1".into());

        let envs = resolve_buildroot_git_env(&br);
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert!(!envs.contains_key("GIT_HTTP_LOW_SPEED_LIMIT"));
        assert!(!envs.contains_key("GIT_HTTP_LOW_SPEED_TIME"));
        assert_eq!(
            envs.get("GIT_HTTP_VERSION").map(String::as_str),
            Some("HTTP/1.1")
        );
    }
}

fn apply_buildroot_symbol_overrides(
    kcfg: &mut String,
    br: &BuildrootConfig,
) -> Result<BTreeMap<String, ExpectedSymbol>> {
    let mut expected = BTreeMap::<String, ExpectedSymbol>::new();

    for (name, enabled) in &br.packages {
        let sym = package_name_to_symbol(name);
        if *enabled {
            set_kv(kcfg, &sym, "y");
            expected.insert(sym, ExpectedSymbol::Enabled);
        } else {
            unset_kv(kcfg, &sym);
            expected.insert(sym, ExpectedSymbol::Disabled);
        }
    }

    for (name, version) in &br.package_versions {
        let sym = format!("{}_VERSION", package_name_to_symbol(name));
        set_kv(kcfg, &sym, &quote_kconfig_string(version));
        expected.insert(sym, ExpectedSymbol::Exists);
    }

    for (name, value) in &br.symbols {
        let sym = normalize_symbol_name(name);
        match value {
            toml::Value::Boolean(b) => {
                if *b {
                    set_kv(kcfg, &sym, "y");
                    expected.insert(sym, ExpectedSymbol::Enabled);
                } else {
                    unset_kv(kcfg, &sym);
                    expected.insert(sym, ExpectedSymbol::Disabled);
                }
            }
            toml::Value::Integer(i) => {
                set_kv(kcfg, &sym, &i.to_string());
                expected.insert(sym, ExpectedSymbol::Exists);
            }
            toml::Value::Float(f) => {
                set_kv(kcfg, &sym, &f.to_string());
                expected.insert(sym, ExpectedSymbol::Exists);
            }
            toml::Value::String(s) => {
                set_kv(kcfg, &sym, &quote_kconfig_string(s));
                expected.insert(sym, ExpectedSymbol::Exists);
            }
            _ => {
                return Err(Error::msg(format!(
                    "buildroot.symbols.{name} supports only bool/int/float/string values"
                )));
            }
        }
    }

    Ok(expected)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ExternalPackageState {
    // key = "<external-source-path>::<package-name>"
    packages: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct KernelBuildState {
    // signature of BR2_LINUX_KERNEL* symbols from buildroot .config
    signature: String,
}

fn apply_kernel_change_cleanup(
    ctx: &mut ExecCtx,
    src_dir: &Path,
    out_dir: &Path,
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    let config_path = out_dir.join(".config");
    if !config_path.is_file() {
        return Ok(());
    }
    let signature = kernel_config_signature(&config_path)?;
    if signature.is_empty() {
        return Ok(());
    }

    let state_path = out_dir.join(".gaia-kernel-state.toml");
    let previous = read_kernel_build_state(&state_path)?;
    let has_existing_kernel_tree = detect_existing_kernel_build_tree(out_dir)?;
    let kernel_changed = previous.signature != signature;
    let bootstrap_reconcile = previous.signature.is_empty() && has_existing_kernel_tree;

    if kernel_changed || bootstrap_reconcile {
        if kernel_changed {
            ctx.log("linux kernel configuration changed since last run; forcing linux-dirclean");
        } else {
            ctx.log(
                "detected existing linux build tree without kernel-state cache; forcing linux-dirclean once",
            );
        }
        if !ctx.dry_run {
            run_make(ctx, src_dir, out_dir, &[], &["linux-dirclean"], envs)?;
        }
    }

    if !ctx.dry_run {
        write_kernel_build_state(
            &state_path,
            &KernelBuildState {
                signature: signature.to_string(),
            },
        )?;
    }
    Ok(())
}

fn apply_external_change_cleanup(
    ctx: &mut ExecCtx,
    ws: &WorkspacePaths,
    br: &BuildrootConfig,
    src_dir: &Path,
    out_dir: &Path,
    envs: &BTreeMap<String, String>,
) -> Result<()> {
    let state_path = out_dir.join(".gaia-external-packages-state.toml");
    let previous = read_external_package_state(&state_path)?;
    let current = collect_external_package_signatures(ws, br)?;

    let mut changed_present = BTreeSet::<String>::new();
    for (key, sig) in &current.packages {
        if previous.packages.get(key) != Some(sig) {
            if let Some((_, pkg)) = split_external_state_key(key) {
                changed_present.insert(pkg.to_string());
            }
        }
    }

    let mut removed = BTreeSet::<String>::new();
    for key in previous.packages.keys() {
        if !current.packages.contains_key(key) {
            if let Some((_, pkg)) = split_external_state_key(key) {
                removed.insert(pkg.to_string());
            }
        }
    }

    if !changed_present.is_empty() {
        let mut list = changed_present.iter().cloned().collect::<Vec<_>>();
        list.sort();
        ctx.log(&format!(
            "external package changes detected: {}",
            list.join(", ")
        ));
    }
    if !removed.is_empty() {
        let mut list = removed.iter().cloned().collect::<Vec<_>>();
        list.sort();
        ctx.log(&format!(
            "external packages removed since last run: {}",
            list.join(", ")
        ));
    }

    if !ctx.dry_run {
        for pkg in changed_present {
            let target = format!("{pkg}-dirclean");
            ctx.log(&format!("cleaning changed external package: {target}"));
            run_make(ctx, src_dir, out_dir, &[], &[target.as_str()], envs)?;
        }
    }

    write_external_package_state(&state_path, &current)?;
    Ok(())
}

fn read_external_package_state(path: &Path) -> Result<ExternalPackageState> {
    if !path.is_file() {
        return Ok(ExternalPackageState::default());
    }
    let raw = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
    toml::from_str::<ExternalPackageState>(&raw)
        .map_err(|e| Error::msg(format!("failed to parse {}: {e}", path.display())))
}

fn read_kernel_build_state(path: &Path) -> Result<KernelBuildState> {
    if !path.is_file() {
        return Ok(KernelBuildState::default());
    }
    let raw = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
    toml::from_str::<KernelBuildState>(&raw)
        .map_err(|e| Error::msg(format!("failed to parse {}: {e}", path.display())))
}

fn write_kernel_build_state(path: &Path, state: &KernelBuildState) -> Result<()> {
    let body = toml::to_string_pretty(state)
        .map_err(|e| Error::msg(format!("failed to encode kernel state: {e}")))?;
    fs::write(path, body)
        .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))
}

fn detect_existing_kernel_build_tree(out_dir: &Path) -> Result<bool> {
    let build_dir = out_dir.join("build");
    if !build_dir.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(&build_dir)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", build_dir.display())))?
    {
        let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
        if !entry.path().is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with("linux") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn kernel_config_signature(config_path: &Path) -> Result<String> {
    let raw = fs::read_to_string(config_path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", config_path.display())))?;
    let mut lines = raw
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("BR2_LINUX_KERNEL"))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();
    Ok(lines.join("\n"))
}

fn write_external_package_state(path: &Path, state: &ExternalPackageState) -> Result<()> {
    let body = toml::to_string_pretty(state)
        .map_err(|e| Error::msg(format!("failed to encode external package state: {e}")))?;
    fs::write(path, body)
        .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))
}

fn collect_external_package_signatures(
    ws: &WorkspacePaths,
    br: &BuildrootConfig,
) -> Result<ExternalPackageState> {
    let mut out = ExternalPackageState::default();
    for raw in &br.external {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let src = resolve_root_or_abs(ws, trimmed)?;
        if !src.exists() {
            continue;
        }
        let package_roots = [src.join("packages"), src.join("package")];
        for root in package_roots {
            if !root.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&root)
                .map_err(|e| Error::msg(format!("failed to read {}: {e}", root.display())))?
            {
                let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
                let pkg_dir = entry.path();
                if !pkg_dir.is_dir() || !pkg_dir.join("Config.in").is_file() {
                    continue;
                }
                let pkg_name = entry.file_name().to_string_lossy().to_string();
                let sig = signature_for_dir(&pkg_dir)?;
                let key = format!("{}::{}", src.display(), pkg_name);
                out.packages.insert(key, sig);
            }
        }
    }
    Ok(out)
}

fn split_external_state_key(key: &str) -> Option<(&str, &str)> {
    let (src, pkg) = key.rsplit_once("::")?;
    Some((src, pkg))
}

fn signature_for_dir(dir: &Path) -> Result<String> {
    let mut files = Vec::<PathBuf>::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        for entry in fs::read_dir(&p)
            .map_err(|e| Error::msg(format!("failed to read {}: {e}", p.display())))?
        {
            let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| Error::msg(format!("failed to stat {}: {e}", path.display())))?;
            if meta.file_type().is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut h = Fnv64::new();
    for file in files {
        let rel = file
            .strip_prefix(dir)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or_default()
            .replace('\\', "/");
        h.update(rel.as_bytes());
        h.update(b"\n");
        let meta = fs::symlink_metadata(&file)
            .map_err(|e| Error::msg(format!("failed to stat {}: {e}", file.display())))?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&file).map_err(|e| {
                Error::msg(format!("failed to read symlink {}: {e}", file.display()))
            })?;
            h.update(target.to_string_lossy().as_bytes());
        } else {
            let bytes = fs::read(&file)
                .map_err(|e| Error::msg(format!("failed to read {}: {e}", file.display())))?;
            h.update(&bytes);
        }
        h.update(b"\n");
    }
    Ok(format!("{:016x}", h.finish()))
}

struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

fn package_name_to_symbol(name: &str) -> String {
    format!("BR2_PACKAGE_{}", normalize_ident(name))
}

fn normalize_symbol_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "BR2_INVALID_EMPTY".into();
    }
    if trimmed.starts_with("BR2_") {
        normalize_ident(trimmed)
    } else {
        format!("BR2_{}", normalize_ident(trimmed))
    }
}

fn normalize_ident(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn quote_kconfig_string(v: &str) -> String {
    let esc = v.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{esc}\"")
}

fn log_symbol_validation(
    ctx: &mut ExecCtx,
    expected: &BTreeMap<String, ExpectedSymbol>,
    final_kcfg: &str,
) {
    let mut bad = 0usize;
    for (sym, want) in expected {
        let is_enabled = has_cfg_enabled(final_kcfg, sym);
        let is_disabled = has_cfg_disabled(final_kcfg, sym);
        let ok = match want {
            ExpectedSymbol::Enabled => is_enabled,
            ExpectedSymbol::Disabled => is_disabled,
            ExpectedSymbol::Exists => is_enabled || is_disabled || has_cfg_entry(final_kcfg, sym),
        };
        if !ok {
            bad = bad.saturating_add(1);
            ctx.log(&format!(
                "WARN: requested symbol '{}' was not realized after olddefconfig",
                sym
            ));
        }
    }
    if bad > 0 {
        ctx.log(&format!(
            "WARN: {} requested buildroot symbols were ignored (missing/invalid dependencies)",
            bad
        ));
    }
}

fn has_cfg_enabled(cfg: &str, key: &str) -> bool {
    cfg.lines().any(|l| l == format!("{key}=y"))
}

fn has_cfg_disabled(cfg: &str, key: &str) -> bool {
    cfg.lines().any(|l| l == format!("# {key} is not set"))
}

fn has_cfg_entry(cfg: &str, key: &str) -> bool {
    let prefix = format!("{key}=");
    cfg.lines()
        .any(|l| l.starts_with(&prefix) || l == format!("# {key} is not set"))
}

fn set_kv(cfg: &mut String, key: &str, val: &str) {
    let prefix = format!("{}=", key);
    let unset = format!("# {} is not set", key);
    let mut out = Vec::new();
    let mut done = false;
    for line in cfg.lines() {
        if line.starts_with(&prefix) || line.starts_with(&unset) {
            if !done {
                out.push(format!("{}={}", key, val));
                done = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        out.push(format!("{}={}", key, val));
    }
    *cfg = out.join("\n");
    if !cfg.ends_with('\n') {
        cfg.push('\n');
    }
}

fn unset_kv(cfg: &mut String, key: &str) {
    let prefix = format!("{}=", key);
    let unset = format!("# {} is not set", key);
    let mut out = Vec::new();
    for line in cfg.lines() {
        if line.starts_with(&prefix) || line.starts_with(&unset) {
            continue;
        }
        out.push(line.to_string());
    }
    out.push(unset);
    *cfg = out.join("\n");
    if !cfg.ends_with('\n') {
        cfg.push('\n');
    }
}

#[derive(Debug, Clone, Copy)]
struct SpeedTweakResult {
    pkg: &'static str,
    changed: bool,
    requires_clean: bool,
}

fn apply_buildroot_codegen_speed_tweaks(
    ctx: &mut ExecCtx,
    src_dir: &Path,
) -> Result<Vec<SpeedTweakResult>> {
    let mut out = Vec::new();
    let llvm_mk = src_dir.join("package/llvm-project/llvm/llvm.mk");
    if llvm_mk.is_file() {
        let patched = ensure_mk_tweak_block(
            &llvm_mk,
            &[
                "LLVM_CMAKE_BACKEND = ninja",
                "HOST_LLVM_CONF_OPTS += -DLLVM_ENABLE_WARNINGS=OFF -DLLVM_ENABLE_PEDANTIC=OFF",
                "LLVM_CONF_OPTS += -DLLVM_ENABLE_WARNINGS=OFF -DLLVM_ENABLE_PEDANTIC=OFF",
            ],
        )?;
        if patched.changed {
            ctx.log("applied llvm build speed tweaks (ninja backend, warning analysis off)");
        }
        out.push(SpeedTweakResult {
            pkg: "llvm",
            changed: patched.changed,
            requires_clean: patched.changed && !patched.had_existing_block,
        });
    }

    let clang_mk = src_dir.join("package/llvm-project/clang/clang.mk");
    if clang_mk.is_file() {
        let patched = ensure_mk_tweak_block(&clang_mk, &["CLANG_CMAKE_BACKEND = ninja"])?;
        if patched.changed {
            ctx.log("applied clang build speed tweaks (ninja backend)");
        }
        out.push(SpeedTweakResult {
            pkg: "clang",
            changed: patched.changed,
            requires_clean: patched.changed && !patched.had_existing_block,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
struct MkTweakApply {
    changed: bool,
    had_existing_block: bool,
}

fn ensure_mk_tweak_block(path: &Path, body_lines: &[&str]) -> Result<MkTweakApply> {
    const START: &str = "# GAIA_SPEED_TWEAK_BEGIN";
    const END: &str = "# GAIA_SPEED_TWEAK_END";

    let original = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
    let had_existing_block = original.lines().any(|l| l.trim() == START);
    let mut cleaned_lines = Vec::<String>::new();
    let mut in_old_block = false;
    for line in original.lines() {
        let trimmed = line.trim();
        if trimmed == START {
            in_old_block = true;
            continue;
        }
        if in_old_block && trimmed == END {
            in_old_block = false;
            continue;
        }
        if !in_old_block {
            cleaned_lines.push(line.to_string());
        }
    }

    let eval_idx = cleaned_lines
        .iter()
        .position(|l| l.contains("$(eval $(cmake-package))"))
        .ok_or_else(|| {
            Error::msg(format!(
                "could not find cmake package eval line in {}",
                path.display()
            ))
        })?;

    let mut final_lines = Vec::<String>::with_capacity(cleaned_lines.len() + body_lines.len() + 2);
    final_lines.extend(cleaned_lines[..eval_idx].iter().cloned());
    while final_lines.last().is_some_and(|l| l.trim().is_empty()) {
        final_lines.pop();
    }
    final_lines.push(START.to_string());
    for line in body_lines {
        final_lines.push((*line).to_string());
    }
    final_lines.push(END.to_string());
    if cleaned_lines
        .get(eval_idx)
        .is_some_and(|line| !line.trim().is_empty())
    {
        final_lines.push(String::new());
    }
    final_lines.extend(cleaned_lines[eval_idx..].iter().cloned());
    let mut rendered = final_lines.join("\n");
    rendered.push('\n');

    if rendered == original {
        return Ok(MkTweakApply {
            changed: false,
            had_existing_block,
        });
    }
    fs::write(path, rendered)
        .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))?;
    Ok(MkTweakApply {
        changed: true,
        had_existing_block,
    })
}

fn apply_compression(cfg: &mut String, compression: Option<&str>) {
    let flags = [
        "BR2_TARGET_ROOTFS_EXT2_NONE",
        "BR2_TARGET_ROOTFS_EXT2_GZIP",
        "BR2_TARGET_ROOTFS_EXT2_BZIP2",
        "BR2_TARGET_ROOTFS_EXT2_LZ4",
        "BR2_TARGET_ROOTFS_EXT2_LZMA",
        "BR2_TARGET_ROOTFS_EXT2_LZO",
        "BR2_TARGET_ROOTFS_EXT2_XZ",
        "BR2_TARGET_ROOTFS_EXT2_ZSTD",
    ];
    for f in flags {
        unset_kv(cfg, f);
    }

    let selected = match compression
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("none")
        .to_ascii_lowercase()
        .as_str()
    {
        "gzip" => "BR2_TARGET_ROOTFS_EXT2_GZIP",
        "bzip2" => "BR2_TARGET_ROOTFS_EXT2_BZIP2",
        "lz4" => "BR2_TARGET_ROOTFS_EXT2_LZ4",
        "lzma" => "BR2_TARGET_ROOTFS_EXT2_LZMA",
        "lzo" => "BR2_TARGET_ROOTFS_EXT2_LZO",
        "xz" => "BR2_TARGET_ROOTFS_EXT2_XZ",
        "zstd" => "BR2_TARGET_ROOTFS_EXT2_ZSTD",
        _ => "BR2_TARGET_ROOTFS_EXT2_NONE",
    };

    set_kv(cfg, selected, "y");
}
