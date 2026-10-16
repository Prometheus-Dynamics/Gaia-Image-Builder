//! `${project.commit}` / `${project.describe}`: the git identity of the
//! repository holding the build file.

use gaia_config::try_resolve_config;
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;
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

fn temp_dir(name: &str) -> TempDir {
    TempDir::new(&format!("gaia-project-git-{name}"))
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
    let scratch = temp_dir("repo");
    let repo = scratch.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "gaia@example.com"]);
    git(repo, &["config", "user.name", "Gaia Test"]);
    let build = repo.join("build.toml");
    fs::write(&build, BUILD).expect("build");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "recipe"]);
    git(repo, &["tag", "v1.4.0"]);
    let head = git(repo, &["rev-parse", "HEAD"]);

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
}

#[test]
fn uncommitted_tracked_changes_mark_the_commit_dirty() {
    let scratch = temp_dir("dirty");
    let repo = scratch.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "gaia@example.com"]);
    git(repo, &["config", "user.name", "Gaia Test"]);
    let build = repo.join("build.toml");
    fs::write(&build, BUILD).expect("build");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "recipe"]);
    let head = git(repo, &["rev-parse", "HEAD"]);
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
}

#[test]
fn project_tokens_outside_git_stay_unresolved() {
    let scratch = temp_dir("plain");
    let dir = scratch.path();
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
}

const INPUT_BUILD: &str = r#"
build_name = "slots"

[inputs.slot_commit]
kind = "string"
default = "${project.commit_short}"

[workspace]
root_dir = "."

[[stage.env_sets]]
id = "slot"
name = "slot"
entries = [["SLOT_ID", "slot-${input.slot_commit}"]]

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#;

fn selected_input<'a>(spec: &'a gaia_spec::ResolvedBuildSpec, name: &str) -> &'a str {
    spec.inputs
        .selected
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .expect("selected input")
}

/// An input default takes project tokens too, also when the build file is
/// in a linked git worktree.
#[test]
fn input_defaults_resolve_project_tokens_in_a_repo_and_its_worktree() {
    let scratch = temp_dir("inputs");
    let repo = scratch.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    fs::write(repo.join("build.toml"), INPUT_BUILD).expect("build");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "recipe"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let short = &head[..8];

    let worktree = scratch.path().join("worktree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().expect("utf-8"),
        ],
    );

    for build in [repo.join("build.toml"), worktree.join("build.toml")] {
        let spec = try_resolve_config(build.to_str().expect("utf-8")).expect("resolve");
        assert_eq!(
            selected_input(&spec, "slot_commit"),
            short,
            "{}",
            build.display()
        );
        assert_eq!(spec.stage.env_sets[0].entries[0].1, format!("slot-{short}"));
        assert!(
            spec.policy.interpolation.unresolved.is_empty(),
            "{:?}",
            spec.policy.interpolation.unresolved
        );
    }
}
