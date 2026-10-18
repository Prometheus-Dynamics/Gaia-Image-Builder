//! Config for generated content: `content` and `json` on file entries and
//! archive members, `[[image.assembly.binaries]]`, and interpolation of the
//! strings inside them. Keys keep the order the file wrote them.

pub mod support;

use gaia_config::{resolve_config, try_resolve_config};
use gaia_spec::{AssemblyArchiveMemberSourceSpec, AssemblyJsonValue};
use support::write_temp_config;

const BASE: &str = r#"
build_name = "acme"
version = "1.2.3"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"
"#;

fn keys(value: &AssemblyJsonValue) -> Vec<String> {
    match value {
        AssemblyJsonValue::Object(members) => members.iter().map(|(key, _)| key.clone()).collect(),
        other => panic!("not an object: {other:?}"),
    }
}

#[test]
fn json_tables_keep_their_key_order_nesting_and_value_types() {
    let contents = format!(
        r#"{BASE}
[[image.assembly.files]]
tree = "boot"
dest = "manifest.json"
json = {{ zeta = 1, alpha = "a", flag = true, nested = {{ beta = [1, "two"], aleph = {{}} }} }}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let assembly = spec.image.assembly.expect("assembly");
    let json = assembly.files[0].json.clone().expect("json");
    assert_eq!(keys(&json), vec!["zeta", "alpha", "flag", "nested"]);
    let AssemblyJsonValue::Object(members) = &json else {
        unreachable!()
    };
    assert_eq!(members[0].1, AssemblyJsonValue::Integer(1));
    assert_eq!(members[2].1, AssemblyJsonValue::Bool(true));
    assert_eq!(
        members[3].1,
        AssemblyJsonValue::Object(vec![
            (
                "beta".into(),
                AssemblyJsonValue::Array(vec![
                    AssemblyJsonValue::Integer(1),
                    AssemblyJsonValue::String("two".into()),
                ]),
            ),
            ("aleph".into(), AssemblyJsonValue::Object(Vec::new())),
        ])
    );
}

#[test]
fn floats_are_not_json_values() {
    let contents = format!(
        r#"{BASE}
[[image.assembly.files]]
tree = "boot"
dest = "manifest.json"
json = {{ x = 1.5 }}
"#
    );
    let path = write_temp_config(&contents);
    let result = try_resolve_config(path.to_str().expect("utf-8 path"));
    assert!(result.is_err(), "a float should be rejected");
}

#[test]
fn content_and_json_strings_interpolate_config_and_keep_assembly_tokens() {
    let contents = format!(
        r#"{BASE}
[[image.assembly.files]]
tree = "boot"
dest = "cmdline.txt"
content = "name=${{build.name}} root=${{assembly.size:$assembly.work/rootfs.erofs}}"

[[image.assembly.files]]
tree = "boot"
dest = "state.json"
json = {{ "${{build.name}}" = "${{build.name}}", size = "${{assembly.size:$assembly.work/rootfs.erofs}}" }}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let assembly = spec.image.assembly.expect("assembly");
    assert_eq!(
        assembly.files[0].content.as_deref(),
        Some("name=acme root=${assembly.size:$assembly.work/rootfs.erofs}")
    );
    let json = assembly.files[1].json.clone().expect("json");
    assert_eq!(
        json,
        AssemblyJsonValue::Object(vec![
            ("acme".into(), AssemblyJsonValue::String("acme".into())),
            (
                "size".into(),
                AssemblyJsonValue::String("${assembly.size:$assembly.work/rootfs.erofs}".into()),
            ),
        ])
    );
}

#[test]
fn archive_members_take_json_and_a_generated_member_without_entries_is_empty() {
    let contents = format!(
        r#"{BASE}
[[image.assembly.archives]]
id = "bundle"
output = "$assembly.work/bundle.tar"

[[image.assembly.archives.generated]]
name = "empty.env"

[[image.assembly.archives.generated]]
name = "slot.json"
json = {{ schema = 1 }}

[[image.assembly.archives.members]]
name = "manifest.json"
json = {{ id = "${{build.name}}" }}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let archive = &spec.image.assembly.expect("assembly").archives[0];
    assert_eq!(archive.members.len(), 3);
    assert!(matches!(
        archive.members[0].source(),
        Some(AssemblyArchiveMemberSourceSpec::Generated(entries)) if entries.is_empty()
    ));
    assert!(matches!(
        archive.members[1].source(),
        Some(AssemblyArchiveMemberSourceSpec::Json(_))
    ));
    assert_eq!(
        archive.members[2].json,
        Some(AssemblyJsonValue::Object(vec![(
            "id".into(),
            AssemblyJsonValue::String("acme".into())
        )]))
    );
}

#[test]
fn listed_binaries_take_their_fields_and_default_nothing_else() {
    let contents = format!(
        r#"{BASE}
[[image.assembly.binaries]]
tree = "boot"
src = "$provider.buildroot_output/target/usr/sbin/e2fsck"
dest = "usr/sbin/e2fsck"
sysroot = "$provider.buildroot_output/target"
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let binary = &spec.image.assembly.expect("assembly").binaries[0];
    assert_eq!(binary.tree.as_str(), "boot");
    assert_eq!(
        binary.src.as_str(),
        "$provider.buildroot_output/target/usr/sbin/e2fsck"
    );
    assert_eq!(binary.dest, "usr/sbin/e2fsck");
    assert_eq!(
        binary.sysroot.as_ref().map(|path| path.as_str()),
        Some("$provider.buildroot_output/target")
    );
    assert_eq!(binary.mode, None);
}

/// `${build.date}` is the build's commit time as RFC 3339 UTC, read from the
/// git `HEAD` of the build file's repository. Skipped when the environment
/// sets `SOURCE_DATE_EPOCH` (it wins) or git is not available.
#[test]
fn build_date_is_the_head_commit_time_as_rfc3339_utc() {
    use gaia_testkit::TempDir;
    use std::process::Command;
    if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
        eprintln!("skipped: SOURCE_DATE_EPOCH is set");
        return;
    }
    let scratch = TempDir::new("gaia-config-build-date");
    let repo = scratch.join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "gaia")
            .env("GIT_AUTHOR_EMAIL", "gaia@example.invalid")
            .env("GIT_COMMITTER_NAME", "gaia")
            .env("GIT_COMMITTER_EMAIL", "gaia@example.invalid")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .status()
    };
    if !matches!(git(&["init", "-q"]), Ok(status) if status.success()) {
        eprintln!("skipped: git is not available");
        return;
    }
    let config = repo.join("build.toml");
    let contents = format!(
        r#"{BASE}
[[image.assembly.files]]
tree = "boot"
dest = "built.txt"
content = "${{build.date}}"
"#
    );
    support::write_temp_config_at(&config, &contents);
    assert!(git(&["add", "build.toml"]).is_ok_and(|status| status.success()));
    assert!(git(&["commit", "-q", "-m", "one"]).is_ok_and(|status| status.success()));

    let spec = resolve_config(config.to_str().expect("utf-8 path"));
    let assembly = spec.image.assembly.expect("assembly");
    assert_eq!(
        assembly.files[0].content.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
}
