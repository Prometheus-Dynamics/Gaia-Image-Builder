pub mod support;

use gaia_config::{resolve_config, try_resolve_config};
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Instant;
use support::write_temp_config;

#[test]
fn resolves_input_choices_from_git_tags() {
    let repo = unique_git_repo("gaia-config-dynamic-input-tags");
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "gaia@example.invalid"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    git(&repo, &["commit", "--allow-empty", "-m", "init"]);
    git(&repo, &["tag", "v2026.2.1"]);
    git(&repo, &["tag", "v2026.3.4"]);

    let config = write_temp_config(&format!(
        r#"
build_name = "dynamic-inputs"
version = "${{input.release_ref}}"

[inputs.release_ref]
description = "Release tag"
kind = "enum"
default_from = "first-choice"

[inputs.release_ref.choices_from]
kind = "git-tags"
repo = "{}"
pattern = "v2026*"
sort = "version-desc"
"#,
        repo.display()
    ));

    let spec = resolve_config(&config.display().to_string());
    let input = spec
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "release_ref")
        .expect("release_ref input");
    assert_eq!(input.choices, vec!["v2026.3.4", "v2026.2.1"]);
    assert_eq!(input.default.as_deref(), Some("v2026.3.4"));
    assert_eq!(spec.identity.version.as_deref(), Some("v2026.3.4"));
}

#[test]
fn resolves_dynamic_git_input_filters_source_fallback_and_cache() {
    let repo = unique_git_repo("gaia-config-dynamic-input-advanced");
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "gaia@example.invalid"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    git(&repo, &["commit", "--allow-empty", "-m", "init"]);
    git(&repo, &["branch", "release/v2026.3.4"]);
    git(&repo, &["branch", "release/v2026.4.0-rc1"]);
    git(&repo, &["branch", "release/v2026.4.0"]);
    git(&repo, &["branch", "scratch/local"]);

    let config = write_temp_config(&format!(
        r#"
build_name = "dynamic-inputs-advanced"
version = "${{input.release_branch}}"

[[sources]]
id = "photonvision"
kind = "git"
repo = "{}"
branch = "${{input.release_branch}}"

[inputs.release_branch]
description = "Release branch"
kind = "enum"
default_from = "latest-stable"

[inputs.release_branch.choices_from]
kind = "git-branches"
source = "photonvision"
include = ["release/v2026*"]
exclude = ["*-rc*"]
strip_prefix = "release/"
sort = "version-desc"
prefer_stable = true
version_scheme = "semver"
cache_ttl_seconds = 3600
"#,
        repo.display()
    ));

    let spec = resolve_config(&config.display().to_string());
    let input = spec
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "release_branch")
        .expect("release_branch input");
    assert_eq!(input.choices, vec!["v2026.4.0", "v2026.3.4"]);
    assert_eq!(input.default.as_deref(), Some("v2026.4.0"));
    assert_eq!(spec.identity.version.as_deref(), Some("v2026.4.0"));

    git(&repo, &["branch", "release/v2026.5.0"]);
    let cached = resolve_config(&config.display().to_string());
    let cached_input = cached
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "release_branch")
        .expect("release_branch input");
    assert_eq!(cached_input.choices, vec!["v2026.4.0", "v2026.3.4"]);

    let fallback_config = write_temp_config(
        r#"
build_name = "dynamic-inputs-fallback"
version = "${input.release_ref}"

[inputs.release_ref]
kind = "enum"
default_from = "latest-stable"

[inputs.release_ref.choices_from]
kind = "git-tags"
repo = "/definitely/not/a/repo"
fallback_choices = ["v2026.1.0-rc1", "v2026.0.0"]
sort = "version-desc"
prefer_stable = true
"#,
    );
    let fallback = resolve_config(&fallback_config.display().to_string());
    let fallback_input = fallback
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "release_ref")
        .expect("release_ref input");
    assert_eq!(fallback_input.choices, vec!["v2026.0.0", "v2026.1.0-rc1"]);
    assert_eq!(fallback_input.default.as_deref(), Some("v2026.0.0"));
}

#[test]
fn resolves_json_command_template_and_nonfatal_dynamic_inputs() {
    let root = unique_git_repo("gaia-config-dynamic-input-json-command");
    let json_path = root.join("releases.json");
    std::fs::write(
        &json_path,
        r#"{"releases":[{"tag":"v2026.2.1"},{"tag":"v2026.3.4-rc1"},{"tag":"v2026.3.4"}]}"#,
    )
    .expect("json choices");
    let script_path = root.join("choices.sh");
    std::fs::write(&script_path, "#!/bin/sh\nprintf 'dev\\nrelease\\n'\n").expect("script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&script_path)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("script permissions");
    }

    let config = write_temp_config(&format!(
        r#"
build_name = "dynamic-json-command"
version = "${{input.json_ref}}"
profile = "${{input.command_profile}}"

[inputs.json_ref]
kind = "enum"
default_from = "latest-stable"

[inputs.json_ref.choices_from]
kind = "json"
url = "{}"
json_path = "$.releases[*].tag"
exclude = ["*-rc*"]
selected_value_template = "refs/tags/${{choice}}"
sort = "version-desc"
version_scheme = "semver"

[inputs.command_profile]
kind = "enum"
default_from = "first-choice"

[inputs.command_profile.choices_from]
kind = "command"
command = ["{}"]
sort = "lexical-desc"

[inputs.optional_missing]
kind = "enum"
required = false

[inputs.optional_missing.choices_from]
kind = "git-tags"
repo = "/definitely/not/a/repo"
on_error = "warn"
"#,
        json_path.display(),
        script_path.display()
    ));

    let spec = resolve_config(&config.display().to_string());
    let json_input = spec
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "json_ref")
        .expect("json_ref input");
    assert_eq!(
        json_input.choices,
        vec!["refs/tags/v2026.3.4", "refs/tags/v2026.2.1"]
    );
    assert_eq!(
        spec.identity.version.as_deref(),
        Some("refs/tags/v2026.3.4")
    );

    let command_input = spec
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "command_profile")
        .expect("command_profile input");
    assert_eq!(command_input.choices, vec!["release", "dev"]);
    assert_eq!(spec.metadata.profile.as_deref(), Some("release"));

    let missing = spec
        .inputs
        .declared
        .iter()
        .find(|input| input.name == "optional_missing")
        .expect("optional_missing input");
    assert!(missing.choices.is_empty());
}

#[cfg(unix)]
#[test]
fn command_dynamic_input_honors_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_git_repo("gaia-config-dynamic-input-timeout");
    let script_path = root.join("choices.sh");
    std::fs::write(&script_path, "#!/bin/sh\nsleep 10\n").expect("script");
    let mut permissions = std::fs::metadata(&script_path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).expect("script permissions");

    let config = write_temp_config(&format!(
        r#"
build_name = "dynamic-command-timeout"

[inputs.profile]
kind = "enum"

[inputs.profile.choices_from]
kind = "command"
command = ["{}"]
timeout_seconds = 1
"#,
        script_path.display()
    ));

    let started = Instant::now();
    let error = try_resolve_config(&config.display().to_string()).expect_err("command timeout");
    let message = error.to_string();

    assert!(started.elapsed().as_secs() < 5, "{message}");
    assert!(message.contains("timed out after 1s"), "{message}");
}

fn unique_git_repo(prefix: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    std::fs::create_dir_all(&path).expect("git repo dir");
    path
}

fn git(repo: &PathBuf, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should start");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
