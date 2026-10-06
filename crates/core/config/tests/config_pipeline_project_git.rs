//! `${project.commit}` / `${project.describe}`: the git identity of the
//! repository holding the build file.

use gaia_config::try_resolve_config;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gaia-project-git-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

const BUILD: &str = r#"
build_name = "recipe"
version = "1.4.0+${project.describe}"

[workspace]
root_dir = "."

[[stage.env_sets]]
id = "image-version"
name = "image-version"
entries = [["IMAGE_COMMIT", "${project.commit}"]]

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#;

fn image_commit(spec: &gaia_spec::ResolvedBuildSpec) -> String {
    spec.stage.env_sets[0].entries[0].1.clone()
}

#[test]
fn project_tokens_resolve_to_the_build_file_repository() {
    let repo = temp_dir("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    let build = repo.join("build.toml");
    fs::write(&build, BUILD).expect("build");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "recipe"]);
    git(&repo, &["tag", "v1.4.0"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);

    let spec = try_resolve_config(build.to_str().expect("utf-8")).expect("resolve");
    assert_eq!(image_commit(&spec), head);
    assert_eq!(spec.identity.version.as_deref(), Some("1.4.0+v1.4.0"));
    assert!(
        !spec
            .policy
            .interpolation
            .unresolved
            .iter()
            .any(|unresolved| unresolved.token.starts_with("project.")),
        "{:?}",
        spec.policy.interpolation.unresolved
    );
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn uncommitted_tracked_changes_mark_the_commit_dirty() {
    let repo = temp_dir("dirty");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    let build = repo.join("build.toml");
    fs::write(&build, BUILD).expect("build");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "recipe"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(&build, format!("{BUILD}\n# local edit\n")).expect("edit");

    let spec = try_resolve_config(build.to_str().expect("utf-8")).expect("resolve");
    assert_eq!(image_commit(&spec), format!("{head}-dirty"));
    assert!(
        spec.identity
            .version
            .as_deref()
            .is_some_and(|version| version.ends_with("-dirty")),
        "{:?}",
        spec.identity.version
    );
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn project_tokens_outside_git_stay_unresolved() {
    let dir = temp_dir("plain");
    let build = dir.join("build.toml");
    fs::write(&build, BUILD).expect("build");

    let spec = try_resolve_config(build.to_str().expect("utf-8")).expect("resolve");
    assert_eq!(image_commit(&spec), "${project.commit}");
    assert!(
        spec.policy
            .interpolation
            .unresolved
            .iter()
            .any(|unresolved| unresolved.token == "project.commit")
    );
    let _ = fs::remove_dir_all(dir);
}
