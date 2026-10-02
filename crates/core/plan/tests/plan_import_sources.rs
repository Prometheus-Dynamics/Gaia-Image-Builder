//! Operation fingerprints follow the revision of config import sources.

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_plan::{OperationKind, operation_fingerprint};
use gaia_spec::{ResolvedBuildSpec, StageItemId};
use std::fs;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=gaia",
            "-c",
            "user.email=gaia@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn resolve_at(workspace: &Path, repo: &Path, rev: &str) -> ResolvedBuildSpec {
    let build = workspace.join("build.toml");
    fs::write(
        &build,
        format!(
            r#"build_name = "imports"
imports = [{{ source = "atlas", path = "layer.toml" }}]

[[stage.env_sets]]
id = "local-env"
name = "local"
entries = [["LOCAL", "1"]]

[[sources]]
id = "atlas"
kind = "git"
repo = "file://{}"
rev = "{rev}"
"#,
            repo.display()
        ),
    )
    .expect("build");
    try_resolve_config_with_options(&build.display().to_string(), &ResolveOptions::default())
        .expect("resolve")
}

fn env_set(id: &str) -> OperationKind {
    OperationKind::RenderStageEnvSet {
        item_id: StageItemId::new(id),
    }
}

#[test]
fn fingerprint_of_layer_items_changes_with_the_import_revision() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-plan-import-src-{nonce}"));
    let workspace = root.join("workspace");
    let repo = root.join("atlas");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&repo).expect("repo");
    fs::write(workspace.join("Cargo.toml"), "[workspace]\n").expect("cargo");
    fs::write(
        repo.join("layer.toml"),
        "[[stage.env_sets]]\nid = \"raze-env\"\nname = \"raze\"\nentries = [[\"RAZE\", \"1\"]]\n",
    )
    .expect("layer");
    fs::write(repo.join("README.md"), "one\n").expect("readme");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    // The layer file is identical at the second revision.
    fs::write(repo.join("README.md"), "two\n").expect("readme");
    git(&repo, &["commit", "--quiet", "-am", "two"]);
    let second = git(&repo, &["rev-parse", "HEAD"]);

    let before = resolve_at(&workspace, &repo, &first);
    let after = resolve_at(&workspace, &repo, &second);
    let again = resolve_at(&workspace, &repo, &first);

    assert_eq!(before.stage.env_sets, after.stage.env_sets);
    assert_ne!(
        operation_fingerprint(&before, &env_set("raze-env")),
        operation_fingerprint(&after, &env_set("raze-env")),
        "a layer item rebuilds when the import moves to another revision"
    );
    assert_eq!(
        operation_fingerprint(&before, &env_set("raze-env")),
        operation_fingerprint(&again, &env_set("raze-env")),
    );
    assert_eq!(
        operation_fingerprint(&before, &env_set("local-env")),
        operation_fingerprint(&after, &env_set("local-env")),
        "local items keep their fingerprint"
    );
    let _ = fs::remove_dir_all(root);
}
