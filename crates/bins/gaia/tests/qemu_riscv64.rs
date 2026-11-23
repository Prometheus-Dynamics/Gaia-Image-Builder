//! `scripts/qemu-riscv64-virt.sh`: the riscv64 QEMU tier's command line and
//! its refusals, read from `--print` and the exit codes. No guest is booted
//! here. Without qemu-system-riscv64 or its OpenSBI firmware the checks that
//! need them return early.

use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/qemu-riscv64-virt.sh")
}

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(script()).args(args).output().expect("runs");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn qemu_here() -> bool {
    let found = Command::new("sh")
        .args(["-c", "command -v qemu-system-riscv64"])
        .output()
        .is_ok_and(|out| out.status.success());
    found && Path::new("/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin").exists()
}

/// A wrong call is exit 2 with its reason, before any tool is looked for.
#[test]
fn a_wrong_call_is_refused() {
    let (code, _, err) = run(&[]);
    assert_eq!(code, Some(2));
    assert!(err.contains("usage:"), "{err}");
    let (code, _, err) = run(&["--kernel", "k", "--root", "r", "--initramfs", "i"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("--root and --initramfs: one of them"), "{err}");
    let (code, _, err) = run(&["--kernel", "k", "--no-such"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("usage:"), "{err}");
}

/// The CPU has the vector extension the K230 family's userspace is built
/// for, unless another is asked for; `--root` wants a root filesystem tree.
#[test]
fn the_cpu_has_the_vector_extension_unless_another_is_named() {
    if !qemu_here() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let kernel = dir.path().join("Image");
    std::fs::write(&kernel, b"not a kernel").expect("write");
    let kernel = kernel.to_str().expect("utf-8");
    let (code, out, err) = run(&["--kernel", kernel, "--print"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(
        out.starts_with("qemu-system-riscv64 -M virt -cpu rv64\\,v=true\\,vlen=128 -m 256M "),
        "{out}"
    );
    assert!(!out.contains("-initrd"), "{out}");
    let (code, out, _) = run(&["--kernel", kernel, "--cpu", "rv64", "--print"]);
    assert_eq!(code, Some(0));
    assert!(out.contains(" -cpu rv64 -m 256M "), "{out}");

    // A root tree: it has /sbin/init, and the command names it.
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).expect("mkdir");
    let root_arg = root.to_str().expect("utf-8");
    let (code, _, err) = run(&["--kernel", kernel, "--root", root_arg, "--print"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("has no /sbin/init"), "{err}");
    std::fs::create_dir_all(root.join("sbin")).expect("mkdir");
    std::fs::write(root.join("sbin/init"), b"").expect("write");
    let (code, out, err) = run(&["--kernel", kernel, "--root", root_arg, "--print"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(
        out.contains("-initrd") && out.contains("initramfs\\ of\\ "),
        "{out}"
    );
    let (code, _, err) = run(&["--kernel", "/no/such/Image", "--print"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("no kernel: /no/such/Image"), "{err}");
}
