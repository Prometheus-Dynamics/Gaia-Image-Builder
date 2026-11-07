//! A trimmed rust library (`trim = { users = [...] }`): every member of its
//! build group records the library's link, and the users must be other
//! crate-building members of that group.

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

{body}
"#
        ),
    )
    .expect("build file");
    try_resolve_config(&build.display().to_string()).expect("resolve")
}

/// The trim's diagnostics; the build file's others are not about it.
fn codes(spec: &ResolvedBuildSpec) -> Vec<&'static str> {
    validate_spec(spec)
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .filter(|code| code.starts_with("rust_"))
        .collect()
}

fn trim(spec: &ResolvedBuildSpec, id: &str) -> Option<gaia_spec::RustTrimSpec> {
    let artifact = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("the artifact");
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.trim.as_deref().cloned(),
        other => panic!("expected a rust artifact, got {other:?}"),
    }
}

const ENGINE: &str = r#"
[[artifacts]]
id = "engine"
kind = "rust"
package = "engine"
build_group = "backend"
output_path = "out/engine"
"#;

fn library(group: &str, users: &str, output: &str) -> String {
    format!(
        r#"
[[artifacts]]
id = "libcore"
kind = "rust"
package = "core"
{group}
output_path = "{output}"
trim = {{ users = [{users}] }}
"#
    )
}

#[test]
fn every_group_member_records_the_trimmed_library() {
    let body = format!(
        "{ENGINE}{}",
        library(
            r#"build_group = "backend""#,
            r#""engine""#,
            "out/libcore.so"
        )
    );
    let spec = resolve("rust-trim-ok", &body);
    assert_eq!(codes(&spec), Vec::<&str>::new());
    let library = trim(&spec, "libcore").expect("trim");
    assert_eq!(library.users, ["engine"]);
    assert_eq!(library.user_files, ["engine"]);
    assert_eq!(library.recorded, ["libcore.so"]);
    let engine = trim(&spec, "engine").expect("the group records");
    assert!(engine.users.is_empty());
    assert_eq!(engine.recorded, ["libcore.so"]);
}

#[test]
fn users_must_be_other_members_of_the_group() {
    let body = format!(
        "{ENGINE}{}",
        library(
            r#"build_group = "backend""#,
            r#""libcore", "elsewhere""#,
            "out/libcore.so"
        )
    );
    let spec = resolve("rust-trim-users", &body);
    assert_eq!(
        codes(&spec),
        ["rust_trim_user_not_in_group", "rust_trim_user_not_in_group"]
    );
}

#[test]
fn a_trim_needs_a_group_and_a_shared_library() {
    let spec = resolve(
        "rust-trim-group",
        &format!("{ENGINE}{}", library("", r#""engine""#, "out/libcore.so")),
    );
    assert_eq!(codes(&spec), ["rust_trim_without_build_group"]);
    let spec = resolve(
        "rust-trim-file",
        &format!(
            "{ENGINE}{}",
            library(r#"build_group = "backend""#, r#""engine""#, "out/core")
        ),
    );
    assert_eq!(codes(&spec), ["rust_trim_not_a_library"]);
}
