pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A buildroot image with one tree and the given `[image.assembly]` extras.
fn config(extra: &str) -> String {
    format!(
        r#"
build_name = "strip-include"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

{extra}
"#
    )
}

fn codes(extra: &str) -> Vec<&'static str> {
    codes_for(&config(extra))
}

/// Like `codes`, with `key` added to the `[image.assembly]` table.
fn codes_with_assembly_key(key: &str, extra: &str) -> Vec<&'static str> {
    codes_for(&config(extra).replace("[image.assembly]\n", &format!("[image.assembly]\n{key}\n")))
}

fn codes_for(text: &str) -> Vec<&'static str> {
    let path = write_temp_config(text);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("assembly_"))
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn include_and_strip_options_validate() {
    assert_eq!(
        codes_with_assembly_key(
            r#"strip_tool = "aarch64-linux-gnu-strip""#,
            r#"
[[image.assembly.trees]]
id = "rootfs"
path = "$assembly.work/rootfs"
strip = true

[[image.assembly.files]]
tree = "boot"
src_glob = "@assets/overlays/*.dtbo"
include = ["rpi-*.dtbo", "config.dtbo"]
strip = true
dest = "overlays/"

[[image.assembly.binaries]]
tree = "boot"
src = "$provider.images/tool"
dest = "usr/bin/tool"
strip = true

"#
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn include_only_applies_to_src_glob_and_needs_names() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "boot"
src = "@assets/a.dtbo"
dest = "a.dtbo"
include = ["a.dtbo"]
"#
        ),
        vec!["assembly_include_not_allowed"]
    );
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "boot"
src_glob = "@assets/*.dtbo"
include = ["a/b.dtbo", ""]
dest = "overlays/"
"#
        ),
        vec!["assembly_include_invalid", "assembly_include_invalid"]
    );
}

#[test]
fn strip_does_not_apply_to_generated_content() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "boot"
content = "hello"
dest = "hello.txt"
strip = true
"#
        ),
        vec!["assembly_strip_not_allowed"]
    );
}

#[test]
fn strip_tool_must_not_be_empty() {
    assert_eq!(
        codes_with_assembly_key(r#"strip_tool = """#, ""),
        vec!["assembly_strip_tool_empty"]
    );
}

#[test]
fn gzip_accepts_levels_one_to_nine() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.work/Image.gz"
level = 1

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.work/Image9.gz"
level = 9
"#
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn kernel_module_metadata_names_must_start_with_modules() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.kernel_modules]]
tree = "boot"
from = "$provider.images/modules"
modules = ["vfat"]
metadata = ["modules.dep", "dep"]
"#
        ),
        vec!["assembly_kernel_modules_metadata_invalid"]
    );
}

#[test]
fn cpio_xz_compression_level_is_zero_to_nine() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-xz"
source_tree = "boot"
output = "$assembly.work/initramfs.cpio.xz"
compression_level = 10
"#
        ),
        vec!["assembly_filesystem_compression_level_invalid"]
    );
}
