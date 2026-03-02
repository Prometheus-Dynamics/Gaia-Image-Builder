use std::fs;
use std::sync::Arc;

use gaia_image_builder::config::ConfigDoc;

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

#[test]
fn starting_point_rootfs_dir_executes_full_flow() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let build_file = root.join("integration.toml");

    fs::create_dir_all(root.join("seed-rootfs/etc")).expect("mkdir seed etc");
    fs::create_dir_all(root.join("seed-rootfs/usr/bin")).expect("mkdir seed bin");
    fs::write(
        root.join("seed-rootfs/etc/os-release"),
        "ID=debian\nVERSION_ID=12\n",
    )
    .expect("write os-release");
    fs::write(root.join("seed-rootfs/usr/bin/apt-get"), "").expect("write apt-get");
    fs::write(root.join("seed-rootfs/etc/original.txt"), "from-seed\n").expect("write seed file");

    let raw = format!(
        r#"
[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[buildroot]
archive_format = "none"
report = true
report_hashes = false

[buildroot.starting_point]
enabled = true
rootfs_dir = "seed-rootfs"
apply_stage_overlay = true

[buildroot.starting_point.packages]
enabled = true
manager = "auto"
execute = false
install = ["curl"]
release_version = "12"
allow_major_upgrade = false

[stage]
enabled = true

[[stage.files]]
dst = "/etc/hostname"
content = "gaia-starting-point\n"
"#,
        root.display()
    );
    fs::write(&build_file, raw).expect("write build file");

    let doc = gaia_image_builder::config::load(&build_file).expect("load config");
    let plan = build_plan(&doc).expect("build plan");
    let reg = gaia_image_builder::executor::builtin_registry().expect("registry");
    let sink = Arc::new(gaia_image_builder::executor::StdoutSink::default());
    let mut ctx = gaia_image_builder::executor::ExecCtx::new(false, sink);
    gaia_image_builder::executor::execute_plan(&doc, &plan, &reg, &mut ctx).expect("execute plan");

    let collected_rootfs = root
        .join("out")
        .join("integration")
        .join("gaia")
        .join("images")
        .join("rootfs");
    assert!(
        collected_rootfs.join("etc").join("original.txt").is_file(),
        "seed rootfs content should be copied"
    );
    assert!(
        collected_rootfs.join("etc").join("hostname").is_file(),
        "stage overlay should be applied"
    );
    let hostname =
        fs::read_to_string(collected_rootfs.join("etc").join("hostname")).expect("read hostname");
    assert_eq!(hostname, "gaia-starting-point\n");

    let manifest_path = root
        .join("out")
        .join("integration")
        .join("gaia")
        .join("manifest.json");
    let manifest_raw = fs::read_to_string(&manifest_path).expect("read manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest_raw).expect("manifest json");
    assert!(
        manifest
            .get("starting_point")
            .and_then(|v| v.get("source_label"))
            .and_then(|v| v.as_str())
            .map(|s| s.contains("rootfs_dir"))
            .unwrap_or(false),
        "manifest should contain starting_point source_label"
    );
    assert_eq!(
        manifest
            .get("starting_point")
            .and_then(|v| v.get("package_reconcile"))
            .and_then(|v| v.get("executed"))
            .and_then(|v| v.as_bool()),
        Some(false)
    );
}
