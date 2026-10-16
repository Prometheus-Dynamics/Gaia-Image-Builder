//! Validation of generated content: file `content` and `json`, listed
//! binaries, and the tokens generated text and JSON read.

pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A rootfs tree with `/etc/motd` from assets, a copied image, and the given
/// sections appended.
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

[[image.assembly.transforms]]
kind = "copy"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.erofs"
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

#[test]
fn a_file_with_content_and_a_src_is_rejected() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
src = "@assets/motd.txt"
content = "hello"
dest = "etc/motd"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_file_source_invalid"]
    );
}

#[test]
fn a_file_with_content_and_json_is_rejected() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
content = "hello"
json = { schema = 1 }
dest = "state.json"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_file_source_invalid"]
    );
}

#[test]
fn a_generated_file_must_name_a_file_not_a_directory() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
content = "hello"
dest = "etc/"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_file_generated_dest_invalid"]
    );
}

#[test]
fn generated_text_that_reads_a_produced_path_is_valid() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
content = "root=${assembly.size:$assembly.work/rootfs.erofs} digest=${assembly.sha256:$assembly.work/rootfs.erofs}\n"
dest = "cmdline.txt"
mode = "0600"
"#,
    );
    assert_eq!(assembly_codes(&contents), Vec::<&str>::new());
}

#[test]
fn a_token_reading_a_path_no_step_writes_is_rejected() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
content = "size=${assembly.size:$assembly.work/never.bin}"
dest = "size.txt"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_token_path_not_produced"]
    );
}

#[test]
fn an_unknown_assembly_token_in_generated_json_is_rejected() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
json = { digest = "${assembly.md5:$assembly.work/rootfs.erofs}" }
dest = "manifest.json"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_generated_token_invalid"]
    );
}

#[test]
fn archive_json_members_are_checked_like_generated_text() {
    let contents = assembly(
        r#"
[[image.assembly.archives]]
id = "bundle"
output = "$assembly.work/bundle.tar"

[[image.assembly.archives.members]]
name = "manifest.json"
json = { image = { size = "${assembly.size:$assembly.work/rootfs.erofs}" }, id = "a" }

[[image.assembly.archives.members]]
name = "both.json"
entries = [["A", "1"]]
json = { schema = 1 }
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_archive_member_source_invalid"]
    );
}

#[test]
fn a_listed_binary_needs_a_known_tree_and_a_valid_mode() {
    let contents = assembly(
        r#"
[[image.assembly.binaries]]
tree = "rootfs"
src = "$provider.buildroot_output/target/usr/sbin/e2fsck"
dest = "usr/sbin/e2fsck"
mode = "0755"

[[image.assembly.binaries]]
tree = "missing"
src = "$provider.buildroot_output/target/usr/sbin/mke2fs"
dest = "usr/sbin/mke2fs"

[[image.assembly.binaries]]
tree = "rootfs"
src = "$provider.buildroot_output/target/usr/sbin/resize2fs"
dest = "usr/sbin/resize2fs"
mode = "0789"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec![
            "assembly_binary_tree_unknown",
            "assembly_binary_mode_invalid"
        ]
    );
}

#[test]
fn a_listed_binary_dest_must_stay_inside_its_tree() {
    let contents = assembly(
        r#"
[[image.assembly.binaries]]
tree = "rootfs"
src = "$provider.buildroot_output/target/usr/sbin/e2fsck"
dest = "../escape"
"#,
    );
    assert_eq!(
        assembly_codes(&contents),
        vec!["assembly_binary_dest_invalid"]
    );
}

#[test]
fn a_tree_digest_is_a_whole_json_value_of_a_known_tree_or_a_path() {
    let json_tree = assembly(
        r#"
[[image.assembly.trees]]
id = "payload"
path = "$assembly.work/payload"

[[image.assembly.files]]
tree = "rootfs"
json = { files = "${assembly.sha256_tree:payload}" }
dest = "manifest.json"
"#,
    );
    assert_eq!(assembly_codes(&json_tree), Vec::<&str>::new());

    let json_path = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
json = { files = "${assembly.sha256_tree:$assembly.work/rootfs}" }
dest = "manifest.json"
"#,
    );
    assert!(
        assembly_codes(&json_path).contains(&"assembly_tree_digest_self"),
        "a path into the file's own tree is the same self-reference: {:?}",
        assembly_codes(&json_path)
    );

    // A directory the steps fill, named by its path, is a produced read.
    let filled = assembly(
        r#"
[[image.assembly.trees]]
id = "payload"
path = "$assembly.work/payload"

[[image.assembly.files]]
tree = "payload"
content = "x"
dest = "a.txt"

[[image.assembly.files]]
tree = "rootfs"
json = { files = "${assembly.sha256_tree:$assembly.work/payload}" }
dest = "manifest.json"
"#,
    );
    assert_eq!(assembly_codes(&filled), Vec::<&str>::new());
}

#[test]
fn a_tree_digest_in_text_or_naming_no_tree_is_rejected() {
    let in_text = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
content = "slots=${assembly.sha256_tree:rootfs}"
dest = "cmdline.txt"
"#,
    );
    assert_eq!(
        assembly_codes(&in_text),
        vec!["assembly_generated_token_invalid"]
    );

    let unknown = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
json = { files = "${assembly.sha256_tree:slots-payload}" }
dest = "manifest.json"
"#,
    );
    assert_eq!(
        assembly_codes(&unknown),
        vec!["assembly_tree_digest_unknown_tree"]
    );
}

#[test]
fn a_file_cannot_digest_the_tree_it_is_written_into() {
    let contents = assembly(
        r#"
[[image.assembly.files]]
tree = "rootfs"
json = { files = "${assembly.sha256_tree:rootfs}" }
dest = "manifest.json"
"#,
    );
    // The file is in the tree it digests: one error, and no unknown-tree error.
    assert!(
        assembly_codes(&contents).contains(&"assembly_tree_digest_self"),
        "{:?}",
        assembly_codes(&contents)
    );
}
