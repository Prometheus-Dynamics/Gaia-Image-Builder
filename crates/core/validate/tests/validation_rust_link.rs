//! A rust artifact's `link`: `shared` reaches the spec, an input can choose
//! it, a build group links one way, and the standard library (`package =
//! "std"`) is a directory that only a shared group ships.

pub mod support;

use gaia_config::try_resolve_config;
use gaia_spec::{ArtifactDefinition, ArtifactVariantSpec, ResolvedBuildSpec, RustLinkSpec};
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

fn build_file(name: &str, body: &str) -> (support::Workspace, String) {
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

[inputs.link_mode]
type = "string"
default = "shared"

{body}
"#
        ),
    )
    .expect("build file");
    let path = build.display().to_string();
    (root, path)
}

fn resolve(name: &str, body: &str) -> ResolvedBuildSpec {
    let (_root, build) = build_file(name, body);
    try_resolve_config(&build).expect("resolve")
}

fn codes(spec: &ResolvedBuildSpec) -> Vec<&'static str> {
    validate_spec(spec)
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn rust(spec: &ResolvedBuildSpec, id: &str) -> gaia_spec::RustArtifactSpec {
    let artifact = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("the artifact");
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.clone(),
        other => panic!("expected a rust artifact, got {other:?}"),
    }
}

const SHARED_GROUP: &str = r#"
[[artifacts]]
id = "engine"
kind = "rust"
package = "engine"
build_group = "backend"
link = "${input.link_mode}"
output_path = "out/engine"

[[artifacts]]
id = "libcore"
kind = "rust"
package = "core-dylib"
build_group = "backend"
link = "shared"
output_path = "out/libcore.so"

[[artifacts]]
id = "rust-std"
kind = "rust"
package = "std"
build_group = "backend"
output_path = "out/rust-std"
"#;

#[test]
fn a_shared_group_with_its_standard_library_validates() {
    let spec = resolve("rust-link-shared", SHARED_GROUP);
    assert_eq!(rust(&spec, "engine").link, RustLinkSpec::Shared);
    let std = rust(&spec, "rust-std");
    assert!(std.is_std_library());
    assert_eq!(std.variant, ArtifactVariantSpec::Directory);
    assert!(std.cargo_packages().is_empty());
    assert_eq!(
        rust(&spec, "engine").cargo_packages(),
        ["core-dylib", "engine"]
    );
    assert!(
        !codes(&spec).iter().any(|code| code.starts_with("rust_")),
        "{:?}",
        codes(&spec)
    );
}

#[test]
fn members_that_link_differently_are_refused() {
    let body = SHARED_GROUP.replacen(r#"link = "shared""#, r#"link = "static""#, 1);
    let spec = resolve("rust-link-mismatch", &body);
    assert!(codes(&spec).contains(&"rust_build_group_link_mismatch"));
}

#[test]
fn the_standard_library_needs_a_shared_group() {
    let body = SHARED_GROUP
        .replace(r#"link = "${input.link_mode}""#, r#"link = "static""#)
        .replace(r#"link = "shared""#, r#"link = "static""#);
    let spec = resolve("rust-link-std-static", &body);
    assert!(codes(&spec).contains(&"rust_std_without_shared_link"));
}

#[test]
fn an_unknown_link_mode_is_a_config_error() {
    let body = SHARED_GROUP.replacen(r#"link = "shared""#, r#"link = "dynamic""#, 1);
    let (_root, build) = build_file("rust-link-unknown", &body);
    let error = try_resolve_config(&build).expect_err("an unknown link mode");
    assert!(error.to_string().contains(r#"link = "dynamic""#), "{error}");
}
