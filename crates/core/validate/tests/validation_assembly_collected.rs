pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A Buildroot image with the given expected images and assembly body; the
/// assembly has one tree, `t`.
fn config(expected_images: &str, assembly_body: &str) -> String {
    format!(
        r#"
build_name = "collected"
version = "1.2.3"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
{expected_images}

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

{assembly_body}
"#
    )
}

fn diagnostics_for(contents: &str) -> Vec<(&'static str, String)> {
    let path = write_temp_config(contents);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.message.clone()))
        .collect()
}

fn not_collected(contents: &str) -> Vec<String> {
    diagnostics_for(contents)
        .into_iter()
        .filter(|(code, _)| *code == "assembly_provider_image_not_collected")
        .map(|(_, message)| message)
        .collect()
}

const FLASH_ID: &str = r#"
[[image.expected_images]]
name = "flash-id"
format = "file"
required = true
"#;

#[test]
fn an_uncollected_provider_images_source_is_an_error_with_a_hint() {
    let contents = config(
        FLASH_ID,
        r#"
[[image.assembly.files]]
tree = "t"
src = "$provider.images/flash-id.raw"
dest = "flash-id.raw"
"#,
    );
    let messages = not_collected(&contents);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("image.assembly files[0].src '$provider.images/flash-id.raw'")
            && messages[0].contains("is not collected")
            && messages[0].contains("expected_images")
            && messages[0].contains("$provider.buildroot_output/images/flash-id.raw"),
        "{messages:?}"
    );
}

#[test]
fn a_collected_provider_images_source_is_accepted() {
    let contents = config(
        FLASH_ID,
        r#"
[[image.assembly.files]]
tree = "t"
src = "$provider.images/flash-id"
dest = "flash-id"
"#,
    );
    assert!(not_collected(&contents).is_empty());
}

#[test]
fn glob_is_checked_only_when_no_expected_name_can_match_its_prefix() {
    let matching = config(
        FLASH_ID,
        r#"
[[image.assembly.files]]
tree = "t"
src_glob = "$provider.images/flash-*"
dest = "."
"#,
    );
    assert!(not_collected(&matching).is_empty());

    let unmatched = config(
        FLASH_ID,
        r#"
[[image.assembly.files]]
tree = "t"
src_glob = "$provider.images/rootfs-*"
dest = "."
"#,
    );
    assert_eq!(not_collected(&unmatched).len(), 1);
}

#[test]
fn names_the_assembly_produces_itself_are_not_checked() {
    let contents = config(
        FLASH_ID,
        r#"
[[image.assembly.filesystems]]
id = "rootfs"
kind = "vfat"
source_tree = "t"
output = "$provider.images/rootfs.vfat"
size = "16M"

[[image.assembly.files]]
tree = "t"
src = "$provider.images/rootfs.vfat"
dest = "rootfs.vfat"
"#,
    );
    assert!(not_collected(&contents).is_empty());
}

#[test]
fn transform_and_archive_member_sources_are_checked_too() {
    let contents = config(
        FLASH_ID,
        r#"
[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/missing.img"
dest = "$assembly.work/missing.img.zst"

[[image.assembly.archives]]
id = "bundle"
output = "$assembly.work/bundle.tar"

[[image.assembly.archives.members]]
name = "boot.img"
src = "${assembly.sha256:$provider.images/other.img}"
"#,
    );
    let messages = not_collected(&contents);
    assert_eq!(messages.len(), 2, "{messages:?}");
}

#[test]
fn without_a_buildroot_expected_image_list_nothing_is_checked() {
    let contents = config(
        "",
        r#"
[[image.assembly.files]]
tree = "t"
src = "$provider.images/anything"
dest = "anything"
"#,
    );
    assert!(not_collected(&contents).is_empty());
}

#[test]
fn file_format_accepts_any_name_and_raw_still_checks_its_suffix() {
    let accepted = diagnostics_for(&config(FLASH_ID, ""));
    assert!(
        !accepted
            .iter()
            .any(|(code, _)| code.starts_with("buildroot_expected_image")),
        "{accepted:?}"
    );

    let raw = diagnostics_for(&config(
        r#"
[[image.expected_images]]
name = "flash-id"
format = "raw"
required = true
"#,
        "",
    ));
    assert!(
        raw.iter()
            .any(|(code, _)| *code == "buildroot_expected_image_name_mismatch"),
        "{raw:?}"
    );
}
