pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{AssemblyFilesystemKindSpec, AssemblyFilesystemSpec};
use support::write_temp_config;

/// The flasher boot.img from docs/configuration.md: an initramfs (BusyBox,
/// kernel modules, cpio-zstd) packed into a FAT boot image that is published.
const FLASHER: &str = r#"
build_name = "flasher"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "flasher_defconfig"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
include_runtime_libs = false
applets = ["sh", "mount", "mkdir", "modprobe", "switch_root"]

[[image.assembly.kernel_modules]]
tree = "initramfs"
from = "$provider.target/lib/modules"
modules = ["libcomposite", "usb-f-mass-storage"]

[[image.assembly.files]]
tree = "initramfs"
src = "@assets/flasher/init"
dest = "init"
mode = "0755"

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-zstd"
source_tree = "initramfs"
output = "$assembly.work/initramfs.cpio.zst"
compression_level = 19

[[image.assembly.files]]
tree = "boot"
src = "$provider.images/Image"
dest = "Image"

[[image.assembly.files]]
tree = "boot"
src = "$provider.images/bcm2711-rpi-4-b.dtb"
dest = "bcm2711-rpi-4-b.dtb"

[[image.assembly.files]]
tree = "boot"
src = "@assets/flasher/config.txt"
dest = "config.txt"

[[image.assembly.files]]
tree = "boot"
src = "$assembly.work/initramfs.cpio.zst"
dest = "initramfs.cpio.zst"

[[image.assembly.filesystems]]
id = "boot"
kind = "vfat"
source_tree = "boot"
output = "$assembly.work/boot.img"
size = "64M"
publish = true
"#;

#[test]
fn flasher_boot_image_resolves_cpio_zstd_kernel_modules_and_publish() {
    let path = write_temp_config(FLASHER);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let _ = std::fs::remove_file(path);
    let assembly = spec.image.assembly.as_ref().expect("assembly");

    assert!(
        spec.policy.interpolation.unresolved.is_empty(),
        "{:?}",
        spec.policy.interpolation.unresolved
    );
    let initramfs = &assembly.filesystems[0];
    assert_eq!(initramfs.kind, AssemblyFilesystemKindSpec::CpioZstd);
    assert_eq!(initramfs.compression_level, Some(19));
    assert!(initramfs.deterministic);
    assert!(!initramfs.publish);
    let boot = &assembly.filesystems[1];
    assert_eq!(boot.kind, AssemblyFilesystemKindSpec::Vfat);
    assert!(boot.publish);
    assert_eq!(
        boot.publish_template().expect("published").as_str(),
        "$assembly.out/boot.img"
    );
    let modules = &assembly.kernel_modules[0];
    assert_eq!(modules.tree.as_str(), "initramfs");
    assert_eq!(modules.from.as_str(), "$provider.target/lib/modules");
    assert_eq!(modules.kernel_version, None);
    assert_eq!(modules.modules, ["libcomposite", "usb-f-mass-storage"]);
    assert_eq!(modules.depmod, None);
}

#[test]
fn cpio_zstd_and_publish_default_to_off_when_unset() {
    let spec = AssemblyFilesystemSpec {
        id: "initramfs".into(),
        kind: AssemblyFilesystemKindSpec::CpioZstd,
        source_tree: "initramfs".into(),
        output: "$assembly.work/initramfs.cpio.zst".into(),
        size: None,
        deterministic: true,
        compression_level: None,
        publish: false,
    };
    assert_eq!(spec.publish_template(), None);
    assert_eq!(AssemblyFilesystemKindSpec::CpioZstd.as_str(), "cpio-zstd");
}
