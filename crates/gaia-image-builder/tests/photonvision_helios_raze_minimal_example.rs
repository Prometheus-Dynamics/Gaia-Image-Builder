use std::path::PathBuf;

use gaia_image_builder::config::{self, ConfigDoc};
use gaia_image_builder::modules::buildroot_rpi::BuildrootRpiConfig;
use gaia_image_builder::modules::stage::StageConfig;
use gaia_image_builder::workspace::{self, WorkspaceConfig};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn example_root() -> PathBuf {
    repo_root()
        .join("examples")
        .join("photonvision-helios-raze-minimal")
}

fn load_doc() -> ConfigDoc {
    let build = example_root().join("gaia").join("build.toml");
    let mut doc = config::load(&build).expect("load example build.toml");
    let ws_tbl = doc
        .value
        .as_table_mut()
        .and_then(|t| t.get_mut("workspace"))
        .and_then(|v| v.as_table_mut())
        .expect("workspace table");
    ws_tbl.insert(
        "root_dir".into(),
        toml::Value::String(example_root().display().to_string()),
    );
    doc
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

#[test]
fn photonvision_helios_raze_minimal_example_resolves_and_plans() {
    let doc = load_doc();
    let plan = build_plan(&doc).expect("example should plan");

    let ids = plan.tasks().map(|t| t.id.as_str()).collect::<Vec<_>>();

    for expected in [
        "core.init",
        "buildroot.fetch",
        "buildroot.configure",
        "buildroot.build",
        "buildroot.collect",
        "buildroot.rpi.validate",
        "buildroot.rpi.prepare",
        "program.custom.artifacts",
        "program.install.stage",
        "stage.render",
        "checkpoints.restore.buildroot-build",
        "checkpoints.capture.buildroot-build",
    ] {
        assert!(
            ids.iter().any(|id| *id == expected),
            "missing task '{expected}' in plan: {ids:?}"
        );
    }
}

#[test]
fn photonvision_helios_raze_minimal_referenced_assets_exist() {
    let doc = load_doc();
    let ws_cfg: WorkspaceConfig = doc
        .deserialize_path("workspace")
        .unwrap()
        .unwrap_or_default();
    let ws = workspace::load_paths(&ws_cfg).expect("workspace paths");

    let stage: StageConfig = doc.deserialize_path("stage").unwrap().unwrap_or_default();
    for file in &stage.files {
        let Some(src) = file.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let path = ws
            .resolve_config_path(src)
            .expect("resolve stage file src path");
        assert!(path.exists(), "missing stage file src: {}", path.display());
    }

    for (name, unit) in &stage.services.units {
        if !unit.enabled {
            continue;
        }
        if let Some(src) = unit.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let path = ws
                .resolve_config_path(src)
                .expect("resolve service unit src path");
            assert!(
                path.is_file(),
                "missing service unit src for '{name}': {}",
                path.display()
            );
        }

        for asset in &unit.assets {
            let path = ws
                .resolve_config_path(asset.src.trim())
                .expect("resolve service asset src path");
            assert!(
                path.exists(),
                "missing service asset src for '{name}': {}",
                path.display()
            );
        }
    }

    let rpi: BuildrootRpiConfig = doc
        .deserialize_path("buildroot.rpi")
        .unwrap()
        .unwrap_or_default();
    if let Some(config_file) = rpi
        .config_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let path = ws
            .resolve_config_path(config_file)
            .expect("resolve buildroot.rpi.config_file path");
        assert!(
            path.is_file(),
            "missing rpi config_file: {}",
            path.display()
        );
    }

    let fetch_script = ws
        .resolve_config_path("scripts/fetch-photonvision-jar.sh")
        .expect("resolve jar fetch script");
    assert!(
        fetch_script.is_file(),
        "missing jar fetch script: {}",
        fetch_script.display()
    );
    let driver_script = ws
        .resolve_config_path("scripts/fetch-photon-libcamera-driver.sh")
        .expect("resolve driver fetch script");
    assert!(
        driver_script.is_file(),
        "missing driver fetch script: {}",
        driver_script.display()
    );
}
