use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gaia_image_builder::config::ConfigDoc;
use gaia_image_builder::error::Error;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn example_source_root() -> PathBuf {
    repo_root().join("examples").join("base-os-starting-point")
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            if let Some(parent) = dst_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn setup_example_workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("base-os-starting-point");
    copy_dir_all(&example_source_root(), &ws).expect("copy base-os-starting-point example");
    (tmp, ws)
}

fn build_plan(doc: &ConfigDoc) -> gaia_image_builder::Result<gaia_image_builder::planner::Plan> {
    let mut plan = gaia_image_builder::planner::Plan::default();
    for m in gaia_image_builder::modules::builtin_modules() {
        if m.detect(doc) {
            m.plan(doc, &mut plan)?;
        }
    }
    gaia_image_builder::checkpoints::validate_against_plan(doc, &plan)?;
    plan.finalize_default()?;
    Ok(plan)
}

fn run_build(build_file: &Path, workspace_root: &Path) -> gaia_image_builder::Result<ConfigDoc> {
    let mut doc = gaia_image_builder::config::load(build_file)?;
    let ws_table = doc
        .value
        .as_table_mut()
        .and_then(|t| t.get_mut("workspace"))
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| Error::msg("missing [workspace] table in base-os-starting-point example"))?;
    ws_table.insert(
        "root_dir".into(),
        toml::Value::String(workspace_root.display().to_string()),
    );

    let plan = build_plan(&doc)?;
    let reg = gaia_image_builder::executor::builtin_registry()?;
    let sink = Arc::new(gaia_image_builder::executor::StdoutSink::default());
    let mut ctx = gaia_image_builder::executor::ExecCtx::new(false, sink);
    gaia_image_builder::executor::execute_plan(&doc, &plan, &reg, &mut ctx)?;
    Ok(doc)
}

fn collected_rootfs(workspace_root: &Path, build_name: &str) -> PathBuf {
    workspace_root
        .join("out")
        .join(build_name)
        .join("gaia")
        .join("images")
        .join("rootfs")
}

fn tar_available() -> bool {
    Command::new("tar")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn base_os_starting_point_rootfs_dir_example() {
    let (_tmp, ws) = setup_example_workspace();
    let build_file = ws.join("build-dir.toml");

    run_build(&build_file, &ws).expect("run build-dir");

    let rootfs = collected_rootfs(&ws, "build-dir");
    assert!(rootfs.join("etc/from-seed.txt").is_file());
    assert_eq!(
        fs::read_to_string(rootfs.join("etc/base-os-id")).expect("read base-os-id"),
        "base-os-rootfs-dir\n"
    );

    let manifest_raw = fs::read_to_string(
        ws.join("out")
            .join("build-dir")
            .join("gaia")
            .join("manifest.json"),
    )
    .expect("read manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest_raw).expect("manifest json");
    assert_eq!(
        manifest
            .get("starting_point")
            .and_then(|v| v.get("source_label"))
            .and_then(|v| v.as_str()),
        Some("base-os-starting-point:rootfs_dir")
    );
}

#[test]
fn base_os_starting_point_rootfs_tar_example() {
    if !tar_available() {
        eprintln!("skip: tar is not available");
        return;
    }

    let (_tmp, ws) = setup_example_workspace();
    let script = ws.join("scripts").join("make-base-rootfs-tar.sh");
    let status = Command::new("bash")
        .arg(&script)
        .current_dir(&ws)
        .status()
        .expect("run tar prep script");
    assert!(status.success(), "tar prep script failed: {status}");
    assert!(ws.join("inputs").join("base-rootfs.tar").is_file());

    let build_file = ws.join("build-tar.toml");
    run_build(&build_file, &ws).expect("run build-tar");

    let rootfs = collected_rootfs(&ws, "build-tar");
    assert!(rootfs.join("etc/from-seed.txt").is_file());
    assert_eq!(
        fs::read_to_string(rootfs.join("etc/base-os-id")).expect("read base-os-id"),
        "base-os-rootfs-tar\n"
    );
    assert!(
        ws.join("build")
            .join("starting-point")
            .join("build-tar")
            .join("extract")
            .is_dir(),
        "starting-point extraction cache should exist"
    );

    let manifest_raw = fs::read_to_string(
        ws.join("out")
            .join("build-tar")
            .join("gaia")
            .join("manifest.json"),
    )
    .expect("read manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest_raw).expect("manifest json");
    assert_eq!(
        manifest
            .get("starting_point")
            .and_then(|v| v.get("source_label"))
            .and_then(|v| v.as_str()),
        Some("base-os-starting-point:rootfs_tar")
    );
}

#[test]
fn base_os_starting_point_checkpointed_example() {
    let (_tmp, ws) = setup_example_workspace();
    let build_file = ws.join("build-checkpointed.toml");
    let doc = run_build(&build_file, &ws).expect("run build-checkpointed");

    let rootfs = collected_rootfs(&ws, "build-checkpointed");
    assert!(rootfs.join("etc/from-seed.txt").is_file());
    assert_eq!(
        fs::read_to_string(rootfs.join("etc/base-os-id")).expect("read base-os-id"),
        "base-os-checkpointed-starting-point\n"
    );

    let status = gaia_image_builder::checkpoints::status_for_doc(&doc).expect("status");
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].id, "base-os");
    assert!(!status[0].exists);
}
