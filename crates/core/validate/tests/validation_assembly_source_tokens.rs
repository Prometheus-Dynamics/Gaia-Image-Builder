//! Source tokens in `[image.assembly]`: a resolved one is not reported, and
//! an unknown source id is still an error.

pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use support::write_temp_config;

fn codes(assembly_entry: &str) -> Vec<&'static str> {
    let contents = format!(
        r#"
build_name = "raze-slot"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app"
kind = "git"
repo = "https://example.invalid/app.git"
rev = "0123456789abcdef0123456789abcdef01234567"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "slot"
path = "$assembly.work/slot"
{assembly_entry}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    validate_spec(&spec)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("interpolation_"))
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn a_resolved_source_token_in_the_assembly_is_not_reported() {
    let resolved = codes(
        r#"
[[image.assembly.files]]
tree = "slot"
content = "commit=${source.app.commit}"
dest = "etc/${source.app.commit_short}.txt"
"#,
    );
    assert!(resolved.is_empty(), "{resolved:?}");
}

#[test]
fn an_unknown_source_id_in_the_assembly_is_an_unresolved_error() {
    let unknown = codes(
        r#"
[[image.assembly.files]]
tree = "slot"
content = "x"
dest = "etc/${source.nope.commit_short}.txt"
"#,
    );
    assert_eq!(unknown, vec!["interpolation_unresolved"]);
}
