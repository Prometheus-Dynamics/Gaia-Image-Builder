pub mod support;

use gaia_app::{
    AppArgs, CommandOutcome, backend_overview_lines, run_with_args, runtime_overview_lines,
};
use gaia_config::{ResolveOptions, resolve_config_with_options};
use std::fs;
use std::path::PathBuf;
use support::{config_path, seed_default_assets, seed_reuse_state, unique_dir};

#[test]
fn backend_and_runtime_overview_helpers_expose_runtime_domains() {
    let spec = resolve_config_with_options(
        &config_path(),
        &ResolveOptions {
            explicit_overrides: vec![
                (
                    "workspace.root_dir".into(),
                    unique_dir("gaia-overview-root"),
                ),
                ("workspace.out_dir".into(), unique_dir("gaia-overview-out")),
                (
                    "workspace.build_dir".into(),
                    unique_dir("gaia-overview-build"),
                ),
            ],
            ..ResolveOptions::default()
        },
    );
    let backend_lines = backend_overview_lines(&spec);
    assert!(backend_lines.iter().any(|line| line.contains(
        "runtime plan: installs=1 stage-files=1 stage-envs=1 stage-services=1 checkpoints=1"
    )));
    assert!(backend_lines
        .iter()
        .any(|line| line.contains("failure policy: rollback_on_error=true preserve_failed_outputs=false rollback_domains=sources,artifacts,installs,stage,images,checkpoints")));
    assert!(
        backend_lines
            .iter()
            .any(|line| line
                .contains("runtime install target: install-gaia-app -> /usr/bin/default"))
    );
    assert!(
        backend_lines
            .iter()
            .any(|line| line.contains("runtime checkpoint target: base-image via local"))
    );

    let run_root_dir = unique_dir("gaia-overview-cli-root");
    let run_out_dir = unique_dir("gaia-overview-cli-out");
    let run_build_dir = unique_dir("gaia-overview-cli-build");
    fs::create_dir_all(&run_root_dir).expect("overview root dir");
    fs::create_dir_all(PathBuf::from(&run_root_dir).join("src")).expect("overview root src");
    fs::write(
        PathBuf::from(&run_root_dir).join("Cargo.toml"),
        "[package]\nname = \"gaia\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .expect("overview cargo toml");
    fs::write(
        PathBuf::from(&run_root_dir).join("src/main.rs"),
        "fn main() {}\n",
    )
    .expect("overview main");
    seed_default_assets(&run_root_dir);
    seed_reuse_state(&run_root_dir, &run_build_dir, &run_out_dir);

    let run = run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        "image.allow_fallback=true".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={run_root_dir}"),
        "--set".to_string(),
        format!("workspace.out_dir={run_out_dir}"),
        "--set".to_string(),
        format!("workspace.build_dir={run_build_dir}"),
    ]));

    match run {
        CommandOutcome::Ran { report, .. } => {
            let runtime_lines = runtime_overview_lines(&report);
            assert!(runtime_lines
                .iter()
                .any(|line| line.contains("runtime state: installs=1 stage-files=1 stage-envs=1 stage-services=1 checkpoints=1")));
            assert!(runtime_lines.iter().any(|line| {
                line.contains("runtime install sample: install-gaia-app -> /usr/bin/default")
            }));
            assert!(
                runtime_lines
                    .iter()
                    .any(|line| line.contains("runtime checkpoint sample: base-image via local"))
            );
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}
