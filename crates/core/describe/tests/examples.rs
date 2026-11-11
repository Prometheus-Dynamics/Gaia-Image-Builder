//! The examples in the new form parse, and say what they should.

use gaia_describe::{Board, FileRef, Product, Source};
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
        bootloader.blobs["tpl"],
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
    assert_eq!(image.recipe, "ab-rootfs");
    assert_eq!(image.budget.map(|s| s.0), Some(Size::mib(1024)));
    assert_eq!(image.settings["hostname"].as_str(), Some("zero3e"));
    assert_eq!(loaded.boards.len(), 1);
    let component = &loaded.components["usb-gadget-network"];
    assert_eq!(component.install.len(), 2);
    assert_eq!(component.install[0].mode.map(|m| m.0), Some(0o644));
    let wants: Vec<_> = loaded.wants().into_iter().map(|(name, _)| name).collect();
    println!("lock names: {wants:?}");
    assert_eq!(
        wants,
        [
            "zero-3e.linux",
            "zero-3e.rkbin",
            "zero-3e.tf-a",
            "zero-3e.u-boot"
        ]
    );
}
