use super::*;
use std::path::PathBuf;

fn read(text: &str) -> Result<(ConfigVersion, toml::Value, V3FileUses), String> {
    let mut value: toml::Value = toml::from_str(text).expect("toml");
    read_version(&PathBuf::from("build.toml"), text, &mut value)
        .map(|(version, uses)| (version, value, uses))
        .map_err(|error| error.to_string())
}

#[test]
fn a_file_without_config_is_v2_and_left_alone() {
    let text = "[workspace]\nout_dir = \"out\"\n";
    let (version, value, _) = read(text).unwrap();
    assert_eq!(version, ConfigVersion::V2);
    assert_eq!(value, toml::from_str::<toml::Value>(text).unwrap());
}

#[test]
fn outputs_cache_and_tokens_are_read_in_the_v2_shape() {
    let (version, value, uses) = read(
        r#"
config = 3

[outputs]
dir = "gaia/output/${build.name}"
images_dir = "${outputs.dir}/images"
report = true

[cache]
ccache = { enabled = true, max_size = "50G" }
packages = { enabled = true }
sccache = true

[providers.buildroot]
work_dir = "ram"

[[image.assembly.files]]
tree = "rootfs"
src = "${assets}/motd"
dest = "etc/motd"
"#,
    )
    .unwrap();
    assert_eq!(version, ConfigVersion::V3);
    let expected: toml::Value = toml::from_str(
        r#"
[workspace]
out_dir = "gaia/output/${build.name}"

[image.output]
collect_dir = "${workspace.out_dir}/images"
emit_report = true

[providers.buildroot]
work_dir = "ram"
ccache = { enabled = true, max_size = "50G" }
package_cache = { enabled = true }

[providers.rust]
sccache = true

[[image.assembly.files]]
tree = "rootfs"
src = "@assets/motd"
dest = "etc/motd"
"#,
    )
    .unwrap();
    assert_eq!(value, expected);
    assert_eq!(
        uses.aliases,
        [("assets".to_string(), "build.toml:19:1".to_string())]
    );
}

#[test]
fn v2_keys_and_tokens_in_a_v3_file_are_errors_at_their_line() {
    for (text, expected) in [
        (
            "config = 3\n[workspace]\nout_dir = \"out\"\n",
            "build.toml:3:1: `workspace.out_dir` is `outputs.dir` in config v3",
        ),
        (
            "config = 3\n[image.output]\nemit_report = true\n",
            "build.toml:3:1: `image.output.emit_report` is `outputs.report`",
        ),
        (
            "config = 3\n[providers.buildroot.ccache]\nenabled = true\n",
            "build.toml:2:1: `providers.buildroot.ccache` is `cache.ccache`",
        ),
        (
            "config = 3\n[outputs]\nfolder = \"x\"\n",
            "build.toml:3:1: `outputs.folder` is not a key of `[outputs]`",
        ),
        (
            "config = 3\n[stage]\n\n[[stage.files]]\nsrc = \"@self/x\"\n",
            "build.toml:5:1: `@self` is written `${self}`",
        ),
        (
            "config = 4\n",
            "build.toml:1:1: config = 4 is not a config version",
        ),
        (
            "config = 3\n[[workspace.named_paths]]\nalias = \"build\"\npath = \"x\"\n",
            "alias 'build' cannot be used",
        ),
    ] {
        let error = read(text).expect_err(text);
        assert!(error.contains(expected), "{text}\n{error}");
    }
}
