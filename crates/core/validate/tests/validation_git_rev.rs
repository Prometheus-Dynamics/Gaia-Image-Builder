pub mod support;

use gaia_config::try_resolve_config;
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

/// The error codes `gaia validate` reports for a git source with `selector`.
fn git_source_errors(name: &str, selector: &str) -> Vec<String> {
    let root = create_temp_workspace(name);
    let build = root.join("build.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "git-rev-validation"

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[[sources]]
id = "atlas"
kind = "git"
repo = "https://example.invalid/atlas.git"
{selector}
"#,
            root = root.display()
        ),
    )
    .expect("build config");
    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    validate_spec(&spec)
        .diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.message.contains("'atlas'"))
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn an_empty_rev_is_an_error_naming_the_source() {
    assert_eq!(
        git_source_errors("gaia-validate-git-rev-empty", r#"rev = """#),
        ["git_selector_empty"]
    );
    assert_eq!(
        git_source_errors("gaia-validate-git-branch-empty", r#"branch = " ""#),
        ["git_selector_empty"]
    );
}

#[test]
fn a_pinned_rev_must_be_a_full_commit_sha() {
    for rev in ["b4de2e1", "main", "v1.0", "HEAD~1"] {
        assert_eq!(
            git_source_errors("gaia-validate-git-rev-short", &format!("rev = \"{rev}\"")),
            ["git_rev_not_full_sha"],
            "{rev}"
        );
    }
    let full = "b4de2e1c519d38bc4e3326cc4144e29eed3f47df";
    assert!(
        git_source_errors("gaia-validate-git-rev-full", &format!("rev = \"{full}\"")).is_empty()
    );
    let sha256 = "a".repeat(64);
    assert!(
        git_source_errors(
            "gaia-validate-git-rev-sha256",
            &format!("rev = \"{sha256}\"")
        )
        .is_empty()
    );
}

#[test]
fn a_floating_rev_may_be_abbreviated() {
    assert!(
        git_source_errors(
            "gaia-validate-git-rev-floating",
            "rev = \"b4de2e1\"\npin = \"floating\""
        )
        .is_empty()
    );
}
