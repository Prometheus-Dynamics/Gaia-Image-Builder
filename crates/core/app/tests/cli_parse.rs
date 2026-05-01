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
