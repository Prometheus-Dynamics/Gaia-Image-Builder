//! A tool in the sandbox: what it can write, whether it reaches the
//! network, and whether the same inputs make the same output.
//!
//! The root filesystem here is this host's `/usr` with the usual top-level
//! links, standing in for an image's. Where bubblewrap or user namespaces
//! are missing the tests say SKIPPED, unless `GAIA_REQUIRE_SANDBOX=1`, when
//! they fail.

use gaia_sandbox::{Limits, Sandbox};
use gaia_step::{Ctx, KeyBuilder, Step, StepError, ToolOutput};
use gaia_store::{Digest, Store};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const BWRAP: &str = "/usr/bin/bwrap";

/// Whether a sandbox can run here.
fn available() -> bool {
    let ok = Path::new(BWRAP).exists()
        && std::process::Command::new(BWRAP)
            .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_SANDBOX").is_none(),
            "GAIA_REQUIRE_SANDBOX is set, and no sandbox runs here"
        );
        println!("SKIPPED: no bubblewrap with user namespaces here");
    }
    ok
}

/// A root filesystem: the top-level links of a merged-/usr system and an
/// `etc`, with the host's `/usr` bound in as an input.
fn rootfs(dir: &Path) -> PathBuf {
    let root = dir.join("rootfs");
    std::fs::create_dir_all(root.join("etc")).expect("mkdir");
    for (link, target) in [
        ("bin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("sbin", "usr/sbin"),
    ] {
        std::os::unix::fs::symlink(target, root.join(link)).expect("symlink");
    }
    std::fs::write(root.join("etc/image-file"), "from the image\n").expect("write");
    root
}

fn sandbox(dir: &Path) -> Sandbox {
    Sandbox::new(BWRAP, rootfs(dir)).input("/usr", "/usr")
}

/// Runs `script` with `/bin/sh` in `sandbox`, in a step of its own.
fn run(store: &Path, sandbox: Sandbox, script: &str) -> Result<(ToolOutput, PathBuf), StepError> {
    let store = Store::open(store).expect("store");
    let cx = Ctx::new();
    let key = sandbox
        .key(KeyBuilder::new("test.sandbox", 1).config("script", script))
        .finish();
    let out = std::sync::Mutex::new(None);
    let made = Step::new(&store, &cx, "test.sandbox", key).look_up_or_make(
        &[("out", Path::new("out"))],
        |work| {
            let dir = work.dir().join("out");
            std::fs::create_dir(&dir).map_err(|e| StepError::body("test", e))?;
            let tool = sandbox
                .output(&dir, "/out")
                .tool("/bin/sh")
                .map_err(|e| StepError::body("test", e))?;
            let output = work.run(&tool.arg("-c").arg(script))?;
            *out.lock().expect("lock") = Some(output);
            Ok(())
        },
    )?;
    let path = store.path(&made.outputs["out"]);
    let output = out.lock().expect("lock").take().expect("ran");
    Ok((output, path))
}

#[test]
fn a_tool_writes_only_its_output() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let input = dir.path().join("input");
    std::fs::create_dir(&input).expect("mkdir");
    std::fs::write(input.join("src.txt"), "source\n").expect("write");
    let script = r#"
        for p in /out /tmp / /etc /usr /in /proc/sys/kernel; do
            if touch "$p/written" 2>/dev/null; then echo "writable $p"; else echo "refused $p"; fi
        done
        echo hi > /out/hello
        cat /etc/image-file /in/src.txt > /out/read
        id -u
    "#;
    let (output, out) = run(
        &dir.path().join("store"),
        sandbox(dir.path()).input(&input, "/in"),
        script,
    )
    .expect("runs");
    println!("{}", output.stdout_tail.join("\n"));
    let writable: Vec<_> = output
        .stdout_tail
        .iter()
        .filter(|l| l.starts_with("writable"))
        .collect();
    assert_eq!(writable, ["writable /out", "writable /tmp"]);
    assert_eq!(output.stdout_tail.last().map(String::as_str), Some("0"));
    assert_eq!(
        std::fs::read_to_string(out.join("read")).expect("read"),
        "from the image\nsource\n"
    );
    // The host sees nothing but the output: the input, the image and /tmp
    // (a tmpfs) are untouched.
    assert_eq!(std::fs::read_dir(&input).expect("ls").count(), 1);
    assert!(
        !dir.path().join("rootfs/written").exists()
            && !dir.path().join("rootfs/etc/written").exists()
    );
    assert!(out.join("written").exists() && out.join("hello").exists());
}

#[test]
fn the_network_is_off_unless_declared() {
    if !available() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nhello\n",
            );
        }
    });
    let script = format!(
        "cat /proc/net/dev | tail -n +3 | cut -d: -f1 | tr -d ' ' | tr '\\n' ' '; echo; \
         if curl -s --max-time 5 http://127.0.0.1:{port}/ >/dev/null; then echo reached; else echo unreachable; fi"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let (off, _) = run(&dir.path().join("store"), sandbox(dir.path()), &script).expect("runs");
    println!("network off: {:?}", off.stdout_tail);
    assert_eq!(off.stdout_tail, ["lo ", "unreachable"]);
    let other = tempfile::tempdir().expect("tempdir");
    let (on, _) = run(
        &other.path().join("store"),
        sandbox(other.path()).network(true),
        &script,
    )
    .expect("runs");
    println!("network declared: {:?}", on.stdout_tail);
    assert_eq!(on.stdout_tail.last().map(String::as_str), Some("reached"));
}

#[test]
fn the_same_inputs_give_the_same_output() {
    if !available() {
        return;
    }
    let script = "sha256sum /etc/image-file > /out/sums; date +%s > /tmp/now; ls / > /out/top";
    let digest = |dir: &Path| -> Digest {
        let (_, out) = run(&dir.join("store"), sandbox(dir), script).expect("runs");
        gaia_store::tree_digest(&out).expect("digest")
    };
    let (one, two) = (
        tempfile::tempdir().expect("t"),
        tempfile::tempdir().expect("t"),
    );
    let (a, b) = (digest(one.path()), digest(two.path()));
    println!("two runs in two stores: {a} and {b}");
    assert_eq!(a, b);
}

#[test]
fn host_limits_apply_and_are_in_the_key() {
    if !available() {
        return;
    }
    let cpu = std::fs::read_to_string("/proc/self/status")
        .expect("status")
        .lines()
        .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
        .map(|l| l.trim().split([',', '-']).next().unwrap_or("0").to_string())
        .expect("cpus");
    let limits = Limits {
        cpus: Some((cpu.clone(), PathBuf::from("/usr/bin/taskset"))),
        jobs: Some(3),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let script =
        "grep Cpus_allowed_list /proc/self/status | tr -d '\\t'; echo \"$MAKEFLAGS $GAIA_JOBS\"";
    let base = sandbox(dir.path());
    let (output, _) = run(
        &dir.path().join("store"),
        base.clone().limits(limits.clone()),
        script,
    )
    .expect("runs");
    println!("limits: {:?}", output.stdout_tail);
    assert_eq!(
        output.stdout_tail,
        [format!("Cpus_allowed_list:{cpu}"), "-j3 3".to_string()]
    );
    let key = |sandbox: Sandbox| sandbox.key(KeyBuilder::new("k", 1)).finish();
    assert_ne!(key(base.clone()), key(base.clone().limits(limits)));
    assert_ne!(key(base.clone()), key(base.clone().network(true)));
    assert_ne!(key(base.clone()), key(base.clone().input("/x", "/in")));
}
