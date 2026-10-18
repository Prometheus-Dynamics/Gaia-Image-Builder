//! `${source.<id>.commit}`, `.commit_short` and `.path` in `[image.assembly]`:
//! tree ids, paths, file entries, JSON values and archive outputs all
//! resolve, and an unknown source id stays unresolved.

pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{AssemblyArchiveMemberSourceSpec, AssemblyJsonValue};
use support::write_temp_config;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn config(assembly: &str) -> String {
    format!(
        r#"
build_name = "myboard-slot"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app"
kind = "git"
repo = "https://example.invalid/app.git"
rev = "{COMMIT}"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"
{assembly}
"#
    )
}

const SLOT: &str = r#"
[[image.assembly.trees]]
id = "slot-${source.app.commit_short}"
path = "$assembly.work/slot-${source.app.commit_short}"

[[image.assembly.files]]
tree = "slot-${source.app.commit_short}"
content = "commit=${source.app.commit}"
dest = "etc/${source.app.commit_short}.txt"

[[image.assembly.files]]
tree = "slot-${source.app.commit_short}"
json = { rev = "${source.app.commit_short}", root = "${source.app.path}" }
dest = "slot.json"

[[image.assembly.archives]]
id = "bundle"
output = "$assembly.out/bundle-${source.app.commit_short}.tar"

[[image.assembly.archives.members]]
name = "rev.json"
json = { rev = "${source.app.commit_short}" }
"#;

#[test]
fn commit_tokens_resolve_in_tree_ids_paths_entries_json_and_archive_outputs() {
    let path = write_temp_config(&config(SLOT));
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let assembly = spec.image.assembly.as_ref().expect("assembly");

    assert_eq!(
        assembly
            .assembly_sources
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["app"],
        "the declared source the tokens name"
    );
    assert_eq!(assembly.trees[0].id.as_str(), "slot-01234567");
    assert_eq!(
        assembly.trees[0].path.as_str(),
        "$assembly.work/slot-01234567"
    );
    assert_eq!(assembly.files[0].tree.as_str(), "slot-01234567");
    assert_eq!(
        assembly.files[0].content.as_deref(),
        Some(format!("commit={COMMIT}").as_str())
    );
    assert_eq!(assembly.files[0].dest, "etc/01234567.txt");

    let Some(AssemblyJsonValue::Object(members)) = &assembly.files[1].json else {
        panic!("json object expected");
    };
    assert_eq!(
        members[0].1,
        AssemblyJsonValue::String("01234567".to_string())
    );
    // A path source's checkout: the git source's directory, not a token.
    assert!(
        matches!(&members[1].1, AssemblyJsonValue::String(text) if !text.contains("${")),
        "{:?}",
        members[1].1
    );

    assert_eq!(
        assembly.archives[0].output.as_str(),
        "$assembly.out/bundle-01234567.tar"
    );
    let member = &assembly.archives[0].members[0];
    assert!(matches!(
        member.source(),
        Some(AssemblyArchiveMemberSourceSpec::Json(_))
    ));
    let rendered = format!("{:?}", member.source());
    assert!(!rendered.contains("${source"), "{rendered}");

    assert!(
        !spec
            .policy
            .interpolation
            .unresolved
            .iter()
            .any(|unresolved| unresolved.token.starts_with("source.")),
        "{:?}",
        spec.policy.interpolation.unresolved
    );
}

#[test]
fn an_unknown_source_id_stays_unresolved_at_its_location() {
    let path = write_temp_config(&config(
        r#"
[[image.assembly.files]]
tree = "boot"
content = "x"
dest = "a/${source.nope.commit_short}.txt"
"#,
    ));
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let unresolved = &spec.policy.interpolation.unresolved;
    assert!(
        unresolved
            .iter()
            .any(|entry| entry.token == "source.nope.commit_short"
                && entry.location == "image.assembly.files.dest"),
        "{unresolved:?}"
    );
}

#[test]
fn resolving_twice_gives_the_same_assembly() {
    let path = write_temp_config(&config(SLOT));
    let first = resolve_config(path.to_str().expect("utf-8 path"));
    let second = resolve_config(path.to_str().expect("utf-8 path"));
    assert_eq!(first.image.assembly, second.image.assembly);
}
