pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use std::fs;
use std::path::{Path, PathBuf};
use support::{config_path, seed_default_assets, seed_reuse_state, unique_dir, write_temp_build};

#[test]
fn validate_and_run_commands_preserve_masked_selection_and_outputs() {
    let run_root_dir = unique_dir("gaia-cli-root");
    let run_out_dir = unique_dir("gaia-cli-out");
    let run_build_dir = unique_dir("gaia-cli-build");
    fs::create_dir_all(&run_root_dir).expect("cli root dir");
    fs::write(PathBuf::from(&run_root_dir).join("tracked.txt"), "cli-root")
        .expect("cli root tracked file");
    fs::create_dir_all(PathBuf::from(&run_root_dir).join("src")).expect("cli root src dir");
    fs::write(
        PathBuf::from(&run_root_dir).join("Cargo.toml"),
        "[package]\nname = \"gaia\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .expect("cli root cargo toml");
    fs::write(
        PathBuf::from(&run_root_dir).join("src/main.rs"),
        "fn main() {}\n",
    )
    .expect("cli root main");
    seed_default_assets(&run_root_dir);

    let validate = run_with_args(AppArgs::parse_from(vec![
        "validate".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--env-file".to_string(),
        "examples/default-workspace/configs/runtime.env".to_string(),
        "--env".to_string(),
        "API_TOKEN=super-secret-token".to_string(),
        "--env".to_string(),
        "GAIA_MODE=ci-env".to_string(),
        "--set".to_string(),
        "env.DB_PASSWORD=ultra-secret-password".to_string(),
        "--set".to_string(),
        "build.version=9.9.9".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={run_root_dir}"),
    ]));

    match validate {
        CommandOutcome::Validated { spec, validation } => {
            assert!(validation.errors.is_empty());
            assert_eq!(spec.selection.selected_preset.as_deref(), Some("ci"));
            assert!(
                spec.selection
                    .env_overrides
                    .iter()
                    .any(|(key, value)| key == "API_TOKEN" && value == "super-secret-token")
            );
        }
        outcome => panic!("expected validated outcome, got {outcome:?}"),
    }

    seed_reuse_state(&run_root_dir, &run_build_dir, &run_out_dir);

    let run = run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--env-file".to_string(),
        "examples/default-workspace/configs/runtime.env".to_string(),
        "--env".to_string(),
        "API_TOKEN=super-secret-token".to_string(),
        "--env".to_string(),
        "GAIA_MODE=ci-env".to_string(),
        "--set".to_string(),
        "env.DB_PASSWORD=ultra-secret-password".to_string(),
        "--set".to_string(),
        "build.version=9.9.9".to_string(),
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
        CommandOutcome::Ran {
            report,
            report_outputs,
            validation,
            plan_diagnostics,
            execution_errors,
            ..
        } => {
            assert!(validation.errors.is_empty());
            assert!(plan_diagnostics.is_empty());
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            assert_eq!(
                report.summary.operation_count,
                report.summary.completed_operations
            );
            assert!(
                report
                    .selection
                    .selected_env_overrides
                    .iter()
                    .any(|(key, value)| key == "API_TOKEN" && value == "***")
            );
            assert!(
                report
                    .selection
                    .explicit_overrides
                    .iter()
                    .any(|(key, value)| key == "env.DB_PASSWORD" && value == "***")
            );
            assert!(
                report
                    .provenance
                    .install_backend_states
                    .iter()
                    .any(|record| record.id == "install-gaia-app"
                        && record.state.get("dest") == Some(&"/usr/bin/default".to_string()))
            );
            assert!(
                report
                    .provenance
                    .checkpoint_backend_states
                    .iter()
                    .any(|record| record.id == "base-image"
                        && record.state.get("backend") == Some(&"local".to_string()))
            );
            assert_eq!(report_outputs.files.len(), 5);
            assert!(
                report_outputs.files.iter().any(|file| file
                    .path
                    .display()
                    .to_string()
                    .ends_with(".selection.json"))
            );
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_executes_post_build_hook_with_payload() {
    let run_root_dir = unique_dir("gaia-cli-hook-root");
    let run_out_dir = unique_dir("gaia-cli-hook-out");
    let run_build_dir = unique_dir("gaia-cli-hook-build");
    fs::create_dir_all(&run_root_dir).expect("hook root dir");
    seed_default_assets(&run_root_dir);
    let payload_capture = PathBuf::from(unique_dir("gaia-cli-hook-payload"));
    let script_path = PathBuf::from(unique_dir("gaia-cli-hook-script")).with_extension("sh");
    fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\ncp \"$1\" \"{}\"\nprintf 'custom hook summary'\n",
            payload_capture.display()
        ),
    )
    .expect("hook script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script permissions");
    }

    seed_reuse_state(&run_root_dir, &run_build_dir, &run_out_dir);
    let hook_build = write_temp_build(&format!(
        "build_name = \"default\"\nextends = \"{}\"\n\n[reporting.post_build]\nscript = \"{}\"\n",
        config_path(),
        script_path.display()
    ));

    let run = run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        hook_build,
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
        CommandOutcome::Ran {
            post_build_output,
            run_duration,
            execution_errors,
            ..
        } => {
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            assert!(run_duration.as_millis() > 0);
            assert_eq!(post_build_output.as_deref(), Some("custom hook summary"));
            let payload = fs::read_to_string(&payload_capture).expect("captured payload");
            assert!(payload.contains("\"build_name\": \"default\""));
            assert!(payload.contains("\"primary_output\""));
            assert!(payload.contains("\"report_files\""));
            assert!(payload.contains("\"run_duration_ms\""));
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_times_out_hanging_post_build_hook() {
    let script_path = write_hook_script(
        "gaia-cli-hook-timeout-script",
        "#!/usr/bin/env bash\nsleep 5\n",
        true,
    );
    let run = run_with_hook(&script_path, "timeout_seconds = 1\n");

    match run {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("post-build hook failed"));
            assert!(message.contains("timed out after 1s"));
        }
        outcome => panic!("expected failed outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_executes_non_executable_post_build_hook_via_bash() {
    let script_path = write_hook_script(
        "gaia-cli-hook-bash-fallback-script",
        "#!/usr/bin/env bash\nprintf 'fallback hook summary'\n",
        false,
    );
    let run = run_with_hook(&script_path, "");

    match run {
        CommandOutcome::Ran {
            post_build_output,
            execution_errors,
            ..
        } => {
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            assert_eq!(post_build_output.as_deref(), Some("fallback hook summary"));
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_reports_failed_post_build_hook_status() {
    let script_path = write_hook_script(
        "gaia-cli-hook-failure-script",
        "#!/usr/bin/env bash\nprintf 'hook failed detail' >&2\nexit 42\n",
        true,
    );
    let run = run_with_hook(&script_path, "");

    match run {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("post-build hook failed"));
            assert!(message.contains("hook failed detail"));
        }
        outcome => panic!("expected failed outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_bounds_noisy_post_build_hook_output() {
    let script_path = write_hook_script(
        "gaia-cli-hook-noisy-script",
        "#!/usr/bin/env bash\nfor i in $(seq 1 20000); do printf 'line-%05d\\n' \"$i\"; done\nprintf 'done'\n",
        true,
    );
    let run = run_with_hook(&script_path, "");

    match run {
        CommandOutcome::Ran {
            post_build_output,
            execution_errors,
            ..
        } => {
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            let output = post_build_output.expect("hook output");
            assert!(output.len() < 1024 * 1024);
            assert!(output.contains("done"));
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}

fn run_with_hook(script_path: &Path, extra_hook_config: &str) -> CommandOutcome {
    let run_root_dir = unique_dir("gaia-cli-hook-root");
    let run_out_dir = unique_dir("gaia-cli-hook-out");
    let run_build_dir = unique_dir("gaia-cli-hook-build");
    fs::create_dir_all(&run_root_dir).expect("hook root dir");
    seed_default_assets(&run_root_dir);
    seed_reuse_state(&run_root_dir, &run_build_dir, &run_out_dir);
    let hook_build = write_temp_build(&format!(
        "build_name = \"default\"\nextends = \"{}\"\n\n[reporting.post_build]\nscript = \"{}\"\n{}",
        config_path(),
        script_path.display(),
        extra_hook_config,
    ));

    run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        hook_build,
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
    ]))
}

fn write_hook_script(prefix: &str, contents: &str, executable: bool) -> PathBuf {
    let script_path = PathBuf::from(unique_dir(prefix)).with_extension("sh");
    fs::write(&script_path, contents).expect("hook script");
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script permissions");
    }
    let _ = executable;
    script_path
}
