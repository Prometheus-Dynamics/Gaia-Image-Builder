pub mod support;

use gaia_config::{resolve_config, try_resolve_config};
use gaia_spec::CargoGitSourceSpec;
use support::write_temp_config;

const DEP: &str = "https://example.invalid/dep.git";

#[test]
fn cargo_git_sources_name_the_git_source_and_make_the_artifact_depend_on_it() {
    let path = write_temp_config(&format!(
        r#"
build_name = "rust-cargo-git"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "dep"
kind = "git"
repo = "{DEP}"
branch = "main"

[[sources]]
id = "other"
kind = "git"
repo = "https://example.invalid/other.git"
branch = "main"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
cargo_git_sources = {{ "{DEP}" = "dep" }}
output_path = "out/app"
"#
    ));

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let app = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "app")
        .expect("app");
    let gaia_spec::ArtifactDefinition::Rust(rust) = &app.definition else {
        panic!("rust artifact");
    };
    assert_eq!(
        rust.cargo_git_sources,
        [CargoGitSourceSpec {
            url: DEP.into(),
            source: gaia_spec::SourceId::new("dep"),
            repo: DEP.into(),
        }]
    );
    let extra = app
        .extra_sources
        .iter()
        .map(|source| source.as_str())
        .collect::<Vec<_>>();
    assert_eq!(extra, ["dep"]);
}

#[test]
fn cargo_git_sources_merge_with_the_declared_extra_sources_without_the_own_source() {
    let path = write_temp_config(&format!(
        r#"
build_name = "rust-cargo-git-extra"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "dep"
kind = "git"
repo = "{DEP}"
branch = "main"

[[sources]]
id = "other"
kind = "git"
repo = "https://example.invalid/other.git"
branch = "main"

[[artifacts]]
id = "app"
kind = "rust"
source = "other"
sources = ["dep", "other"]
package = "app"
cargo_git_sources = {{ "{DEP}" = "dep" }}
output_path = "out/app"
"#
    ));

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let app = &spec.artifacts[0];
    let extra = app
        .extra_sources
        .iter()
        .map(|source| source.as_str())
        .collect::<Vec<_>>();
    assert_eq!(extra, ["dep"]);
}

#[test]
fn cargo_git_sources_on_a_non_rust_artifact_is_a_config_error() {
    let path = write_temp_config(&format!(
        r#"
build_name = "node-cargo-git"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "dep"
kind = "git"
repo = "{DEP}"
branch = "main"

[[artifacts]]
id = "web"
kind = "node"
package_dir = "web"
cargo_git_sources = {{ "{DEP}" = "dep" }}
output_path = "out/web"
"#
    ));

    let error = try_resolve_config(path.to_str().expect("temp path utf-8"))
        .expect_err("cargo_git_sources needs a rust artifact");
    assert!(error.to_string().contains("cargo_git_sources"), "{error}");
}

#[test]
fn artifacts_without_cargo_git_sources_keep_their_extra_sources() {
    let path = write_temp_config(
        r#"
build_name = "rust-plain"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "dep"
kind = "git"
repo = "https://example.invalid/dep.git"
branch = "main"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
output_path = "out/app"
"#,
    );
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let app = &spec.artifacts[0];
    assert!(app.extra_sources.is_empty());
    let gaia_spec::ArtifactDefinition::Rust(rust) = &app.definition else {
        panic!("rust artifact");
    };
    assert!(rust.cargo_git_sources.is_empty());
}
