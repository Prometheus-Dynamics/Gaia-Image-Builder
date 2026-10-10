use super::{COMMAND_SIGNATURE_TIMEOUT_SECONDS, command_signature};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_script(name: &str, contents: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let path = std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{name}-{nonce}.sh"));
    fs::create_dir_all(path.parent().expect("script parent")).expect("script parent");
    fs::write(&path, contents).expect("script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("script permissions");
    }
    path
}

#[test]
fn command_signature_returns_tool_output() {
    let script = temp_script(
        "gaia-command-signature-version",
        "#!/bin/sh\necho version-1\n",
    );

    assert_eq!(
        command_signature(script.to_str().expect("script path"), ["--version"]),
        format!("{}:version-1", script.to_str().expect("script path"))
    );

    let _ = fs::remove_file(script);
}

#[test]
fn command_signature_times_out_hanging_tools() {
    let script = temp_script(
        "gaia-command-signature-hang",
        "#!/bin/sh\nsleep 30\necho never\n",
    );
    let started = Instant::now();

    let signature = command_signature(script.to_str().expect("script path"), ["--version"]);

    assert!(started.elapsed() < Duration::from_secs(25));
    assert_eq!(
        signature,
        format!(
            "{}:timeout-{COMMAND_SIGNATURE_TIMEOUT_SECONDS}s",
            script.to_str().expect("script path")
        )
    );

    let _ = fs::remove_file(script);
}

#[test]
fn content_state_signature_ignores_path_and_mtime() {
    use super::content_state_signature;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-content-signature-{nonce}"));
    let first_dir = root.join("first");
    let second_dir = root.join("second");
    fs::create_dir_all(&first_dir).expect("first dir");
    fs::create_dir_all(&second_dir).expect("second dir");
    let first = first_dir.join(".config");
    let second = second_dir.join(".config");
    fs::write(&first, "BR2_x86_64=y\n").expect("first config");
    fs::write(&second, "BR2_x86_64=y\n").expect("second config");

    let before = content_state_signature(&first);
    let timestamp = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(&first)
        .expect("open")
        .set_modified(timestamp)
        .expect("set mtime");

    // Same contents at another path and mtime: same signature.
    assert_eq!(before, content_state_signature(&second));
    assert_eq!(before, content_state_signature(&first));
    assert!(before.starts_with("sha256:"));

    // A symlink to the same file (as used for a RAM tree) hashes its contents.
    #[cfg(unix)]
    {
        let link = root.join("link");
        std::os::unix::fs::symlink(&second, &link).expect("symlink");
        assert_eq!(before, content_state_signature(&link));
    }

    fs::write(&second, "BR2_x86_64=n\n").expect("changed config");
    assert_ne!(before, content_state_signature(&second));
    assert_eq!(
        content_state_signature(&root.join("absent")),
        "missing:absent"
    );
    let _ = fs::remove_dir_all(root);
}
