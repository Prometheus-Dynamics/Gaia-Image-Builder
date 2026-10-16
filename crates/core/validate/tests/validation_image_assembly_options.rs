pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A root tree of one file, with the given assembly sections appended.
fn assembly(sections: &str) -> String {
    format!(
        r#"
build_name = "helios"
version = "1.2.3"

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
id = "rootfs"
path = "$assembly.work/rootfs"

[[image.assembly.files]]
tree = "rootfs"
src = "@assets/motd.txt"
dest = "etc/motd"
{sections}
"#
    )
}

fn assembly_codes(contents: &str) -> Vec<&'static str> {
    let path = write_temp_config(contents);
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

const VERITY_OK: &str = r#"
[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"
"#;

#[test]
fn verity_transform_and_its_archive_tokens_validate() {
    let contents = assembly(&format!(
        r#"{VERITY_OK}
[[image.assembly.archives]]
id = "update"
output = "$provider.images/update.tar"

[[image.assembly.archives.members]]
name = "verity.env"
entries = [
  ["ROOT_HASH", "${{assembly.verity:$assembly.work/rootfs.verity.erofs:root_hash}}"],
  ["HASH_OFFSET", "${{assembly.verity:$assembly.work/rootfs.verity.erofs:hash_offset}}"],
]
"#
    ));
    assert_eq!(assembly_codes(&contents), Vec::<&str>::new());
}

#[test]
fn verity_options_are_checked() {
    let contents = assembly(
        r#"
[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"
hash_dest = "$assembly.work/rootfs.verity.erofs"
data_block_size = 3000
hash_block_size = 8192
salt = "xyz"
uuid = "not-a-uuid"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec![
            "assembly_verity_hash_dest_same_as_dest",
            "assembly_verity_block_size_invalid",
            "assembly_verity_block_size_invalid",
            "assembly_verity_salt_invalid",
            "assembly_verity_uuid_invalid",
        ]
    );
}

#[test]
fn verity_options_on_other_kinds_are_rejected() {
    let contents = assembly(
        r#"
[[image.assembly.transforms]]
kind = "copy"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/copy.erofs"
salt = "00ff"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_transform_option_unsupported"]
    );
}

#[test]
fn verity_tokens_must_name_a_verity_output_and_a_known_field() {
    let contents = assembly(&format!(
        r#"{VERITY_OK}
[[image.assembly.archives]]
id = "update"
output = "$provider.images/update.tar"

[[image.assembly.archives.members]]
name = "verity.env"
entries = [
  ["WRONG", "${{assembly.verity:$assembly.work/other.erofs:root_hash}}"],
  ["FIELD", "${{assembly.verity:$assembly.work/rootfs.verity.erofs:uuid}}"],
]
"#
    ));
    assert_eq!(
        assembly_codes(&contents),
        vec![
            "assembly_archive_verity_target_unknown",
            "assembly_archive_entry_value_invalid",
            // `other.erofs` is read, but no step writes it.
            "assembly_token_path_not_produced",
        ]
    );
}

#[test]
fn ext4_filesystems_validate_with_a_size_and_their_options() {
    let contents = assembly(
        r#"
[[image.assembly.filesystems]]
id = "data"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/data.ext4"
size = "16M"
label = "data"
uuid = "6e2d2b1c-0000-4000-8000-000000000001"
journal = false

[[image.assembly.filesystems]]
id = "auto"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/auto.ext4"
size = "auto"
"#,
    );
    assert_eq!(assembly_codes(&contents), Vec::<&str>::new());
}

#[test]
fn ext4_filesystem_options_are_checked() {
    let contents = assembly(
        r#"
[[image.assembly.filesystems]]
id = "nosize"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/nosize.ext4"

[[image.assembly.filesystems]]
id = "label"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/label.ext4"
size = "16M"
label = "a-label-that-is-too-long"

[[image.assembly.filesystems]]
id = "unaligned"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/unaligned.ext4"
size = "1000"

[[image.assembly.filesystems]]
id = "loose"
kind = "ext4"
source_tree = "rootfs"
output = "$provider.images/loose.ext4"
size = "16M"
deterministic = false

[[image.assembly.filesystems]]
id = "vfat"
kind = "vfat"
source_tree = "rootfs"
output = "$provider.images/vfat.img"
size = "16M"
label = "boot"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec![
            "assembly_filesystem_deterministic_unsupported",
            "assembly_filesystem_size_required",
            "assembly_filesystem_label_invalid",
            "assembly_filesystem_size_unaligned",
            "assembly_filesystem_option_unsupported",
        ]
    );
}
