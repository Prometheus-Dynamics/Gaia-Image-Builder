//! The image checks on crafted trees: each check passes a good image and
//! fails a bad one with a message that names the missing or wrong item.
//! The binary check needs a C compiler; without one it says SKIPPED, unless
//! `GAIA_REQUIRE_TOOLS=1` is set, when it fails.

use gaia_check::{
    check_binaries, check_device_tree, check_kernel_config, check_modules, check_units,
};
use gaia_step::{Ctx, Store};
use gaia_types::Tree;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Writes `files` (relative path, text) under `dir`.
fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        fs::write(full, text).expect("write");
    }
}

/// Stores the tree at `dir` and returns it as an input to a node.
fn put(store_root: &Path, dir: &Path) -> (String, Tree) {
    let store = Store::open(store_root).expect("store");
    let object = store.put_tree(dir).expect("put");
    let tree = Tree {
        object: object.into(),
        entries: Vec::new(),
    };
    (store_root.display().to_string(), tree)
}

/// Runs a check on a tree at `dir`, returning the failure text if it fails.
fn fail_text(
    run: impl FnOnce(&str, Tree) -> Result<Tree, gaia_step::StepError>,
    dir: &Path,
    store_root: &Path,
) -> Option<String> {
    let (store, tree) = put(store_root, dir);
    run(&store, tree).err().map(|e| e.to_string())
}

fn ok(
    run: impl FnOnce(&str, Tree) -> Result<Tree, gaia_step::StepError>,
    dir: &Path,
    store_root: &Path,
) {
    if let Some(text) = fail_text(run, dir, store_root) {
        panic!("expected a pass, got: {text}");
    }
}

fn expect_failure(text: Option<String>, names: &[&str]) {
    let text = text.expect("the check should fail");
    for name in names {
        assert!(text.contains(name), "the failure names {name}:\n{text}");
    }
}

#[test]
fn modules_and_firmware_are_found_or_named() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    write(
        &image,
        &[
            (
                "lib/modules/6.1.0/kernel/drivers/net/foo_bar.ko.xz",
                "module",
            ),
            (
                "lib/modules/6.1.0/modules.builtin",
                "kernel/drivers/x/builtin_one.ko\n",
            ),
            ("lib/firmware/vendor/fw.bin", "firmware"),
        ],
    );
    let store = root.path().join("store");
    let good =
        r#"{"modules": ["foo-bar", "builtin-one"], "firmware": ["/lib/firmware/vendor/fw.bin"]}"#;
    ok(
        |s, t| check_modules(s, t, good, &Ctx::new()),
        &image,
        &store,
    );

    let missing = r#"{"modules": ["absent-module"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_modules(s, t, missing, &Ctx::new()),
            &image,
            &store,
        ),
        &["absent-module"],
    );

    let firmware = r#"{"firmware": ["/lib/firmware/vendor/none.bin"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_modules(s, t, firmware, &Ctx::new()),
            &image,
            &store,
        ),
        &["/lib/firmware/vendor/none.bin"],
    );
}

#[test]
fn a_firmware_link_out_of_the_tree_is_not_found() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    write(&image, &[("lib/firmware/x.bin", "x")]);
    fs::create_dir_all(image.join("lib/firmware/vendor")).expect("mkdir");
    std::os::unix::fs::symlink(
        "/nonexistent/host/fw.bin",
        image.join("lib/firmware/vendor/link.bin"),
    )
    .expect("link");
    let req = r#"{"firmware": ["/lib/firmware/vendor/link.bin"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_modules(s, t, req, &Ctx::new()),
            &image,
            &root.path().join("store"),
        ),
        &["/lib/firmware/vendor/link.bin"],
    );
}

#[test]
fn kernel_options_must_be_set_as_the_board_needs() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    write(
        &image,
        &[(
            "boot/config",
            "CONFIG_A=y\n# CONFIG_B is not set\nCONFIG_C=m\nCONFIG_D=y\n",
        )],
    );
    let store = root.path().join("store");
    let good = r#"{"file": "boot/config", "options": ["CONFIG_A=y", "CONFIG_B=n"]}"#;
    ok(
        |s, t| check_kernel_config(s, t, good, &Ctx::new()),
        &image,
        &store,
    );

    let wrong = r#"{"file": "boot/config", "options": ["CONFIG_C=y"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_kernel_config(s, t, wrong, &Ctx::new()),
            &image,
            &store,
        ),
        &["CONFIG_C is m", "the board needs y"],
    );

    let off = r#"{"file": "boot/config", "options": ["CONFIG_D=n"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_kernel_config(s, t, off, &Ctx::new()),
            &image,
            &store,
        ),
        &["CONFIG_D must not be set"],
    );

    let absent = r#"{"file": "boot/config", "options": ["CONFIG_E=y"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_kernel_config(s, t, absent, &Ctx::new()),
            &image,
            &store,
        ),
        &["CONFIG_E is missing"],
    );

    let no_file = r#"{"file": "boot/other", "options": ["CONFIG_A=y"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_kernel_config(s, t, no_file, &Ctx::new()),
            &image,
            &store,
        ),
        &["boot/other is not in the image"],
    );
}

/// A device tree blob with `/soc/serial@7e201000` and `/soc/i2c@7e804000`.
fn blob() -> Vec<u8> {
    fn be(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_be_bytes());
    }
    fn pad(out: &mut Vec<u8>) {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    fn begin(out: &mut Vec<u8>, name: &str) {
        be(out, 1);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        pad(out);
    }
    let mut structure = Vec::new();
    begin(&mut structure, "");
    begin(&mut structure, "soc");
    be(&mut structure, 3);
    be(&mut structure, 2);
    be(&mut structure, 0);
    structure.extend_from_slice(b"x\0");
    pad(&mut structure);
    begin(&mut structure, "serial@7e201000");
    be(&mut structure, 2);
    begin(&mut structure, "i2c@7e804000");
    be(&mut structure, 2);
    be(&mut structure, 2);
    be(&mut structure, 2);
    be(&mut structure, 9);

    let rsvmap = [0u8; 16];
    let mut out = Vec::new();
    let strings_at = 56 + structure.len();
    for word in [
        0xd00d_feedu32,
        0,
        56,
        strings_at as u32,
        40,
        17,
        16,
        0,
        0,
        structure.len() as u32,
    ] {
        be(&mut out, word);
    }
    out.extend_from_slice(&rsvmap);
    out.extend_from_slice(&structure);
    let total = u32::try_from(out.len()).expect("size");
    out[4..8].copy_from_slice(&total.to_be_bytes());
    out
}

#[test]
fn device_tree_nodes_must_exist() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    fs::create_dir_all(image.join("boot")).expect("mkdir");
    fs::write(image.join("boot/board.dtb"), blob()).expect("write");
    fs::write(image.join("boot/junk.dtb"), b"not a tree").expect("write");
    let store = root.path().join("store");

    let good = r#"{"blobs": [{"file": "boot/board.dtb", "nodes": ["/", "/soc/serial@7e201000", "/soc/i2c@7e804000"]}]}"#;
    ok(
        |s, t| check_device_tree(s, t, good, &Ctx::new()),
        &image,
        &store,
    );

    let missing = r#"{"blobs": [{"file": "boot/board.dtb", "nodes": ["/soc/uart@0"]}]}"#;
    expect_failure(
        fail_text(
            |s, t| check_device_tree(s, t, missing, &Ctx::new()),
            &image,
            &store,
        ),
        &["boot/board.dtb has no node /soc/uart@0"],
    );

    let junk = r#"{"blobs": [{"file": "boot/junk.dtb", "nodes": ["/"]}]}"#;
    expect_failure(
        fail_text(
            |s, t| check_device_tree(s, t, junk, &Ctx::new()),
            &image,
            &store,
        ),
        &["boot/junk.dtb is not a device tree blob"],
    );
}

#[test]
fn a_service_with_a_start_limit_fails_and_exemptions_pass() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    write(
        &image,
        &[
            (
                "usr/lib/systemd/system/fine.service",
                "[Service]\nRestart=always\n[Unit]\nStartLimitIntervalSec=0\n",
            ),
            (
                "usr/lib/systemd/system/limited.service",
                "[Service]\nRestart=always\n",
            ),
            (
                "usr/lib/systemd/system/exempt.service",
                "[Service]\nRestart=always\n",
            ),
            (
                "etc/systemd/system.conf",
                "[Manager]\n#DefaultStartLimitIntervalSec=5\n",
            ),
        ],
    );
    let store = root.path().join("store");
    let exempt = r#"{"exempt": ["exempt.service", "fine.service"]}"#;
    expect_failure(
        fail_text(
            |s, t| check_units(s, t, exempt, &Ctx::new()),
            &image,
            &store,
        ),
        &["limited.service has a start limit"],
    );
    let text = fail_text(
        |s, t| check_units(s, t, exempt, &Ctx::new()),
        &image,
        &store,
    )
    .expect("fails");
    assert!(
        !text.contains("exempt.service") && !text.contains("fine.service"),
        "{text}"
    );

    // The manager default of 0 removes the limit for every unit that does not set one.
    fs::write(
        image.join("etc/systemd/system.conf"),
        "[Manager]\nDefaultStartLimitIntervalSec=0\n",
    )
    .expect("write");
    ok(|s, t| check_units(s, t, "{}", &Ctx::new()), &image, &store);
}

#[test]
fn a_drop_in_that_sets_zero_clears_the_limit() {
    let root = tempfile::tempdir().expect("tempdir");
    let image = root.path().join("image");
    write(
        &image,
        &[
            (
                "etc/systemd/system/app.service",
                "[Unit]\nStartLimitIntervalSec=30s\n",
            ),
            (
                "etc/systemd/system/app.service.d/10-clear.conf",
                "[Unit]\nStartLimitIntervalSec=0\n",
            ),
            (
                "etc/systemd/system/other.service",
                "[Unit]\nStartLimitIntervalSec=1min 30s\n",
            ),
            (
                "etc/systemd/system/empty.service",
                "[Unit]\nStartLimitIntervalSec=\n",
            ),
        ],
    );
    let store = root.path().join("store");
    let text =
        fail_text(|s, t| check_units(s, t, "{}", &Ctx::new()), &image, &store).expect("fails");
    assert!(
        text.contains("other.service has a start limit: StartLimitIntervalSec=1min 30s"),
        "{text}"
    );
    assert!(
        !text.contains("app.service"),
        "a drop-in that sets 0 clears the limit:\n{text}"
    );
    // An empty value resets to systemd's default, which is a limit.
    assert!(
        text.contains("empty.service has a start limit"),
        "an empty value is the default:\n{text}"
    );
}

fn cc() -> bool {
    let found = Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and a C compiler (cc) is missing"
        );
        println!("SKIPPED: a C compiler (cc) is not installed");
    }
    found
}

fn compile(args: &[&str]) {
    let status = Command::new("cc").args(args).status().expect("cc");
    assert!(status.success(), "cc {args:?} failed");
}

/// A tree with a shared library, a program linked against it, and an
/// interpreter. `lib_missing` leaves the library out; `interp` names the
/// program's interpreter path, which the tree holds as an empty file unless
/// `interp_missing` is set.
fn binary_tree(
    scratch: &Path,
    image: &Path,
    lib_missing: bool,
    interp: &str,
    interp_missing: bool,
) {
    let src = scratch.join("src");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("dep.c"), "int dep_fn(void) { return 3; }\n").expect("write");
    fs::write(
        src.join("app.c"),
        "int dep_fn(void);\nvoid _start(void) { dep_fn(); for (;;) {} }\n",
    )
    .expect("write");
    let lib_dir = scratch.join("libs");
    fs::create_dir_all(&lib_dir).expect("mkdir");
    compile(&[
        "-shared",
        "-fPIC",
        "-nostdlib",
        "-Wl,-soname,libdep.so.1",
        "-o",
        &lib_dir.join("libdep.so.1").display().to_string(),
        &src.join("dep.c").display().to_string(),
    ]);
    fs::copy(lib_dir.join("libdep.so.1"), lib_dir.join("libdep.so")).expect("copy");
    let app = scratch.join("app");
    compile(&[
        "-nostdlib",
        "-Wl,-e,_start",
        "-o",
        &app.display().to_string(),
        &src.join("app.c").display().to_string(),
        &format!("-L{}", lib_dir.display()),
        "-ldep",
        &format!("-Wl,--dynamic-linker={interp}"),
    ]);
    fs::create_dir_all(image.join("usr/bin")).expect("mkdir");
    fs::create_dir_all(image.join("usr/lib")).expect("mkdir");
    fs::copy(&app, image.join("usr/bin/app")).expect("copy");
    if !lib_missing {
        fs::copy(
            lib_dir.join("libdep.so.1"),
            image.join("usr/lib/libdep.so.1"),
        )
        .expect("copy");
        std::os::unix::fs::symlink("libdep.so.1", image.join("usr/lib/libdep.so")).expect("link");
    }
    if !interp_missing {
        fs::create_dir_all(
            image.join(
                Path::new(interp)
                    .parent()
                    .expect("dir")
                    .strip_prefix("/")
                    .expect("abs"),
            ),
        )
        .expect("mkdir");
        fs::write(image.join(interp.trim_start_matches('/')), "").expect("write");
    }
}

#[test]
fn every_binary_has_its_interpreter_and_libraries() {
    if !cc() {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store");
    let interp = "/lib/ld-check-test.so.1";

    let good = root.path().join("good");
    let image = good.join("image");
    binary_tree(&good, &image, false, interp, false);
    ok(|s, t| check_binaries(s, t, &Ctx::new()), &image, &store);

    let no_lib = root.path().join("no-lib");
    let image = no_lib.join("image");
    binary_tree(&no_lib, &image, true, interp, false);
    expect_failure(
        fail_text(|s, t| check_binaries(s, t, &Ctx::new()), &image, &store),
        &["usr/bin/app needs the library libdep.so.1"],
    );

    let no_interp = root.path().join("no-interp");
    let image = no_interp.join("image");
    binary_tree(&no_interp, &image, false, interp, true);
    expect_failure(
        fail_text(|s, t| check_binaries(s, t, &Ctx::new()), &image, &store),
        &["usr/bin/app needs the program interpreter /lib/ld-check-test.so.1"],
    );
}
