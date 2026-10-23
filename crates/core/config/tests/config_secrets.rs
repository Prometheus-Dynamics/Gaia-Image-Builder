//! `[secrets]`: the entries keep their source, and a reference to one stays in
//! the resolved spec as written (the value is read when it is used).

pub mod support;

use gaia_config::resolve_config;
use gaia_spec::ArtifactDefinition;
use support::write_temp_config;

const JAVA_WITH_ENV: &str = r#"
build_name = "secrets-java"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "myapp-source"
kind = "path"
path = "/tmp/myapp"

[[artifacts]]
id = "myapp-jar"
kind = "java"
source = "myapp-source"
build_target = "build/libs/myapp.jar"
build_command = ["./gradlew", "jar"]
build_env = [["GAIA_BUILD_TOKEN", "${secret:tok}"]]
output_path = "out/myapp.jar"

[secrets.tok]
command = ["printf", "x"]
"#;

#[test]
fn the_secrets_table_keeps_each_source_and_defaults_to_required() {
    let path = write_temp_config(
        r#"
build_name = "secrets-table"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[secrets.ghcr]
command = ["gh", "auth", "token"]

[secrets.signing]
file = "~/.config/gaia/secrets/signing"
required = false

[secrets.ci]
env = "CI_TOKEN"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let entry = |name: &str| {
        spec.secrets
            .entry(name)
            .unwrap_or_else(|| panic!("entry {name}"))
            .clone()
    };
    let ghcr = entry("ghcr");
    assert_eq!(
        ghcr.command,
        Some(vec!["gh".into(), "auth".into(), "token".into()])
    );
    assert!(ghcr.required);
    let signing = entry("signing");
    assert_eq!(
        signing.file.as_deref(),
        Some("~/.config/gaia/secrets/signing")
    );
    assert!(!signing.required);
    assert_eq!(entry("ci").env.as_deref(), Some("CI_TOKEN"));
}

#[test]
fn a_secret_reference_in_build_env_stays_as_written_and_is_not_unresolved() {
    let path = write_temp_config(JAVA_WITH_ENV);

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    let java = spec
        .artifacts
        .iter()
        .find_map(|artifact| match &artifact.definition {
            ArtifactDefinition::Java(java) => Some(java),
            _ => None,
        })
        .expect("a java artifact");
    assert_eq!(
        java.build_env,
        vec![("GAIA_BUILD_TOKEN".to_string(), "${secret:tok}".to_string())]
    );
    assert!(spec.policy.interpolation.unresolved.is_empty());
}

#[test]
fn a_checkpoint_token_and_username_reach_the_registry_settings() {
    let path = write_temp_config(
        r#"
build_name = "secrets-checkpoint"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"
repository = "owner/gaia-cache"
use_policy = "auto"
upload_policy = "auto"
verify_key = "/keys/gaia.pub"
sign_key = "secret:signing"
token = "secret:ghcr"
username = "ci-bot"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let oci = spec.checkpoints.points[0]
        .backend
        .as_ref()
        .and_then(|backend| backend.oci.as_ref())
        .expect("an oci checkpoint");
    assert_eq!(oci.token.as_deref(), Some("secret:ghcr"));
    assert_eq!(oci.username.as_deref(), Some("ci-bot"));
    assert_eq!(oci.sign_key.as_deref(), Some("secret:signing"));
}
