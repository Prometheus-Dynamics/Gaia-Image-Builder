//! A rust artifact's `build_env`: config tokens are rendered, `${secret:}`
//! is kept for cargo's start, and the toolchain and build group rules.

pub mod support;

use gaia_config::try_resolve_config;
use gaia_spec::{ArtifactDefinition, ResolvedBuildSpec};
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

fn resolve(name: &str, body: &str) -> ResolvedBuildSpec {
    let root = create_temp_workspace(name);
    let build = root.join("build.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "{name}"
version = "1.2.3"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "none"

[secrets.git_token]
env = "GAIA_TEST_RUST_BUILD_ENV_TOKEN"

{body}
"#
        ),
    )
    .expect("build file");
    try_resolve_config(&build.display().to_string()).expect("resolve")
}

fn codes(spec: &ResolvedBuildSpec) -> Vec<&'static str> {
    validate_spec(spec)
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn rust_env(spec: &ResolvedBuildSpec, id: &str) -> Vec<(String, String)> {
    let artifact = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("the artifact");
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.build_env.clone(),
        other => panic!("expected a rust artifact, got {other:?}"),
    }
}

#[test]
fn build_env_renders_config_tokens_and_keeps_secret_references() {
    let spec = resolve(
        "rust-build-env",
        r#"
[[artifacts]]
id = "app"
kind = "rust"
package = "app"
output_path = "out/app"
build_env = [
  ["APP_VERSION", "${build.version}"],
  ["CARGO_NET_GIT_FETCH_WITH_CLI", "true"],
  ["GIT_TOKEN", "${secret:git_token}"],
]
"#,
    );

    assert_eq!(
        rust_env(&spec, "app"),
        [
            ("APP_VERSION".to_string(), "1.2.3".to_string()),
            (
                "CARGO_NET_GIT_FETCH_WITH_CLI".to_string(),
                "true".to_string()
            ),
            ("GIT_TOKEN".to_string(), "${secret:git_token}".to_string()),
        ]
    );
    let codes = codes(&spec);
    assert!(
        codes.iter().all(|code| !code.starts_with("secret_")),
        "{codes:?}"
    );
}

#[test]
fn rustflags_in_build_env_is_refused_for_a_toolchain_build() {
    let spec = resolve(
        "rust-build-env-rustflags",
        r#"
[[sources]]
id = "sdk"
kind = "path"
path = "sdk"

[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
source = "sdk"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"
rust_target = "aarch64-unknown-linux-gnu"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
toolchain = "systemcore"
output_path = "out/app"
build_env = [["RUSTFLAGS", "-C opt-level=s"]]
"#,
    );

    assert!(
        codes(&spec).contains(&"artifact_toolchain_rustflags"),
        "{:?}",
        codes(&spec)
    );
}

#[test]
fn build_group_members_must_share_build_env() {
    let spec = resolve(
        "rust-build-env-group",
        r#"
[[artifacts]]
id = "api"
kind = "rust"
package = "api"
build_group = "apps"
output_path = "out/api"
build_env = [["APP_MODE", "a"]]

[[artifacts]]
id = "cli"
kind = "rust"
package = "cli"
build_group = "apps"
output_path = "out/cli"
build_env = [["APP_MODE", "b"]]
"#,
    );

    assert!(
        codes(&spec).contains(&"rust_build_group_conflict"),
        "{:?}",
        codes(&spec)
    );
}
