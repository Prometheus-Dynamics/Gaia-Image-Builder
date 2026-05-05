use super::*;
use std::time::Instant;

#[cfg(unix)]
fn executable_script(name: &str, contents: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gaia-dynamic-input-test-{name}-{nonce}"));
    fs::create_dir_all(&dir).expect("script dir");
    let script = dir.join(name);
    let temp_script = dir.join(format!("{name}.tmp"));
    fs::write(&temp_script, contents).expect("script");
    let mut permissions = fs::metadata(&temp_script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&temp_script, permissions).expect("script permissions");
    fs::rename(&temp_script, &script).expect("publish script");
    std::thread::sleep(Duration::from_millis(10));
    script
}

#[cfg(unix)]
#[test]
fn git_dynamic_input_honors_timeout() {
    let fake_git = executable_script("git", "#!/bin/sh\nsleep 10\n");
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::GitTags,
        repo: "https://example.invalid/repo.git".into(),
        timeout_seconds: Some(1),
        ..RawInputChoicesFromConfig::default()
    };

    let started = Instant::now();
    let error =
        fetch_git_choices_with_program(&config, &config.repo, &fake_git).expect_err("git timeout");

    assert!(started.elapsed().as_secs() < 5, "{error}");
    assert!(error.contains("timed out after 1s"), "{error}");
}

#[cfg(unix)]
#[test]
fn curl_dynamic_input_honors_timeout() {
    let fake_curl = executable_script("curl", "#!/bin/sh\nsleep 10\n");
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Json,
        timeout_seconds: Some(1),
        ..RawInputChoicesFromConfig::default()
    };

    let started = Instant::now();
    let error = fetch_url_with_program(&config, "https://example.invalid/data.json", &fake_curl)
        .expect_err("curl timeout");

    assert!(started.elapsed().as_secs() < 5, "{error}");
    assert!(error.contains("timed out after 1s"), "{error}");
}

#[cfg(unix)]
#[test]
fn dynamic_command_error_diagnostics_are_bounded() {
    let script = executable_script(
        "noisy",
        "#!/bin/sh\nyes prefix | head -c 1200000 >&2\nprintf TAIL >&2\nexit 7\n",
    );
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Command,
        command: vec!["sh".into(), script.display().to_string()],
        timeout_seconds: Some(5),
        ..RawInputChoicesFromConfig::default()
    };

    let error = fetch_command_choices(&config).expect_err("command should fail");

    assert!(error.contains("TAIL"), "{error}");
    assert!(
        error.len() < 1_100_000,
        "diagnostic length: {}",
        error.len()
    );
}

#[test]
fn dynamic_command_cache_identity_includes_command() {
    let source_dir = temp_dir("command-cache-identity");
    let first = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Command,
        command: vec!["printf".into(), "one\n".into()],
        ..RawInputChoicesFromConfig::default()
    };
    let second = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Command,
        command: vec!["printf".into(), "two\n".into()],
        ..RawInputChoicesFromConfig::default()
    };

    assert_ne!(
        cache_path(&first, "", &source_dir),
        cache_path(&second, "", &source_dir)
    );
    assert_ne!(
        lock_path(&first, "", &source_dir),
        lock_path(&second, "", &source_dir)
    );
}

#[test]
fn dynamic_json_cache_identity_includes_url_and_json_path() {
    let source_dir = temp_dir("json-cache-identity");
    let first = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Json,
        url: Some("https://example.invalid/releases.json".into()),
        json_path: Some("$.stable[*]".into()),
        ..RawInputChoicesFromConfig::default()
    };
    let different_path = RawInputChoicesFromConfig {
        json_path: Some("$.nightly[*]".into()),
        ..first.clone()
    };
    let different_url = RawInputChoicesFromConfig {
        url: Some("https://example.invalid/other.json".into()),
        ..first.clone()
    };

    assert_ne!(
        cache_path(&first, "", &source_dir),
        cache_path(&different_path, "", &source_dir)
    );
    assert_ne!(
        cache_path(&first, "", &source_dir),
        cache_path(&different_url, "", &source_dir)
    );
    assert_ne!(
        lock_path(&first, "", &source_dir),
        lock_path(&different_path, "", &source_dir)
    );
}

#[test]
fn dynamic_cache_identity_includes_rendering_templates() {
    let source_dir = temp_dir("template-cache-identity");
    let first = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::GitTags,
        repo: "https://example.invalid/repo.git".into(),
        display_template: Some("release ${choice}".into()),
        selected_value_template: Some("refs/tags/${choice}".into()),
        ..RawInputChoicesFromConfig::default()
    };
    let different_display = RawInputChoicesFromConfig {
        display_template: Some("version ${choice}".into()),
        ..first.clone()
    };
    let different_selected = RawInputChoicesFromConfig {
        selected_value_template: Some("${choice}".into()),
        ..first.clone()
    };

    assert_ne!(
        cache_path(&first, &first.repo, &source_dir),
        cache_path(&different_display, &different_display.repo, &source_dir)
    );
    assert_ne!(
        cache_path(&first, &first.repo, &source_dir),
        cache_path(&different_selected, &different_selected.repo, &source_dir)
    );
}

#[test]
fn curl_auth_config_escapes_quotes_and_backslashes() {
    let path = write_curl_auth_config(r#"abc"def\ghi"#).expect("auth config");
    let contents = fs::read_to_string(&path).expect("auth config contents");
    let _ = fs::remove_file(path);

    assert_eq!(
        contents,
        "header = \"Authorization: Bearer abc\\\"def\\\\ghi\"\n"
    );
}

#[test]
fn curl_auth_config_rejects_newlines() {
    let error = write_curl_auth_config("abc\ndef").expect_err("newline should be rejected");

    assert!(error.contains("newline"), "{error}");
}

#[cfg(unix)]
#[test]
fn curl_auth_config_is_removed_after_curl_failure() {
    let marker = temp_dir("curl-auth-cleanup").join("auth-path");
    fs::create_dir_all(marker.parent().expect("marker parent")).expect("marker dir");
    let fake_curl = executable_script(
        "curl",
        &format!(
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--config" ]; then
        shift
        printf '%s' "$1" > '{}'
    fi
    shift
done
printf 'failed' >&2
exit 7
"#,
            marker.display()
        ),
    );
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Json,
        auth_env: Some("GAIA_TEST_DYNAMIC_AUTH".into()),
        ..RawInputChoicesFromConfig::default()
    };

    let error = fetch_url_with_auth_resolver(
        &config,
        "https://example.invalid/data.json",
        &fake_curl,
        |_| Some("secret-token".to_string()),
    )
    .expect_err("fake curl should fail");

    assert!(error.contains("curl failed"), "{error}");
    let auth_path = fs::read_to_string(&marker).expect("recorded auth path");
    assert!(
        !Path::new(&auth_path).exists(),
        "auth config should be removed: {auth_path}"
    );
}

#[cfg(unix)]
#[test]
fn curl_auth_config_is_removed_after_curl_timeout() {
    let marker = temp_dir("curl-auth-timeout-cleanup").join("auth-path");
    fs::create_dir_all(marker.parent().expect("marker parent")).expect("marker dir");
    let fake_curl = executable_script(
        "curl",
        &format!(
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--config" ]; then
        shift
        printf '%s' "$1" > '{}'
    fi
    shift
done
sleep 10
"#,
            marker.display()
        ),
    );
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Json,
        auth_env: Some("GAIA_TEST_DYNAMIC_AUTH_TIMEOUT".into()),
        timeout_seconds: Some(1),
        ..RawInputChoicesFromConfig::default()
    };

    let error = fetch_url_with_auth_resolver(
        &config,
        "https://example.invalid/data.json",
        &fake_curl,
        |_| Some("secret-token".to_string()),
    )
    .expect_err("fake curl should time out");

    assert!(error.contains("timed out after 1s"), "{error}");
    let auth_path = fs::read_to_string(&marker).expect("recorded auth path");
    assert!(
        !Path::new(&auth_path).exists(),
        "auth config should be removed: {auth_path}"
    );
}

#[cfg(unix)]
#[test]
fn missing_curl_auth_token_skips_auth_config() {
    let marker = temp_dir("curl-auth-missing").join("saw-config");
    fs::create_dir_all(marker.parent().expect("marker parent")).expect("marker dir");
    let fake_curl = executable_script(
        "curl",
        &format!(
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--config" ]; then
        printf yes > '{}'
    fi
    shift
done
printf '{{"ok":true}}'
"#,
            marker.display()
        ),
    );
    let config = RawInputChoicesFromConfig {
        kind: RawInputChoicesFromKind::Json,
        auth_env: Some("GAIA_TEST_DYNAMIC_AUTH_MISSING".into()),
        ..RawInputChoicesFromConfig::default()
    };

    let body = fetch_url_with_auth_resolver(
        &config,
        "https://example.invalid/data.json",
        &fake_curl,
        |_| None,
    )
    .expect("fake curl should succeed");

    assert_eq!(body, r#"{"ok":true}"#);
    assert!(!marker.exists(), "auth config should not be passed");
}

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("gaia-dynamic-input-test-{name}-{nonce}"))
}
