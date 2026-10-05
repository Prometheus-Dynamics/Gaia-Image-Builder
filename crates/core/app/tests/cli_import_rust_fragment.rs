//! A project publishes an importable Gaia fragment (like Orion's
//! `packaging/gaia/orion-node.toml`): a Rust artifact built from the project
//! itself for aarch64, its install, and a systemd unit shipped next to the
//! fragment. Image recipes import it from a pinned git source; it must
//! validate and plan alongside their own layers.
pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use gaia_spec::ArtifactDefinition;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::unique_dir;

const FRAGMENT: &str = r#"
[[artifacts]]
id = "orion-node"
kind = "rust"
package = "orion-node"
source = "orion"
target = "aarch64-unknown-linux-gnu"
no_default_features = true
features = ["single-node"]
install_name = "orion-node"
install_class = "binary"
install_dest_hint = "/usr/bin/orion-node"
output_path = "${workspace.out_dir}/artifacts/orion-node"

[[install]]
id = "install-orion-node"
artifact = "orion-node"
dest = "/usr/bin/orion-node"
replace = true
mode = 493
owner = "root"

[[stage.services]]
id = "orion-node"
name = "orion-node.service"
unit_path = "@self/orion-node.service"
"#;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// An Orion-like repository holding the fragment and its unit file.
fn orion_repo(root: &Path) -> (PathBuf, String) {
    let repo = root.join("orion");
    let packaging = repo.join("packaging/gaia");
    fs::create_dir_all(&packaging).expect("packaging dir");
    fs::write(packaging.join("orion-node.toml"), FRAGMENT).expect("fragment");
    fs::write(
        packaging.join("orion-node.service"),
        "[Service]\nExecStart=/usr/bin/orion-node\n",
    )
    .expect("unit");
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("cargo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "fragment"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    (repo, head)
}

#[test]
fn imported_rust_fragment_with_service_validates_and_plans() {
    let root = PathBuf::from(unique_dir("gaia-import-rust-fragment"));
    let (repo, rev) = orion_repo(&root);
    let workspace = root.join("image");
    fs::create_dir_all(workspace.join("rootfs")).expect("rootfs");
    fs::write(workspace.join("Cargo.toml"), "[workspace]\n").expect("workspace marker");
    let build = workspace.join("build.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "raze-image"
imports = [{{ source = "orion", path = "packaging/gaia/orion-node.toml" }}]

[workspace]
root_dir = "{workspace}"
build_dir = "build"
out_dir = "out"

[[sources]]
id = "orion"
kind = "git"
repo = "file://{repo}"
rev = "{rev}"

[[stage.env_sets]]
id = "image-version"
name = "image-version"
entries = [["ORION_COMMIT", "${{source.orion.commit}}"]]

[image]
kind = "starting-point"
rootfs_path = "{workspace}/rootfs"
"#,
            workspace = workspace.display(),
            repo = repo.display(),
        ),
    )
    .expect("build file");
    let build = build.display().to_string();

    let spec = match run_with_args(AppArgs::parse_from(["validate", build.as_str()])) {
        CommandOutcome::Validated { spec, validation } => {
            assert!(validation.errors.is_empty(), "{:?}", validation.errors);
            spec
        }
        other => panic!("expected validated outcome, got {other:?}"),
    };
    let artifact = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "orion-node")
        .expect("imported artifact");
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => {
            assert!(rust.no_default_features);
            assert_eq!(rust.features, vec!["single-node".to_string()]);
        }
        other => panic!("expected rust artifact, got {other:?}"),
    }
    assert_eq!(
        artifact.target.as_deref(),
        Some("aarch64-unknown-linux-gnu")
    );
    let service = spec
        .stage
        .services
        .iter()
        .find(|service| service.id.as_str() == "orion-node")
        .expect("imported service");
    assert!(
        Path::new(&service.unit_path).is_file(),
        "unit resolves inside the checkout: {}",
        service.unit_path
    );
    assert!(
        spec.stage
            .env_sets
            .iter()
            .flat_map(|env_set| &env_set.entries)
            .any(|(key, value)| key == "ORION_COMMIT" && *value == rev)
    );

    match run_with_args(AppArgs::parse_from(["plan", build.as_str()])) {
        CommandOutcome::Planned {
            plan, diagnostics, ..
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let ids = plan
                .operations
                .iter()
                .map(|operation| operation.id.as_str())
                .collect::<Vec<_>>();
            for expected in [
                "source:orion",
                "artifact:orion-node",
                "install:install-orion-node",
                "stage:service:orion-node",
            ] {
                assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
            }
            let artifact = plan
                .operations
                .iter()
                .find(|operation| operation.id.as_str() == "artifact:orion-node")
                .expect("artifact op");
            assert!(
                artifact
                    .depends_on
                    .iter()
                    .any(|dependency| dependency.as_str() == "source:orion")
            );
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
    let _ = fs::remove_dir_all(root);
}
