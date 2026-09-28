pub mod support;

use gaia_config::try_resolve_config;
use gaia_spec::SourceDefinition;
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

fn write_build(root: &std::path::Path, orion_branch: &str) -> std::path::PathBuf {
    let build = root.join("cm5.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "git-lock-validation"

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[[sources]]
id = "orion"
kind = "git"
repo = "https://example.invalid/orion.git"
branch = "{orion_branch}"
pin = "locked"

[[sources]]
id = "orion-tools"
kind = "git"
repo = "https://example.invalid/orion"
tag = "v1"
"#,
            root = root.display()
        ),
    )
    .expect("build config");
    build
}

fn lock_contents(branch: &str) -> String {
    format!(
        "version = 1\n\n[[git]]\nsource = \"orion\"\nrepo = \"https://example.invalid/orion.git\"\nref = \"branch:{branch}\"\ncommit = \"{}\"\n",
        "a".repeat(40)
    )
}

#[test]
fn matching_lock_entry_pins_source_without_warnings() {
    let root = create_temp_workspace("gaia-validate-git-lock");
    let build = write_build(&root, "main");
    fs::write(root.join("cm5.gaia.lock"), lock_contents("main")).expect("lockfile");

    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    let SourceDefinition::Git(git) = &spec.sources[0].definition else {
        panic!("expected git source");
    };
    assert_eq!(git.locked_commit.as_deref(), Some("a".repeat(40).as_str()));

    let report = validate_spec(&spec);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git_lock_stale")
    );
    // Same repo (modulo `.git`) at a different ref is flagged.
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git_source_ref_divergence")
    );
}

#[test]
fn stale_lock_entry_is_ignored_and_warned() {
    let root = create_temp_workspace("gaia-validate-git-lock-stale");
    let build = write_build(&root, "release");
    fs::write(root.join("cm5.gaia.lock"), lock_contents("main")).expect("lockfile");

    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    let SourceDefinition::Git(git) = &spec.sources[0].definition else {
        panic!("expected git source");
    };
    assert_eq!(git.locked_commit, None);
    let report = validate_spec(&spec);
    let stale = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "git_lock_stale")
        .expect("stale lock warning");
    assert!(stale.message.contains("branch:main"));
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git_lockfile_invalid")
    );
}

#[test]
fn unreadable_lockfile_is_a_validation_error() {
    let root = create_temp_workspace("gaia-validate-git-lock-invalid");
    let build = write_build(&root, "main");
    fs::write(root.join("cm5.gaia.lock"), "version = [").expect("lockfile");

    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    let report = validate_spec(&spec);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git_lockfile_invalid")
    );
}

#[test]
fn no_lockfile_keeps_sources_floating() {
    let root = create_temp_workspace("gaia-validate-git-no-lock");
    let build = write_build(&root, "main");
    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    let SourceDefinition::Git(git) = &spec.sources[0].definition else {
        panic!("expected git source");
    };
    assert_eq!(git.locked_commit, None);
}
