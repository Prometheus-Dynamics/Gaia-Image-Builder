//! Validation of `src_dir` file entries: exclusive with the other sources,
//! `exclude` allowed, `include` and `preserve_symlink` refused.

pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A buildroot image with a `rootfs` tree and the given file entries.
fn config(files: &str) -> String {
    format!(
        r#"
build_name = "src-dir"
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
id = "rootfs"
path = "$assembly.work/rootfs"

{files}
"#
    )
}

/// The diagnostics of the file entries, as `(code, message)`.
fn diagnostics(files: &str) -> Vec<(&'static str, String)> {
    let path = write_temp_config(&config(files));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("assembly_"))
        .map(|diagnostic| (diagnostic.code, diagnostic.message.clone()))
        .collect()
}

fn codes(files: &str) -> Vec<&'static str> {
    diagnostics(files)
        .into_iter()
        .map(|(code, _)| code)
        .collect()
}

#[test]
fn a_src_dir_entry_validates_alone() {
    assert_eq!(
        codes(
            r#"
[[image.assembly.files]]
tree = "rootfs"
src_dir = "@assets/site"
dest = "web/"
exclude = ["*.map"]
mode = "0644"
strip = true
"#
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn src_dir_is_exclusive_with_src_src_glob_content_and_json() {
    for extra in [
        r#"src = "@assets/index.html""#,
        r#"src_glob = "@assets/*.html""#,
        r#"content = "x""#,
    ] {
        let files = format!(
            r#"
[[image.assembly.files]]
tree = "rootfs"
src_dir = "@assets/site"
dest = "web/"
{extra}
"#
        );
        assert!(
            codes(&files).contains(&"assembly_file_source_invalid"),
            "src_dir with {extra} must be refused"
        );
    }
}

#[test]
fn src_dir_with_json_is_refused() {
    let files = r#"
[[image.assembly.files]]
tree = "rootfs"
src_dir = "@assets/site"
dest = "web/"
json = { a = 1 }
"#;
    assert!(codes(files).contains(&"assembly_file_source_invalid"));
}

#[test]
fn an_entry_without_any_source_is_refused() {
    let files = r#"
[[image.assembly.files]]
tree = "rootfs"
dest = "web/"
"#;
    assert!(codes(files).contains(&"assembly_file_source_invalid"));
}

#[test]
fn include_and_preserve_symlink_are_refused_on_src_dir() {
    let files = r#"
[[image.assembly.files]]
tree = "rootfs"
src_dir = "@assets/site"
dest = "web/"
include = ["a.js"]
preserve_symlink = true
"#;
    let found = codes(files);
    assert!(found.contains(&"assembly_include_not_allowed"), "{found:?}");
    assert!(
        found.contains(&"assembly_preserve_symlink_not_allowed"),
        "{found:?}"
    );
}

#[test]
fn exclude_on_a_plain_src_is_still_refused_and_names_src_dir() {
    let files = r#"
[[image.assembly.files]]
tree = "rootfs"
src = "@assets/index.html"
dest = "index.html"
exclude = ["*.map"]
"#;
    let found = diagnostics(files);
    let (code, message) = found
        .iter()
        .find(|(code, _)| *code == "assembly_exclude_not_allowed")
        .expect("exclude on src is refused");
    assert_eq!(*code, "assembly_exclude_not_allowed");
    assert!(message.contains("src_dir"), "{message}");
}
