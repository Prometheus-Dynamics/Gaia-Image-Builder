//! An image with an OS, planned: the ZERO 3E demo example. The board's
//! kernel and bootloader reach the OS step as Buildroot config, pinned by
//! the lock and listed by `gaia plan`; the root filesystem is the OS's tree
//! merged with the component's layer. Plans only: tools are named, not run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// The examples and recipes, copied so a test can change them.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&repo().join("examples"), &dir.path().join("examples"));
    copy_dir(&repo().join("recipes"), &dir.path().join("recipes"));
    dir
}

fn plan(dir: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "plan",
        "examples/products/zero-3e-demo/product.toml",
        "--recipes",
        "recipes",
        "--tool",
        "make=/usr/bin/make",
        "--tool",
        "mkfs_erofs=/usr/bin/mkfs.erofs",
        "--tool",
        "veritysetup=/usr/sbin/veritysetup",
        "--tool",
        "bwrap=/usr/bin/bwrap",
        "--tool",
        "toolchain=/opt/rust",
        "--tool",
        "mke2fs=/usr/sbin/mke2fs",
        "--tool",
        "debugfs=/usr/sbin/debugfs",
    ];
    args.extend(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(&args)
        .current_dir(dir)
        .env("GAIA_STORE", dir.join("store"))
        .env("GAIA_JOBS", "4")
        .env_remove("GAIA_CPUS")
        .env_remove("GAIA_BUILDROOT_DL")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("runs");
    println!("$ gaia {}", args.join(" "));
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("read");
    assert!(text.contains(from), "{} has no {from:?}", path.display());
    std::fs::write(path, text.replace(from, to)).expect("write");
}

#[test]
fn the_plan_lists_the_config_the_board_gives() {
    let dir = project();
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        // The OS step, its sources and the merged root filesystem.
        "zero-3e-demo.os                         gaia.os.buildroot",
        "zero-3e-demo.buildroot.tree -> zero-3e-demo.os.source",
        "zero-3e.linux.tree -> zero-3e-demo.os.inputs0",
        "zero-3e-demo.os.layer.layer -> zero-3e-demo.rootfs.layers0",
        "zero-3e-demo.usb-gadget-network.place0.layer -> zero-3e-demo.rootfs.layers1",
        "zero-3e-demo.rootfs.tree -> zero-3e-demo::erofs.tree",
        "zero-3e-demo.rootfs.tree -> zero-3e-demo::root.tree",
        // The board's raw area, from the OS's bootloader output.
        "zero-3e-demo.os.bootloader -> zero-3e-demo.blob.u-boot.from",
        "zero-3e-demo.blob.u-boot.blob -> zero-3e-demo.blobs.blobs0",
        "zero-3e-demo.blobs.list -> zero-3e-demo::disk.blobs",
        "zero-3e-demo.disk-layout  gaia:disk-layout  gpt, board partitions uboot-env",
        // The slot, a file in the data partition.
        "zero-3e-demo::verity.image -> zero-3e-demo::slot.file",
        "zero-3e-demo::slot.tree -> zero-3e-demo::data.tree",
        // The config, by rule from the board, at the locked commits.
        "BR2_aarch64=y",
        "BR2_cortex_a55=y",
        "BR2_LINUX_KERNEL_CUSTOM_REPO_VERSION=\"d50d994bffa3c7c6db2d8bc4c5f0c55e3f46b0cd\"",
        "BR2_LINUX_KERNEL_USE_ARCH_DEFAULT_CONFIG=y",
        "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"@files.0@/linux.fragment @files.1@/linux.fragment\"",
        "BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"rockchip/rk3566-radxa-zero-3e\"",
        "BR2_LINUX_KERNEL_CUSTOM_DTS_DIR=\"@files.2@\"",
        "BR2_TARGET_UBOOT_BOARD_DEFCONFIG=\"radxa-zero-3-rk3566\"",
        "BR2_TARGET_UBOOT_CUSTOM_MAKEOPTS=\"ROCKCHIP_TPL=@zero-3e.rkbin@/bin/rk35/rk3566_ddr_1056MHz_v1.23.bin\"",
        "BR2_TARGET_ARM_TRUSTED_FIRMWARE_PLATFORM=\"rk3568\"",
        "built from LINUX=zero-3e.linux UBOOT=zero-3e.u-boot ARM_TRUSTED_FIRMWARE=zero-3e.tf-a",
        // A Rust component: its crates from its Cargo.lock, its target from
        // the board, its binary placed in the root filesystem.
        "hello-rust.vendor                       gaia.build.rust.vendor",
        "hello-rust.src.tree -> hello-rust.vendor.source",
        "hello-rust.vendor.vendor -> hello-rust.build.vendor",
        "hello-rust.root.rootfs -> hello-rust.build.root",
        "hello-rust.build.tree -> zero-3e-demo.hello-rust.place0.from",
        // The image's own fragment, last.
        "[image.0.os.fragment]",
        "BR2_TARGET_GENERIC_HOSTNAME=\"zero3e\"",
        "/etc/init.d/S41usb0 /etc/udhcpd.usb0.conf",
    ] {
        assert!(text.contains(line), "the plan has no line {line:?}");
    }
}

#[test]
fn a_rust_component_takes_its_target_from_the_board() {
    let dir = project();
    let out = plan(dir.path(), &["--json"]);
    assert!(out.status.success());
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let build = nodes
        .iter()
        .find(|n| n["label"] == "hello-rust.build")
        .expect("the build");
    println!("{}", build["consts"]);
    assert_eq!(
        build["consts"]["triple"]["value"],
        "aarch64-unknown-linux-musl"
    );
    assert_eq!(build["consts"]["cpu"]["value"], "cortex-a55");
    assert_eq!(build["consts"]["package"]["value"], "hello-rust");
}

#[test]
fn the_json_plan_has_the_os_config_as_a_host_input() {
    let dir = project();
    let out = plan(dir.path(), &["--json"]);
    assert!(out.status.success());
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let inputs = both["host_inputs"].as_array().expect("host inputs");
    let os = inputs
        .iter()
        .find(|i| i["port"] == "zero-3e-demo.os")
        .expect("the os config");
    assert_eq!(os["type"], "gaia:os-config");
    let names: Vec<&str> = os["value"]["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|f| f["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        [
            "board.target",
            "board.kernel",
            "board.bootloader",
            "image.0.os.fragment"
        ]
    );
    assert_eq!(os["value"]["overrides"][0]["package"], "LINUX");
    let host = inputs
        .iter()
        .find(|i| i["port"] == "buildroot-host")
        .expect("the host");
    assert_eq!(host["value"]["jobs"], 4);
}

#[test]
fn a_board_change_changes_the_config() {
    let dir = project();
    edit(
        &dir.path().join("examples/boards/zero-3e/board.toml"),
        "platform = \"rk3568\"",
        "platform = \"rk3588\"",
    );
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("BR2_TARGET_ARM_TRUSTED_FIRMWARE_PLATFORM=\"rk3588\""));
}

#[test]
fn a_big_little_pair_is_buildroots_option_and_the_little_core_for_rust() {
    let dir = project();
    edit(
        &dir.path().join("examples/boards/zero-3e/board.toml"),
        "cpu = \"cortex-a55\"",
        "cpu = \"cortex-a76.cortex-a55\"",
    );
    let out = plan(dir.path(), &["--json"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("BR2_cortex_a76_a55=y"),
        "Buildroot's name for the pair"
    );
    assert!(!text.contains("BR2_cortex_a76_cortex_a55"));
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let build = nodes
        .iter()
        .find(|n| n["label"] == "hello-rust.build")
        .expect("the build");
    assert_eq!(build["consts"]["cpu"]["value"], "cortex-a55");
}

#[test]
fn a_blob_name_that_is_no_make_variable_is_an_error_at_its_key() {
    let dir = project();
    edit(
        &dir.path().join("examples/boards/zero-3e/board.toml"),
        "ROCKCHIP_TPL = {",
        "tpl = {",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("board.toml"), "{err}");
    assert!(err.contains("bootloader.blobs.tpl"), "{err}");
    assert!(err.contains("not a make variable"), "{err}");
}

#[test]
fn an_unlocked_board_source_says_to_lock() {
    let dir = project();
    let lock = dir.path().join("examples/products/zero-3e-demo/gaia.lock");
    edit(
        &lock,
        "[source.\"zero-3e.linux\"]",
        "[source.\"zero-3e.other\"]",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("zero-3e.linux is not in the lock; lock it first"),
        "{err}"
    );
}

/// A recipe that signs the OS's bootloader files: the `bootloader` input
/// is the OS step's `bootloader` output.
#[test]
fn a_recipe_input_takes_the_os_boot_files_by_what_they_are() {
    let dir = project();
    edit(
        &dir.path().join("recipes/sd-image.toml"),
        "[inputs]\n",
        "[inputs]\nbootloader = [\"verity.veritysetup\"]\n",
    );
    // The types differ (an artifact into a string), so the planner refuses
    // it, naming the edge: the input reached the OS step's output.
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("`gaia.os.buildroot`.bootloader -> `gaia.verity`.veritysetup"),
        "{err}"
    );
}

#[test]
fn the_boards_raw_area_needs_an_image_with_an_os() {
    let dir = project();
    edit(
        &dir.path()
            .join("examples/products/zero-3e-demo/product.toml"),
        "os = { provider = \"buildroot\", source = \"buildroot\", fragments = [\"os.fragment\"] }\n",
        "",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("writes the board's `u-boot` area from the OS's bootloader output; this image has no `os`"),
        "{err}"
    );
}

#[test]
fn content_on_a_partition_in_the_table_is_an_error_at_its_key() {
    let dir = project();
    edit(
        &dir.path().join("examples/boards/rk35xx/family.toml"),
        "size = \"64K\"\n",
        "size = \"64K\"\ncontent = \"env.bin\"\n",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("partitions[1].content"), "{err}");
    assert!(err.contains("only an area outside the table"), "{err}");
}

#[test]
fn a_recipe_that_takes_os_needs_an_image_with_one() {
    let dir = project();
    edit(
        &dir.path()
            .join("examples/products/zero-3e-demo/product.toml"),
        "os = { provider = \"buildroot\", source = \"buildroot\", fragments = [\"os.fragment\"] }\n",
        "",
    );
    edit(
        &dir.path().join("recipes/sd-image.toml"),
        "blobs = [\"disk.blobs\"]\n",
        "bootloader = [\"verity.veritysetup\"]\n",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("boot files as `bootloader`; this image has no `os`"),
        "{err}"
    );
}
