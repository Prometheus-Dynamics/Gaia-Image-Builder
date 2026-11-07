pub mod support;

use gaia_config::{resolve_config, try_resolve_config};
use gaia_spec::{ArtifactDefinition, ArtifactProviderKind};
use support::write_temp_config;

const BASE: &str = r#"
build_name = "command-artifacts"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "helios"
kind = "path"
path = "/tmp/helios"

[[sources]]
id = "scenes"
kind = "path"
path = "/tmp/scenes"
"#;

#[test]
fn a_command_artifact_compiles_with_its_tokens_rendered() {
    let path = write_temp_config(&format!(
        r#"{BASE}
[providers.command]
retry_attempts = 2

[[artifacts]]
id = "helios-ui"
kind = "command"
source = "helios"
command = ["sh", "-c", "bun run build --name ${{build.name}}"]
build_env = [["CACHE", "${{workspace.build_dir}}/bun"], ["SCENES", "${{source.scenes.path}}"]]
collect = "ui/dist"
collect_exclude = ["mock", "./dev/"]
output_path = "out/helios-ui"
"#
    ));
    let spec = resolve_config(path.to_str().expect("utf-8"));

    let artifact = &spec.artifacts[0];
    assert_eq!(artifact.provider_kind(), ArtifactProviderKind::Command);
    let ArtifactDefinition::Command(command) = &artifact.definition else {
        panic!("expected a command artifact");
    };
    assert_eq!(
        command.command,
        ["sh", "-c", "bun run build --name command-artifacts"]
    );
    assert_eq!(command.build_env[0], ("CACHE".into(), "build/bun".into()));
    assert!(
        !command.build_env[1].1.contains("${source"),
        "{:?}",
        command.build_env
    );
    assert_eq!(command.collect.as_deref(), Some("ui/dist"));
    assert_eq!(command.collect_exclude, ["mock", "./dev/"]);
    // A source token adds the source the artifact reads.
    assert_eq!(
        artifact
            .extra_sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>(),
        ["scenes"]
    );
    let policy = spec
        .policy
        .providers
        .artifact_command_policy(ArtifactProviderKind::Command);
    assert_eq!(policy.timeout_seconds, 1800);
    assert_eq!(policy.retry_attempts, 2);
}

#[test]
fn a_command_artifact_rejects_another_kinds_key() {
    let path = write_temp_config(&format!(
        r#"{BASE}
[[artifacts]]
id = "helios-ui"
kind = "command"
source = "helios"
command = ["make"]
build_target = "dist"
output_path = "out/helios-ui"
"#
    ));
    let spec = try_resolve_config(path.to_str().expect("utf-8")).expect("config resolves");
    let warnings = format!("{:?}", spec.metadata.config_warnings);
    assert!(warnings.contains("build_target"), "{warnings}");
    assert!(!warnings.contains("'command'"), "{warnings}");
}
