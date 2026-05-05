use super::command_signature;
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

    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        signature,
        format!("{}:timeout-2s", script.to_str().expect("script path"))
    );

    let _ = fs::remove_file(script);
}
