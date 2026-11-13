//! The examples in the new form parse, and say what they should.

use gaia_describe::{Board, FileRef, OsProvider, Product, Source};
use gaia_types::{Arch, Os, PartitionRole, Size};
use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples")
}

#[test]
fn the_zero_3e_board_reads_with_its_family() {
    let board = Board::read(examples().join("boards/zero-3e/board.toml")).expect("reads");
    println!("{board:#?}");
    assert_eq!(board.name, "zero-3e");
    assert_eq!(board.target.arch, Arch::AArch64);
    assert_eq!(board.target.cpu.as_deref(), Some("cortex-a55"));
    assert_eq!(board.target.os, Os::Linux);
    assert!(
        board
            .family
            .as_ref()
            .is_some_and(|f| f.ends_with("rk35xx/family.toml"))
    );
    assert_eq!(
        board.sources.keys().collect::<Vec<_>>(),
        ["linux", "rkbin", "tf-a", "u-boot"]
    );
    assert!(
        matches!(&board.sources["linux"], Source::Git { reference, .. } if reference == "v7.3.3")
    );

    let kernel = board.kernel.as_ref().expect("kernel");
    // The family's source and defconfig, the board's device tree.
    assert_eq!(kernel.source.as_deref(), Some("linux"));
    assert_eq!(kernel.defconfig.as_deref(), Some("defconfig"));
    assert_eq!(
        kernel.device_tree.as_deref(),
        Some("rockchip/rk3566-radxa-zero-3e")
    );
    // The board's list replaces the family's; each path is relative to the
    // file it is written in.
    let fragments: Vec<_> = kernel
        .fragments
        .iter()
        .flatten()
        .map(|f| f.path().to_path_buf())
        .collect();
    assert_eq!(fragments.len(), 2);
    assert!(fragments[0].ends_with("examples/boards/rk35xx/assets/linux.fragment"));
    assert!(fragments[1].ends_with("examples/boards/zero-3e/assets/linux.fragment"));
    // No `..` is left in a resolved path.
    assert!(
        fragments
            .iter()
            .all(|f| !f.components().any(|c| c.as_os_str() == ".."))
    );

    let bootloader = board.bootloader.as_ref().expect("bootloader");
    assert_eq!(bootloader.source.as_deref(), Some("u-boot"));
    assert_eq!(bootloader.defconfig.as_deref(), Some("radxa-zero-3-rk3566"));
    let tf = bootloader.trusted_firmware.as_ref().expect("tf-a");
    assert_eq!(
        (tf.source.as_deref(), tf.platform.as_deref()),
        (Some("tf-a"), Some("rk3568"))
    );
    assert_eq!(
        bootloader.blobs["ROCKCHIP_TPL"],
        FileRef::InSource {
            source: "rkbin".into(),
            path: "bin/rk35/rk3566_ddr_1056MHz_v1.23.bin".into()
        }
    );
    // A family's fragment, resolved against the family file.
    assert!(
        bootloader
            .fragments
            .iter()
            .flatten()
            .all(|f| f.path().ends_with("rk35xx/assets/u-boot.fragment"))
    );

    assert_eq!(board.partitions.len(), 2);
    assert_eq!(board.partitions[0].role, PartitionRole::Bootloader);
    assert!(!board.partitions[0].in_table);
    assert_eq!(board.partitions[1].size.map(|s| s.0), Some(Size::kib(64)));
    assert_eq!(board.partitions[1].offset.map(|s| s.0), Some(Size::mib(16)));

    let flash = board.flash.as_ref().expect("flash");
    assert_eq!(
        flash.methods.as_deref(),
        Some(&["sd-card".to_string(), "maskrom".to_string()][..])
    );
    assert!(flash.files.contains_key("loader_ini"));
}

#[test]
fn the_demo_product_loads_its_board_and_component() {
    let loaded =
        Product::load(examples().join("products/zero-3e-demo/product.toml")).expect("loads");
    assert_eq!(loaded.product.images.len(), 1);
    let image = &loaded.product.images[0];
    assert_eq!(image.recipe, "sd-image");
    assert_eq!(image.budget.map(|s| s.0), Some(Size::mib(1024)));
    assert_eq!(image.settings["id"].as_str(), Some("zero-3e-demo"));
    let os = image.os.as_ref().expect("an os");
    assert_eq!(os.provider, OsProvider::Buildroot);
    assert_eq!(os.source, "buildroot");
    assert!(
        os.fragments[0]
            .path()
            .ends_with("examples/products/zero-3e-demo/os.fragment")
    );
    assert_eq!(loaded.boards.len(), 1);
    let component = &loaded.components["usb-gadget-network"];
    assert_eq!(component.install.len(), 2);
    assert_eq!(component.install[0].mode.map(|m| m.0), Some(0o755));
    let wants: Vec<_> = loaded.wants().into_iter().map(|(name, _)| name).collect();
    println!("lock names: {wants:?}");
    assert_eq!(
        wants,
        [
            "hello-rust.root",
            "hello-rust.src",
            "zero-3e-demo.buildroot",
            "zero-3e.linux",
            "zero-3e.rkbin",
            "zero-3e.tf-a",
            "zero-3e.u-boot"
        ]
    );
}

/// The gadget network's Ethernet addresses are the same on every boot: its
/// script derives them from the board's identity by the rule Lemnos uses
/// for its NCM function. The vector is that rule's.
#[test]
fn the_gadget_networks_addresses_follow_the_identity_rule() {
    let script = examples().join("components/usb-gadget-network/files/S41usb0");
    let addresses = |family: &str, serial: &str| {
        let out = std::process::Command::new("/bin/sh")
            .arg(&script)
            .args(["addresses", family, serial])
            .output()
            .expect("sh");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    // Host, then device.
    assert_eq!(
        addresses("lemnos-test", "0123456789abcdef"),
        "aa:22:75:11:2a:72 aa:22:75:11:2a:73"
    );
    // Leading zeros are kept.
    assert_eq!(
        addresses("alpha", "00000001"),
        "b2:b5:ba:4c:1e:30 b2:b5:ba:4c:1e:31"
    );
    // The serial is hashed in lowercase.
    assert_eq!(
        addresses("lemnos-test", "0123456789ABCDEF"),
        "aa:22:75:11:2a:72 aa:22:75:11:2a:73"
    );
    // A serial of another shape, or no family, gives no address at all.
    for (family, serial) in [
        ("alpha", "0000001"),
        ("alpha", "0x00000001"),
        ("alpha", "0000000g"),
        ("alpha", "000000000000000000000000000000001"),
        ("alpha", ""),
        ("", "00000001"),
    ] {
        assert_eq!(addresses(family, serial), "", "{family:?} {serial:?}");
    }
    let fragment = std::fs::read_to_string(examples().join("boards/zero-3e/assets/linux.fragment"))
        .expect("read");
    assert!(
        fragment.contains("CONFIG_USB_CONFIGFS_NCM=y") && !fragment.contains("CONFIG_USB_ETH=y"),
        "the script makes the gadget; a built-in one would bind first with random addresses"
    );
}

/// The bring-up gadget never presents a managed gadget's identity: Windows
/// keeps what it learned about a device per vendor, product and revision.
/// The managed values are Lemnos's (its daemon's gadget at W4): 1d6b:0104,
/// revisions 0x0100 to 0x0aff.
#[test]
fn the_gadget_networks_identity_is_not_a_managed_gadgets() {
    let script = examples().join("components/usb-gadget-network/files/S41usb0");
    let out = std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg("identity")
        .output()
        .expect("sh");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let values: Vec<u32> = text
        .split_whitespace()
        .map(|word| u32::from_str_radix(word.trim_start_matches("0x"), 16).expect("hex"))
        .collect();
    assert_eq!(
        values,
        [0x1d6b, 0x0103, 0x0b00],
        "vendor, product, revision"
    );
    let managed = |vendor: u32, product: u32, revision: u32| {
        vendor == 0x1d6b && product == 0x0104 && (0x0100..=0x0aff).contains(&revision)
    };
    assert!(!managed(values[0], values[1], values[2]));
    // Each alone keeps it apart: another product, and a revision outside
    // the managed range even if the product were the same.
    assert!(!(0x0100..=0x0aff).contains(&values[2]));
    // The script writes all three before it binds, the revision included.
    let source = std::fs::read_to_string(&script).expect("read");
    for line in [
        "echo \"$ID_VENDOR\" > \"$G/idVendor\"",
        "echo \"$ID_PRODUCT\" > \"$G/idProduct\"",
        "echo \"$BCD_DEVICE\" > \"$G/bcdDevice\"",
    ] {
        assert!(source.contains(line), "the script has no line {line:?}");
    }
}
