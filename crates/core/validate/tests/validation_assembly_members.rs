pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A buildroot image with one tree and the given `[image.assembly]` extras.
fn config(extra: &str) -> String {
    format!(
        r#"
build_name = "members"
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
    let path = write_temp_config(&config(extra));
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
fn tree_dir_and_glob_members_validate() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
tree = "boot"
prefix = "boot/overlays/"
exclude = ["zz*", "link"]

[[image.assembly.archives.members]]
name = "extra/"
src_glob = "@assets/*.txt"
exclude = ["skip.txt"]

[[image.assembly.archives.members]]
src_dir = "@assets"
prefix = "dir/"
"#
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn exclude_only_applies_to_globs_and_trees() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "boot"
src = "@assets/a.txt"
dest = "a.txt"
exclude = ["b.txt"]
"#
        ),
        vec!["assembly_exclude_not_allowed"]
    );
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
name = "a.txt"
src = "@assets/a.txt"
exclude = ["b.txt"]
"#
        ),
        vec!["assembly_exclude_not_allowed"]
    );
}

#[test]
fn exclude_entries_must_be_file_names() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "boot"
src_glob = "@assets/*.dtbo"
exclude = ["sub/b.dtbo"]
dest = "overlays/"
"#
        ),
        vec!["assembly_exclude_invalid"]
    );
}

#[test]
fn archive_member_sources_and_prefixes_are_checked() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
tree = "missing"
"#
        ),
        vec!["assembly_archive_member_tree_unknown"]
    );
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
name = "a.txt"
src = "@assets/a.txt"
prefix = "boot/"
"#
        ),
        vec!["assembly_archive_member_prefix_invalid"]
    );
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
tree = "boot"
prefix = "boot"
"#
        ),
        vec!["assembly_archive_member_prefix_invalid"]
    );
    assert_eq!(
        codes(
            r#"
[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
name = "extra"
src_glob = "@assets/*.txt"
"#
        ),
        vec!["assembly_archive_member_name_invalid"]
    );
}

#[test]
fn filesystem_free_needs_auto_size_and_a_byte_size() {
    let filesystem = |size: &str, free: &str| {
        format!(
            r#"
[[image.assembly.filesystems]]
id = "boot"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
size = "{size}"
free = "{free}"
"#
        )
    };
    assert_eq!(
        codes(&filesystem("16M", "256M")),
        vec!["assembly_filesystem_free_without_auto"]
    );
    assert_eq!(codes(&filesystem("auto", "256M")), Vec::<&str>::new());
    assert_eq!(
        codes(&filesystem("auto", "lots")),
        vec!["assembly_filesystem_free_invalid"]
    );
}
