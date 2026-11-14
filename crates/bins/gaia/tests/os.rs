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
        // Three builds from the one Buildroot tree: the root filesystem,
        // the kernel and the bootloader, each with only its own sources.
        "zero-3e-demo.os                         gaia.os.buildroot",
        "zero-3e-demo.kernel                     gaia.os.buildroot",
        "zero-3e-demo.bootloader                 gaia.os.buildroot",
        // The host tools, built once from the whole config, and given to
        // each of the three.
        "zero-3e-demo.host-tools                 gaia.os.buildroot.host",
        "zero-3e-demo.host-tools.seed -> zero-3e-demo.os.seed",
        "zero-3e-demo.host-tools.seed -> zero-3e-demo.kernel.seed",
        "zero-3e-demo.host-tools.seed -> zero-3e-demo.bootloader.seed",
        "zero-3e-demo.buildroot.tree -> zero-3e-demo.os.source",
        "zero-3e-demo.buildroot.tree -> zero-3e-demo.kernel.source",
        "zero-3e-demo.buildroot.tree -> zero-3e-demo.bootloader.source",
        "zero-3e.linux.tree -> zero-3e-demo.kernel.inputs0",
        "zero-3e.u-boot.tree -> zero-3e-demo.bootloader.inputs0",
        // Each build takes the files it names, each alone: the kernel its
        // fragments, the bootloader its own, and neither the other's.
        "zero-3e-demo.os.files.0.tree -> zero-3e-demo.kernel.inputs1",
        "zero-3e-demo.os.files.3.tree -> zero-3e-demo.bootloader.inputs1",
        // The kernel build's modules, image and device trees are layers of
        // the root filesystem, at the paths Buildroot installs them to.
        "zero-3e-demo.kernel.modules -> zero-3e-demo.kernel.modules.from",
        "zero-3e-demo.kernel.kernel -> zero-3e-demo.kernel.image.from",
        "zero-3e-demo.kernel.device_trees -> zero-3e-demo.kernel.device-trees.from",
        "zero-3e-demo.os.layer.layer -> zero-3e-demo.rootfs.layers0",
        "zero-3e-demo.kernel.modules.layer -> zero-3e-demo.rootfs.layers1",
        "zero-3e-demo.usb-gadget-network.place0.layer -> zero-3e-demo.rootfs.layers2",
        "zero-3e-demo.rootfs.tree -> zero-3e-demo::root.tree",
        // The slot's tree is the same without the kernel's boot files.
        "zero-3e-demo.kernel.modules.layer -> zero-3e-demo.system.layers1",
        "zero-3e-demo.system.tree -> zero-3e-demo::erofs.tree",
        // The board's raw area, from the bootloader build's output.
        "zero-3e-demo.bootloader.bootloader -> zero-3e-demo.blob.u-boot.from",
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
        "inputs zero-3e.linux files.0 files.1 files.2, built from LINUX=zero-3e.linux",
        "built from UBOOT=zero-3e.u-boot ARM_TRUSTED_FIRMWARE=zero-3e.tf-a",
        // The root filesystem's build has neither.
        "zero-3e-demo.os  gaia:os-config  fragments board.target image.0.os.fragment, inputs files.4\n",
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
        .find(|i| i["port"] == "zero-3e-demo.os.kernel")
        .expect("the kernel's config");
    assert_eq!(os["type"], "gaia:os-config");
    let names: Vec<&str> = os["value"]["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|f| f["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        ["board.target", "board.kernel", "image.0.os.fragment"]
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

/// The kernel image and device trees are in `/boot` of the bring-up root
/// only: the slot's tree has the modules and not the boot files.
#[test]
fn the_slots_tree_has_no_kernel_boot_files() {
    let dir = project();
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("zero-3e-demo.kernel.image.layer -> zero-3e-demo.rootfs.layers"));
    assert!(!text.contains("zero-3e-demo.kernel.image.layer -> zero-3e-demo.system.layers"));
    assert!(!text.contains("zero-3e-demo.kernel.device-trees.layer -> zero-3e-demo.system.layers"));
}

#[test]
fn a_boards_overlays_are_built_by_the_kernel_build() {
    let dir = project();
    let board = dir.path().join("examples/boards/zero-3e/board.toml");
    edit(
        &board,
        "device_tree_dir = \"assets/dts\"",
        "device_tree_dir = \"assets/dts\"\noverlays = [\"overlays/first\", \"overlays/second-board\"]",
    );
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "BR2_LINUX_KERNEL_INTREE_DTSO_NAMES=\"overlays/first overlays/second-board\""
        ),
        "{text}"
    );
    edit(&board, "\"overlays/first\"", "\"overlays/first.dtbo\"");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kernel.overlays[0]"), "{err}");
    assert!(err.contains("with no extension"), "{err}");
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

/// A component states a kernel contract: a file in one of its sources. The
/// kernel's build gets that file alone (not the whole source), with the
/// board's storage drivers and the image's kind.
#[test]
fn a_components_kernel_contract_is_checked_in_the_kernels_build() {
    let dir = project();
    let component = dir
        .path()
        .join("examples/components/hello-rust/component.toml");
    let board = dir.path().join("examples/boards/zero-3e/board.toml");
    std::fs::write(
        dir.path()
            .join("examples/components/hello-rust/src/slot-kernel.toml"),
        "version = 2\n",
    )
    .expect("write");
    edit(
        &component,
        "[build]\n",
        "[requires]\nkernel_contract = { source = \"src\", path = \"slot-kernel.toml\" }\n\n[build]\n",
    );
    edit(
        &board,
        "device_tree_dir = \"assets/dts\"",
        "device_tree_dir = \"assets/dts\"\nstorage_drivers = [\"CONFIG_MMC_DW_ROCKCHIP\", \"CONFIG_MMC_SDHCI_OF_DWCMSHC\"]",
    );
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "zero-3e-demo.os.contract.hello-rust     gaia.tree.pick",
        "hello-rust.src.tree -> zero-3e-demo.os.contract.hello-rust.tree",
        "zero-3e-demo.os.contract.hello-rust.tree -> zero-3e-demo.kernel.inputs",
    ] {
        assert!(text.contains(line), "the plan has no line {line:?}\n{text}");
    }
    // Only the kernel's build has it.
    assert!(!text.contains("zero-3e-demo.os.contract.hello-rust.tree -> zero-3e-demo.os.inputs"));
    assert!(
        !text
            .contains("zero-3e-demo.os.contract.hello-rust.tree -> zero-3e-demo.bootloader.inputs")
    );

    let json = plan(dir.path(), &["--json"]);
    let both: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    let kernel = both["host_inputs"]
        .as_array()
        .expect("host inputs")
        .iter()
        .find(|i| i["port"] == "zero-3e-demo.os.kernel")
        .expect("the kernel's config");
    let contract = &kernel["value"]["kernel_contracts"][0];
    println!("{contract}");
    assert_eq!(contract["input"], "contract.hello-rust");
    assert_eq!(contract["path"], "slot-kernel.toml");
    assert_eq!(contract["asked_by"], "hello-rust");
    assert_eq!(contract["board"], "zero-3e");
    assert_eq!(contract["kind"], "disk");
    assert_eq!(
        contract["firmware"], "",
        "this board has no boot firmware of its own"
    );
    assert_eq!(
        contract["storage_drivers"][1],
        "CONFIG_MMC_SDHCI_OF_DWCMSHC"
    );

    // A driver not written as the kernel's config writes it.
    edit(&board, "\"CONFIG_MMC_DW_ROCKCHIP\"", "\"MMC_DW_ROCKCHIP\"");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kernel.storage_drivers"), "{err}");
    assert!(
        err.contains("`MMC_DW_ROCKCHIP` is not an option as the kernel's .config writes it"),
        "{err}"
    );
    edit(&board, "\"MMC_DW_ROCKCHIP\"", "\"CONFIG_MMC_DW_ROCKCHIP\"");

    // A source the component does not have.
    edit(
        &component,
        "source = \"src\", path = \"slot-kernel.toml\"",
        "source = \"lemnos\", path = \"slot-kernel.toml\"",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("requires.kernel_contract.source"), "{err}");
    assert!(err.contains("no source named `lemnos`"), "{err}");
}

/// A board states files that describe it to other programs, by name; a
/// component installs one by that name, whatever the board is.
#[test]
fn a_component_installs_the_boards_description_by_name() {
    let dir = project();
    let board_dir = dir.path().join("examples/boards/zero-3e");
    let board = board_dir.join("board.toml");
    let component = dir
        .path()
        .join("examples/components/usb-gadget-network/component.toml");
    let add_install = "[[install]]\nfrom = { description = \"daemon\" }\nto = \"/etc/daemon/board.toml\"\nmode = \"0644\"\n";
    let text = std::fs::read_to_string(&component).expect("read");
    std::fs::write(&component, format!("{text}\n{add_install}")).expect("write");

    // The board states none: an error at the install, naming both.
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    println!("{err}");
    assert!(err.contains("install[2].from.description"), "{err}");
    assert!(
        err.contains(
            "usb-gadget-network installs the board's description `daemon`, and the board zero-3e"
        ),
        "{err}"
    );
    assert!(
        err.contains("states none of that name under [descriptions]"),
        "{err}"
    );

    // Stated, and the file is not there: an error at the board's key.
    let text = std::fs::read_to_string(&board).expect("read");
    std::fs::write(
        &board,
        format!("{text}\n[descriptions]\ndaemon = \"daemon-board.toml\"\n"),
    )
    .expect("write");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("descriptions.daemon"), "{err}");

    // There: one file, installed where the component says.
    std::fs::write(board_dir.join("daemon-board.toml"), "id = \"zero-3e\"\n").expect("write");
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "zero-3e-demo.usb-gadget-network.description.daemon  gaia.source.input",
        "zero-3e-demo.usb-gadget-network.description.daemon.tree -> zero-3e-demo.usb-gadget-network.place1.from",
        "/etc/daemon/board.toml",
    ] {
        assert!(text.contains(line), "the plan has no line {line:?}\n{text}");
    }

    // A description is named by a component, not by the board itself.
    edit(
        &board,
        "ROCKCHIP_TPL = { source = \"rkbin\", path = \"bin/rk35/rk3566_ddr_1056MHz_v1.23.bin\" }",
        "ROCKCHIP_TPL = { description = \"daemon\" }",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "a board description is named by a component's install entry, not by the board"
        ),
        "{err}"
    );
}

/// A component generates files from the board's description with a tool
/// built from one of its sources: the plan has the tool's build for the
/// build machine, the run and the install, and nothing is built or run to
/// find what is wrong with it.
#[test]
fn a_component_generates_from_the_boards_description() {
    let dir = project();
    let board_dir = dir.path().join("examples/boards/zero-3e");
    let board = board_dir.join("board.toml");
    let component = dir
        .path()
        .join("examples/components/hello-rust/component.toml");
    let block = "[generate]\ntool = { source = \"src\", package = \"hello-rust\" }\ndescription = \"daemon\"\nargs = [\"tree\", \"--socs\", \"@source@/socs\", \"@description@\", \"-o\", \"@out@/board.dts\", \"--modprobe\", \"@out@/modprobe.conf\"]\noutputs = [\"board.dts\", \"modprobe.conf\"]\n\n[[install]]\nfrom = { generated = \"modprobe.conf\" }\nto = \"/etc/modprobe.d/board.conf\"\nmode = \"0644\"\n";
    let original = std::fs::read_to_string(&component).expect("read");
    std::fs::write(&component, format!("{original}\n{block}")).expect("write");

    // The board states no such description: an error at the generator.
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    println!("{err}");
    assert!(err.contains("generate.description"), "{err}");
    assert!(
        err.contains(
            "hello-rust generates from the board's description `daemon`, and the board zero-3e"
        ),
        "{err}"
    );

    let text = std::fs::read_to_string(&board).expect("read");
    std::fs::write(
        &board,
        format!("{text}\n[descriptions]\ndaemon = \"daemon-board.toml\"\n"),
    )
    .expect("write");
    std::fs::write(board_dir.join("daemon-board.toml"), "id = \"zero-3e\"\n").expect("write");
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    // The step list's columns are as wide as its longest name: compared
    // with runs of spaces as one.
    let text = String::from_utf8_lossy(&out.stdout)
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    for line in [
        // The tool, built as any Rust component is, and its crates.
        "hello-rust.tool gaia.build.rust",
        "hello-rust.tool.vendor gaia.build.rust.vendor",
        "hello-rust.tool.vendor.vendor -> hello-rust.tool.vendor",
        // The run: the tool, its source, the board's description.
        "zero-3e-demo.hello-rust.generate gaia.generate",
        "hello-rust.tool.tree -> zero-3e-demo.hello-rust.generate.tool",
        "hello-rust.src.tree -> zero-3e-demo.hello-rust.generate.source",
        "zero-3e-demo.hello-rust.description.daemon.tree -> zero-3e-demo.hello-rust.generate.description",
        // It runs in the image it was built in.
        "hello-rust.root.rootfs -> hello-rust.tool.root",
        "hello-rust.root.rootfs -> zero-3e-demo.hello-rust.generate.root",
        // A generated file, installed.
        "zero-3e-demo.hello-rust.generate.tree -> zero-3e-demo.hello-rust.place",
        "/etc/modprobe.d/board.conf",
    ] {
        assert!(text.contains(line), "the plan has no line {line:?}\n{text}");
    }
    let json = plan(dir.path(), &["--json"]);
    let both: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let find = |label: &str| {
        nodes
            .iter()
            .find(|n| n["label"] == label)
            .unwrap_or_else(|| panic!("no node {label}"))
    };
    let tool = find("hello-rust.tool");
    // For the build machine, not the board, and tuned for no CPU.
    let triple = tool["consts"]["triple"]["value"].as_str().expect("triple");
    assert!(triple.ends_with("-unknown-linux-gnu"), "{triple}");
    assert!(triple.starts_with(std::env::consts::ARCH), "{triple}");
    assert_eq!(tool["consts"]["cpu"]["value"], "");
    // The component's own build is still the board's.
    assert_eq!(
        find("hello-rust.build")["consts"]["triple"]["value"],
        "aarch64-unknown-linux-musl"
    );
    let run = find("zero-3e-demo.hello-rust.generate");
    assert_eq!(run["consts"]["program"]["value"], "bin/hello-rust");
    assert_eq!(run["consts"]["description_name"]["value"], "daemon");
    assert_eq!(
        run["consts"]["outputs"]["value"],
        "[\"board.dts\",\"modprobe.conf\"]"
    );

    // What a plan refuses, each at its key.
    let cases = [
        (
            "\"@source@/socs\"",
            "\"@sources@/socs\"",
            "generate.args[2]",
            "names @sources@; the names are @source@, @description@, @out@, @slot@",
        ),
        (
            "from = { generated = \"modprobe.conf\" }",
            "from = { generated = \"udev.rules\" }",
            ".from.generated",
            "`udev.rules` is in no declared output; [generate] declares board.dts, modprobe.conf",
        ),
        (
            "tool = { source = \"src\"",
            "tool = { source = \"lemnos\"",
            "generate.tool.source",
            "no source named `lemnos`",
        ),
        (
            "\"@out@/board.dts\"",
            "\"@out@/@slot@.dts\"",
            "generate.args",
            "names @slot@, and the image zero-3e-demo sets no `slot` in its settings",
        ),
        (
            "outputs = [\"board.dts\", \"modprobe.conf\"]",
            "outputs = [\"boot/board.dts\", \"modprobe.conf\"]",
            "generate.outputs[0]",
            "is not a name at the top of the output directory",
        ),
    ];
    for (from, to, key, message) in cases {
        std::fs::write(
            &component,
            format!("{original}\n{}", block.replace(from, to)),
        )
        .expect("write");
        let out = plan(dir.path(), &[]);
        assert_eq!(out.status.code(), Some(2), "{to}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(key), "{to}: {err}");
        assert!(err.contains(message), "{to}: {err}");
    }
}

/// An image with an OS makes its slot with that build's own tools: the
/// fragment lines that provide them, and a compression they write, are
/// held at plan time, before anything is built.
#[test]
fn a_slot_needs_the_builds_own_tools_and_a_compression_they_write() {
    let dir = project();
    let product = dir
        .path()
        .join("examples/products/zero-3e-demo/product.toml");
    let fragment = dir
        .path()
        .join("examples/products/zero-3e-demo/os.fragment");
    assert!(plan(dir.path(), &[]).status.success());

    edit(
        &product,
        "id = \"zero-3e-demo\"",
        "id = \"zero-3e-demo\"\ncompression = \"zstd\"",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].settings.compression"), "{err}");
    assert!(
        err.contains("sets erofs compression `zstd`, which its build's own mkfs.erofs does not write: Buildroot builds the host erofs-utils with lz4, lz4hc, lzma and without zstd")
            && err.contains("`zstd` needs a host erofs-utils built with it, which this builder does not have yet"),
        "{err}"
    );
    edit(
        &product,
        "compression = \"zstd\"",
        "compression = \"lz4hc\"",
    );
    assert!(plan(dir.path(), &[]).status.success());

    for (line, program) in [
        ("BR2_PACKAGE_HOST_CRYPTSETUP=y\n", "veritysetup"),
        ("BR2_PACKAGE_HOST_EROFS_UTILS=y\n", "mkfs.erofs"),
    ] {
        edit(&fragment, line, "");
        let out = plan(dir.path(), &[]);
        assert_eq!(out.status.code(), Some(2), "{line}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("images[0].os.fragments"), "{err}");
        assert!(
            err.contains(&format!(
                "with the build's own {program}, and no OS fragment of the image has `{}`: add it",
                line.trim()
            )),
            "{err}"
        );
    }
}

/// An image that names an initramfs with a recipe that takes none would
/// ship those components in no output: refused, by name.
#[test]
fn an_initramfs_for_a_recipe_that_takes_none_is_refused() {
    let dir = project();
    let product = dir
        .path()
        .join("examples/products/zero-3e-demo/product.toml");
    edit(
        &product,
        "components = [\"usb-gadget-network\", \"extlinux\", \"hello-rust\"]",
        "components = [\"usb-gadget-network\", \"hello-rust\"]\ninitramfs = [\"extlinux\"]",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].initramfs"), "{err}");
    assert!(
        err.contains(
            "names `initramfs` (extlinux), and its recipe sd-image takes none of it (a recipe takes it as its input initramfs): it would be in no output"
        ),
        "{err}"
    );
}

/// The same for every key an image names things under for its recipe: data
/// for a recipe that takes no data, components and an OS for a recipe that
/// takes only data.
#[test]
fn what_an_image_names_and_its_recipe_does_not_take_is_refused() {
    let dir = project();
    let product = dir
        .path()
        .join("examples/products/zero-3e-demo/product.toml");
    edit(
        &product,
        "recipe = \"sd-image\"",
        "data = [\"hello-rust\"]\nrecipe = \"slot\"",
    );
    edit(&product, "id = \"zero-3e-demo\"", "device = \"/dev/x\"");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].data"), "{err}");
    assert!(
        err.contains("names `data` (hello-rust), and its recipe slot takes none of it"),
        "{err}"
    );
    // A recipe that takes only data: the components and the OS are in no output.
    edit(&product, "recipe = \"slot\"", "recipe = \"data-ext4\"");
    edit(&product, "device = \"/dev/x\"", "");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].components"), "{err}");
    assert!(
        err.contains("its recipe data-ext4 takes none of it (a recipe takes it as its input build, system, source)"),
        "{err}"
    );
}

/// A recipe takes what the image's generator wrote by role, the board's
/// base command line and the OS build's host tools: no file name of the
/// generator's is in the recipe or the product.
#[test]
fn a_recipe_takes_the_generators_files_by_role() {
    let dir = project();
    let board_dir = dir.path().join("examples/boards/zero-3e");
    let board = board_dir.join("board.toml");
    let product = dir
        .path()
        .join("examples/products/zero-3e-demo/product.toml");
    let component = dir
        .path()
        .join("examples/components/hello-rust/component.toml");
    let block = "[generate]\ntool = { source = \"src\", package = \"hello-rust\" }\ndescription = \"daemon\"\nargs = [\"tree\", \"@description@\", \"--slot\", \"@slot@\", \"@out@\"]\noutputs = [\"overlay.dts\", \"words\", \"manifest-boot.json\"]\n\n[generate.provides]\nboard-overlay = \"overlay.dts\"\nmanifest-boot = \"manifest-boot.json\"\ncmdline = \"words/cmdline.txt\"\ncmdline-slot = \"words/slot.txt\"\n\n[generate.image]\nfit = [\"part\", \"--min-size\", \"--payload\", \"@payload@\"]\nargs = [\"part\", \"@description@\", \"--slot\", \"@slot@\", \"--payload\", \"@payload@\", \"--size\", \"@size@\", \"--out\", \"@out@\"]\nverify = [\"part\", \"--verify\", \"@out@\"]\n";
    let original = std::fs::read_to_string(&component).expect("read");
    std::fs::write(&component, format!("{original}\n{block}")).expect("write");
    let text = std::fs::read_to_string(&board).expect("read");
    std::fs::write(
        &board,
        format!("{text}\n[descriptions]\ndaemon = \"daemon-board.toml\"\n"),
    )
    .expect("write");
    std::fs::write(board_dir.join("daemon-board.toml"), "id = \"zero-3e\"\n").expect("write");
    edit(
        &board,
        "[kernel]\n",
        "[kernel]\ncmdline = \"console=ttyS2,1500000 rootwait\"\n",
    );
    edit(&product, "recipe = \"sd-image\"", "recipe = \"slot-boot\"");
    // One component goes in the initramfs, not the root filesystem.
    edit(
        &product,
        "components = [\"usb-gadget-network\", \"extlinux\", \"hello-rust\"]",
        "components = [\"usb-gadget-network\", \"hello-rust\"]\ninitramfs = [\"extlinux\"]",
    );
    edit(
        &product,
        "id = \"zero-3e-demo\"",
        "slot = \"base\"\ndevice = \"/dev/mmcblk0p3\"\nboot_size = 67108864",
    );

    let out = plan(dir.path(), &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let text = String::from_utf8_lossy(&out.stdout)
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    for line in [
        "zero-3e-demo.hello-rust.generate.tree -> zero-3e-demo::overlay.source",
        "zero-3e-demo.hello-rust.generate.tree -> zero-3e-demo::cmdline.generated",
        "hello-rust.src.tree -> zero-3e-demo::overlay.contract",
        "zero-3e-demo.host-tools.seed -> zero-3e-demo::overlay.tools",
        "zero-3e-demo::verity.table -> zero-3e-demo::cmdline.table",
        // The initramfs: its component's files, merged apart from the root.
        "zero-3e-demo.extlinux-zero-3e.place0.layer -> zero-3e-demo.initramfs.layers0",
        "zero-3e-demo.initramfs.tree -> zero-3e-demo::initramfs.tree",
        // The boot files together, each under the name of what it is.
        "zero-3e-demo.kernel.kernel -> zero-3e-demo::as_kernel.from",
        "zero-3e-demo.kernel.device_trees -> zero-3e-demo::as_device_tree.from",
        "zero-3e-demo::initramfs.archive -> zero-3e-demo::as_initramfs.from",
        "zero-3e-demo::cmdline.cmdline -> zero-3e-demo::as_cmdline.from",
        "zero-3e-demo::overlay.overlay -> zero-3e-demo::as_overlay.from",
        "zero-3e-demo::as_overlay.tree -> host.zero-3e-demo.payload",
        // The generator's tool again, on the payload: the boot partition.
        "zero-3e-demo::as_overlay.tree -> zero-3e-demo::boot.payload",
        "hello-rust.tool.tree -> zero-3e-demo::boot.tool",
        "hello-rust.root.rootfs -> zero-3e-demo::boot.root",
        "hello-rust.src.tree -> zero-3e-demo::boot.source",
        "zero-3e-demo.hello-rust.description.daemon.tree -> zero-3e-demo::boot.description",
        "zero-3e-demo::boot.image -> host.zero-3e-demo.boot",
    ] {
        assert!(text.contains(line), "the plan has no line {line:?}\n{text}");
    }
    assert!(
        !text.contains("zero-3e-demo.extlinux-zero-3e.place0.layer -> zero-3e-demo.rootfs"),
        "the initramfs's component is in the root filesystem too\n{text}"
    );
    let json = plan(dir.path(), &["--json"]);
    let both: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let consts = |label: &str| {
        &nodes
            .iter()
            .find(|n| n["label"] == label)
            .unwrap_or_else(|| panic!("no node {label}"))["consts"]
    };
    let overlay = consts("zero-3e-demo::overlay");
    assert_eq!(overlay["source_path"]["value"], "overlay.dts");
    assert_eq!(overlay["stated_path"]["value"], "manifest-boot.json");
    assert_eq!(
        overlay["contract_path"]["value"],
        "contracts/overlay-dtc.toml"
    );
    let name = overlay["base_name"]["value"].as_str().expect("a name");
    assert!(name.ends_with(".dtb") && !name.contains('/'), "{name}");
    assert_eq!(
        consts("zero-3e-demo::as_device_tree")["path"]["value"],
        overlay["base_name"]["value"],
        "the device tree placed is the one the overlay was proven on"
    );
    assert_eq!(
        consts("zero-3e-demo::as_overlay")["role"]["value"],
        "board-overlay"
    );
    let boot = consts("zero-3e-demo::boot");
    assert_eq!(boot["program"]["value"], "bin/hello-rust");
    assert_eq!(boot["description_name"]["value"], "daemon");
    assert_eq!(boot["slot"]["value"], "base");
    assert_eq!(boot["size"]["value"], 67_108_864);
    let runs: serde_json::Value =
        serde_json::from_str(boot["runs"]["value"].as_str().expect("runs")).expect("json");
    assert_eq!(runs["verify"][1], "--verify");
    assert_eq!(runs["args"][9], "@out@");
    let cmdline = consts("zero-3e-demo::cmdline");
    assert_eq!(cmdline["base"]["value"], "console=ttyS2,1500000 rootwait");
    assert_eq!(cmdline["words_path"]["value"], "words/cmdline.txt");
    assert_eq!(cmdline["slot_path"]["value"], "words/slot.txt");
    assert_eq!(cmdline["slot"]["value"], "base");
    // The generator is run for the same slot.
    assert_eq!(
        consts("zero-3e-demo.hello-rust.generate")["slot"]["value"],
        "base"
    );

    // What a plan refuses, each at its key.
    let cases = [
        (
            "cmdline-slot = \"words/slot.txt\"\n",
            "",
            "generate.provides",
            "recipe slot-boot takes the generated file of the role `cmdline-slot`; hello-rust provides board-overlay, cmdline, manifest-boot",
        ),
        (
            "cmdline = \"words/cmdline.txt\"",
            "cmdline = \"other/cmdline.txt\"",
            "generate.provides.cmdline",
            "`other/cmdline.txt` is in no declared output; [generate] declares overlay.dts, words, manifest-boot.json",
        ),
        (
            "\"--payload\", \"@payload@\", \"--size\"",
            "\"--payload\", \"@payloads@\", \"--size\"",
            "generate.image.args[5]",
            "names @payloads@; the names are @source@, @description@, @payload@, @slot@, @size@, @out@",
        ),
        (
            "\"--out\", \"@out@\"]",
            "\"--out\"]",
            "generate.image.args",
            "no argument names @out@, the file the tool writes",
        ),
        (
            "board-overlay = ",
            "Board_Overlay = ",
            "generate.provides.Board_Overlay",
            "a role is lowercase letters, digits and `-`",
        ),
    ];
    for (from, to, key, message) in cases {
        std::fs::write(
            &component,
            format!("{original}\n{}", block.replace(from, to)),
        )
        .expect("write");
        let out = plan(dir.path(), &[]);
        assert_eq!(out.status.code(), Some(2), "{to}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(key), "{to}: {err}");
        assert!(err.contains(message), "{to}: {err}");
    }
    // The recipe has the generator's tool write an image, and the component
    // says nothing of how.
    let cut = block.split("\n[generate.image]").next().expect("the block");
    std::fs::write(&component, format!("{original}\n{cut}\n")).expect("write");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("generate.image"), "{err}");
    assert!(
        err.contains("recipe slot-boot has hello-rust's tool write an image; the component states no [generate.image]"),
        "{err}"
    );
    // A component in both trees, and a recipe that takes an initramfs from
    // an image that names none.
    std::fs::write(&component, format!("{original}\n{block}")).expect("write");
    edit(
        &product,
        "initramfs = [\"extlinux\"]",
        "initramfs = [\"hello-rust\"]",
    );
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].initramfs[0]"), "{err}");
    assert!(err.contains("`hello-rust` is in `components` too"), "{err}");
    edit(&product, "initramfs = [\"hello-rust\"]\n", "");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("recipe slot-boot takes the image's initramfs tree; this image names no components under `initramfs`"),
        "{err}"
    );
    edit(
        &product,
        "components = [\"usb-gadget-network\", \"hello-rust\"]",
        "components = [\"usb-gadget-network\", \"hello-rust\"]\ninitramfs = [\"extlinux\"]",
    );
    // No generator in the image at all.
    std::fs::write(&component, &original).expect("write");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("from the image's one component with a [generate]; this image has 0"),
        "{err}"
    );
    // A command line of two lines.
    edit(&board, "1500000 rootwait\"", "1500000\\nrootwait\"");
    let out = plan(dir.path(), &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kernel.cmdline"), "{err}");
    assert!(err.contains("more than one line"), "{err}");
}

/// A board whose kernel starts from a complete configuration, a saved
/// `.config`, in place of a defconfig.
#[test]
fn a_boards_kernel_starts_from_a_complete_configuration() {
    let dir = project();
    let board_dir = dir.path().join("examples/boards/zero-3e");
    let board = board_dir.join("board.toml");
    edit(
        &board,
        "device_tree_dir = \"assets/dts\"",
        "device_tree_dir = \"assets/dts\"\nconfig = \"saved.config\"",
    );
    // The family states a defconfig: two starting points.
    std::fs::write(board_dir.join("saved.config"), "CONFIG_X=y\n").expect("write");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kernel.config"), "{err}");
    assert!(err.contains("two starting points"), "{err}");

    let family = dir.path().join("examples/boards/rk35xx/family.toml");
    let text = std::fs::read_to_string(&family).expect("read");
    let without: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("defconfig ="))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(without, text, "the family states a defconfig");
    std::fs::write(&family, without).expect("write");
    let out = plan(dir.path(), &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("BR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y"),
        "{text}"
    );
    assert!(
        text.contains("BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE=\"@files.")
            && text.contains("/saved.config\""),
        "{text}"
    );
    assert!(!text.contains("BR2_LINUX_KERNEL_USE_ARCH_DEFAULT_CONFIG=y"));

    // A file that is not there is an error at the board's key.
    std::fs::remove_file(board_dir.join("saved.config")).expect("remove");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kernel.config"), "{err}");
}

/// A kernel with nothing to start its configuration from is an error where
/// the plan is made: Buildroot would stop on it much later.
#[test]
fn a_kernel_with_no_starting_configuration_is_an_error_in_the_plan() {
    let dir = project();
    let family = dir.path().join("examples/boards/rk35xx/family.toml");
    let text = std::fs::read_to_string(&family).expect("read");
    let without: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("defconfig ="))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&family, without).expect("write");
    let out = plan(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("board.toml: kernel:"), "{err}");
    assert!(
        err.contains("the board zero-3e states no configuration for its kernel to start from"),
        "{err}"
    );
}

/// A product built from its own directory by a bare file name: the
/// directory of a fragment named with no directory is `.`, not an empty
/// path, which no step can read. And what a fragment names beside itself
/// is an input by itself: a file as that file alone, a directory as that
/// directory, and only a bare `@here@` as the fragment's whole directory.
#[test]
fn a_product_named_with_no_directory_has_its_fragments_directory() {
    let dir = project();
    let product = dir.path().join("examples/products/zero-3e-demo");
    let inputs_of = |nodes: &[serde_json::Value], id: &str| -> Vec<String> {
        nodes
            .iter()
            .filter(|n| n["id"] == id)
            .filter_map(|n| n["consts"]["path"]["value"].as_str())
            .map(str::to_string)
            .collect()
    };
    // As the example is: its fragment names one file beside itself, so the
    // product's directory (its README, its lock) is no input of a build.
    let out = plan(dir.path(), &["--json"]);
    assert!(out.status.success());
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let whole = inputs_of(nodes, "gaia.source.path");
    assert!(
        !whole
            .iter()
            .any(|path| path.ends_with("products/zero-3e-demo")),
        "the product's directory is an input: {whole:?}"
    );
    let single = inputs_of(nodes, "gaia.source.input");
    assert!(
        single
            .iter()
            .any(|path| path.ends_with("products/zero-3e-demo/busybox.fragment")),
        "the named file alone: {single:?}"
    );
    // A directory named beside the fragment, and the bare directory.
    std::fs::create_dir_all(product.join("patches/linux")).expect("mkdir");
    let fragment = product.join("os.fragment");
    let text = std::fs::read_to_string(&fragment).expect("read");
    std::fs::write(
        &fragment,
        format!("{text}BR2_GLOBAL_PATCH_DIR=\"@here@/patches\"\nBR2_ROOTFS_OVERLAY=\"@here@\"\n"),
    )
    .expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args([
            "plan",
            "product.toml",
            "--json",
            "--recipes",
            "../../../recipes",
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
        ])
        .current_dir(&product)
        .env("GAIA_STORE", dir.path().join("store"))
        .env("GAIA_JOBS", "4")
        .env_remove("GAIA_CPUS")
        .env_remove("GAIA_BUILDROOT_DL")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let paths = inputs_of(nodes, "gaia.source.path");
    println!("{paths:?}");
    assert!(
        paths.iter().any(|path| path == "."),
        "the product's own directory: {paths:?}"
    );
    assert!(
        !paths.iter().any(String::is_empty),
        "no empty path: {paths:?}"
    );
    assert!(
        paths.iter().any(|path| path == "patches"),
        "the named directory alone: {paths:?}"
    );
    let single = inputs_of(nodes, "gaia.source.input");
    assert!(
        single.iter().any(|path| path == "busybox.fragment"),
        "the named file alone: {single:?}"
    );
}

/// A component states a path that is a symbolic link, not a file.
#[test]
fn a_component_installs_a_symbolic_link() {
    let dir = project();
    let component = dir
        .path()
        .join("examples/components/usb-gadget-network/component.toml");
    let original = std::fs::read_to_string(&component).expect("read");
    let add =
        |entry: &str| std::fs::write(&component, format!("{original}\n{entry}")).expect("write");
    add("[[install]]\nfrom = { link = \"/run/board\" }\nto = \"/etc/board\"\n");
    let out = plan(dir.path(), &["--json"]);
    assert!(out.status.success());
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let links = nodes
        .iter()
        .find(|n| n["label"] == "zero-3e-demo.usb-gadget-network.links")
        .expect("the links step");
    assert_eq!(links["id"], "gaia.tree.links");
    assert_eq!(links["consts"]["owner"]["value"], "usb-gadget-network");
    assert_eq!(
        links["consts"]["links"]["value"],
        "[[\"/etc/board\",\"/run/board\"]]"
    );

    for (entry, key, message) in [
        (
            "[[install]]\nfrom = { link = \"/run/board\" }\nto = \"/etc/board\"\nmode = \"0755\"\n",
            ".mode",
            "a link has no mode of its own",
        ),
        (
            "[[install]]\nfrom = { link = \"\" }\nto = \"/etc/board\"\n",
            ".from.link",
            "a link needs a target",
        ),
        (
            "[[install]]\nfrom = { link = \"/run/board\", file = \"files/S41usb0\" }\nto = \"/etc/board\"\n",
            "install",
            "a link is not combined with another kind of source",
        ),
    ] {
        add(entry);
        let out = plan(dir.path(), &[]);
        assert_eq!(out.status.code(), Some(2), "{entry}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(key) && err.contains(message), "{entry}: {err}");
    }
}
