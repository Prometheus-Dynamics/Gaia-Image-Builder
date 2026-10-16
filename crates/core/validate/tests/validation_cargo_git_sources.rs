pub mod support;

use gaia_config::try_resolve_config;
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

#[test]
fn cargo_git_sources_must_name_declared_git_sources_without_conflicts() {
    let root = create_temp_workspace("gaia-validate-cargo-git");
    let build = root.join("cargo-git.toml");
    fs::write(
        &build,
        r#"
build_name = "cargo-git-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "dep"
kind = "git"
repo = "https://example.invalid/dep.git"
branch = "main"

[[sources]]
id = "dep2"
kind = "git"
repo = "https://example.invalid/other.git"
branch = "main"

[[sources]]
id = "local"
kind = "path"
path = "local-src"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
cargo_git_sources = { "https://example.invalid/dep.git" = "dep" }
output_path = "out/app"

[[artifacts]]
id = "wrong-kind"
kind = "rust"
package = "wrong"
cargo_git_sources = { "https://example.invalid/dep.git" = "local" }
output_path = "out/wrong"

[[artifacts]]
id = "ghost"
kind = "rust"
package = "ghost"
cargo_git_sources = { "https://example.invalid/dep.git" = "nope" }
output_path = "out/ghost"

[[artifacts]]
id = "clash"
kind = "rust"
package = "clash"
cargo_git_sources = { "https://example.invalid/dep.git" = "dep", "https://example.invalid/dep" = "dep2" }
output_path = "out/clash"
"#,
    )
    .expect("build file");

    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    let report = validate_spec(&spec);
    let has = |code: &str, artifact: &str| {
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == code
                && diagnostic.location.as_deref() == Some(&format!("artifact:{artifact}"))
        })
    };
    assert!(
        !report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.as_deref() == Some("artifact:app")
                    && diagnostic.code.starts_with("rust_cargo_git")
            ),
        "{:?}",
        report.errors
    );
    assert!(
        has("rust_cargo_git_source_not_git", "wrong-kind"),
        "{:?}",
        report.errors
    );
    assert!(
        has("unknown_artifact_source", "ghost"),
        "{:?}",
        report.errors
    );
    assert!(
        has("rust_cargo_git_source_conflict", "clash"),
        "{:?}",
        report.errors
    );
}
