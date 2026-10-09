pub mod support;

use gaia_app::{AppArgs, AppCommand};

#[test]
fn parses_help_and_version_commands() {
    let help = AppArgs::parse_from(["--help"]);
    assert_eq!(help.command, AppCommand::Help);
    assert!(help.build.is_empty());

    let version = AppArgs::parse_from(["version"]);
    assert_eq!(version.command, AppCommand::Version);
    assert!(version.build.is_empty());

    let tui = AppArgs::parse_from(["tui", "examples/default-workspace/configs/default.toml"]);
    assert_eq!(tui.command, AppCommand::Tui);
    assert_eq!(tui.build, "examples/default-workspace/configs/default.toml");
}

#[test]
fn parses_default_run_and_override_flags() {
    let args = AppArgs::parse_from([
        "run",
        "examples/default-workspace/configs/default.toml",
        "--preset",
        "ci",
        "--env-file",
        "examples/default-workspace/configs/runtime.env",
        "--env",
        "API_TOKEN=super-secret-token",
        "--env",
        "GAIA_MODE=ci-env",
        "--set",
        "env.DB_PASSWORD=ultra-secret-password",
        "--set",
        "build.version=9.9.9",
    ]);

    assert_eq!(args.command, AppCommand::Run);
    assert_eq!(
        args.build,
        "examples/default-workspace/configs/default.toml"
    );
    assert_eq!(args.preset.as_deref(), Some("ci"));
    assert_eq!(
        args.env_files,
        vec!["examples/default-workspace/configs/runtime.env".to_string()]
    );
    assert_eq!(
        args.env_overrides,
        vec![
            ("API_TOKEN".to_string(), "super-secret-token".to_string()),
            ("GAIA_MODE".to_string(), "ci-env".to_string()),
        ]
    );
    assert_eq!(
        args.explicit_overrides,
        vec![
            (
                "env.DB_PASSWORD".to_string(),
                "ultra-secret-password".to_string()
            ),
            ("build.version".to_string(), "9.9.9".to_string()),
        ]
    );
}

#[test]
fn parses_clean_command_flags() {
    let args = AppArgs::parse_from([
        "clean",
        "examples/default-workspace/configs/default.toml",
        "--profile",
        "dist",
        "--target",
        "out",
        "--path",
        ".cache/gaia",
        "--dry-run",
    ]);

    assert_eq!(args.command, AppCommand::Clean);
    assert_eq!(
        args.build,
        "examples/default-workspace/configs/default.toml"
    );
    assert_eq!(args.clean.profile.as_deref(), Some("dist"));
    assert_eq!(args.clean.targets, vec!["out".to_string()]);
    assert_eq!(args.clean.paths, vec![".cache/gaia".to_string()]);
    assert!(args.clean.dry_run);
}

#[test]
fn flags_are_never_taken_as_the_build_path() {
    let args = AppArgs::parse_from(["tui", "--builds-dir", "configs/builds"]);

    assert_eq!(args.command, AppCommand::Tui);
    assert!(!args.build_explicit);
    assert_ne!(args.build, "--builds-dir");
    assert_eq!(args.builds_dir.as_deref(), Some("configs/builds"));
    assert!(args.usage_errors.is_empty());
}

#[test]
fn build_path_may_follow_flags() {
    let args = AppArgs::parse_from(["plan", "--preset", "ci", "configs/builds/cm5.toml"]);

    assert_eq!(args.command, AppCommand::Plan);
    assert_eq!(args.build, "configs/builds/cm5.toml");
    assert!(args.build_explicit);
    assert_eq!(args.preset.as_deref(), Some("ci"));
}

#[test]
fn reports_unknown_flags_missing_values_and_extra_arguments() {
    let args = AppArgs::parse_from([
        "run",
        "a.toml",
        "b.toml",
        "--bogus",
        "--set",
        "no-equals-sign",
        "--preset",
    ]);

    assert_eq!(
        args.usage_errors,
        vec![
            "unexpected argument 'b.toml'".to_string(),
            "unknown flag '--bogus'".to_string(),
            "--set expects KEY=VALUE, got 'no-equals-sign'".to_string(),
            "--preset requires a value".to_string(),
        ]
    );
}

#[test]
fn parses_lock_update_with_optional_source_ids() {
    let all = AppArgs::parse_from(["lock", "cm5.toml", "--update"]);
    assert_eq!(all.command, AppCommand::Lock);
    assert_eq!(all.build, "cm5.toml");
    assert!(all.lock.update);
    assert!(all.lock.sources.is_empty());
    assert!(all.usage_errors.is_empty());

    let named = AppArgs::parse_from(["lock", "cm5.toml", "--update", "orion,tools"]);
    assert_eq!(named.lock.sources, vec!["orion", "tools"]);
    assert!(named.usage_errors.is_empty());

    // Before the build path, the value after --update stays the build.
    let flag_first = AppArgs::parse_from(["lock", "--update", "cm5.toml"]);
    assert_eq!(flag_first.build, "cm5.toml");
    assert!(flag_first.lock.sources.is_empty());

    let equals = AppArgs::parse_from(["lock", "--update=orion", "cm5.toml"]);
    assert_eq!(equals.lock.sources, vec!["orion"]);
    assert_eq!(equals.build, "cm5.toml");

    let followed_by_flag = AppArgs::parse_from(["lock", "cm5.toml", "--update", "--preset", "ci"]);
    assert!(followed_by_flag.lock.sources.is_empty());
    assert_eq!(followed_by_flag.preset.as_deref(), Some("ci"));
}

#[test]
fn parses_clean_cache_flags() {
    let args = AppArgs::parse_from([
        "clean",
        "cm5.toml",
        "--target",
        "caches",
        "--all-caches",
        "--dry-run",
    ]);
    assert_eq!(args.command, AppCommand::Clean);
    assert_eq!(args.clean.targets, vec!["caches"]);
    assert!(args.clean.all_caches);
    assert!(args.clean.dry_run);
}

#[test]
fn help_flag_after_command_shows_help() {
    let args = AppArgs::parse_from(["run", "a.toml", "--help"]);
    assert_eq!(args.command, AppCommand::Help);
}

#[test]
fn parses_cache_flags() {
    let args = AppArgs::parse_from([
        "cache",
        "cm5.toml",
        "--level",
        "project",
        "--package",
        "mesa*",
        "--remove",
        "mesa3d,linux@ab12",
        "--dry-run",
    ]);
    assert_eq!(args.command, AppCommand::Cache);
    assert_eq!(args.build, "cm5.toml");
    assert_eq!(args.cache.level.as_deref(), Some("project"));
    assert_eq!(args.cache.packages, vec!["mesa*"]);
    assert_eq!(args.cache.remove, vec!["mesa3d", "linux@ab12"]);
    assert!(args.cache.dry_run);
}

#[test]
fn run_selectors_are_optional_for_status_and_control_commands() {
    // No run named: the registry decides, so no build config is implied.
    for command in ["status", "pause", "resume", "cancel"] {
        let args = AppArgs::parse_from([command]);
        assert!(!args.build_explicit, "{command} names no build");
        let follow = AppArgs::parse_from(["status", "-f"]);
        assert!(follow.follow && !follow.build_explicit);
    }
    // A number, a name or a config path is the named run.
    for selector in ["2", "Cm5 image", "configs/builds/cm5.toml"] {
        let args = AppArgs::parse_from(["status", selector, "--follow"]);
        assert!(args.build_explicit);
        assert_eq!(args.build, selector);
        assert!(args.follow);
    }
    let cancel = AppArgs::parse_from(["cancel", "1"]);
    assert_eq!(cancel.command, AppCommand::Cancel);
    assert_eq!((cancel.build_explicit, cancel.build.as_str()), (true, "1"));
}
