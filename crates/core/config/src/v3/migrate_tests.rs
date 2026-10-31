use super::*;
use std::path::PathBuf;

/// The v3 reader's view of a migrated file must be the v2 file's values,
/// exactly: the migration only changes spelling.
fn assert_round_trip(v2: &str) -> String {
    let v3 = migrate_config_text(v2).unwrap_or_else(|error| panic!("{error}\n{v2}"));
    let mut read: toml::Value = toml::from_str(&v3).unwrap_or_else(|error| panic!("{error}\n{v3}"));
    let (version, _) = super::super::read_version(&PathBuf::from("build.toml"), &v3, &mut read)
        .unwrap_or_else(|error| panic!("{error}\n{v3}"));
    assert_eq!(version, super::super::ConfigVersion::V3, "{v3}");
    // `${inputs.x}` is respelled `${input.x}`, which v2 reads the same.
    let original: toml::Value =
        toml::from_str(&v2.replace("${inputs.", "${input.")).expect("v2 parses");
    assert_eq!(read, original, "\n--- v3 ---\n{v3}");
    v3
}

#[test]
fn a_workspace_layer_moves_outputs_and_caches_keeping_comments() {
    let v2 = r#"# HeliOS workspace defaults.

[workspace]
root_dir = "../.."
# Every build's outputs, by build name.
out_dir = "gaia/output/${build.name}"

[[workspace.named_paths]]
alias = "assets"
path = "gaia/assets"
kind = "host"

[providers.buildroot]
work_dir = "ram"
download_dir = "/srv/dl"

[providers.buildroot.package_cache]
enabled = true

[providers.buildroot.host_tools]
# The build image's ccache instead of building host-ccache.
ccache = "system,build"

[providers.buildroot.ccache]
enabled = true
max_size = "50G"

[providers.rust]
sccache = true
"#;
    let v3 = assert_round_trip(v2);
    assert_eq!(
        v3,
        r#"# HeliOS workspace defaults.

config = 3

[workspace]
root_dir = "../.."

[outputs]
# Every build's outputs, by build name.
dir = "gaia/output/${build.name}"

[[workspace.named_paths]]
alias = "assets"
path = "gaia/assets"
kind = "host"

[providers.buildroot]
work_dir = "ram"

[cache]
downloads = "/srv/dl"
sccache = true

[cache.packages]
enabled = true

[providers.buildroot.host_tools]
# The build image's ccache instead of building host-ccache.
ccache = "system,build"

[cache.ccache]
enabled = true
max_size = "50G"

"#
    );
}

#[test]
fn image_output_becomes_outputs_and_tokens_are_respelled_everywhere() {
    let v2 = r#"build_name = "acme"
extends = "@self/../workspace.toml"

[image]
kind = "buildroot"
external_tree = "@self/br2:@source:bsp/br2"

[image.output]
# Collected next to the other outputs: $provider.images is the raw tree.
collect_dir = "${workspace.out_dir}/images"
emit_report = true

[image.assembly]
work_dir = "$provider.images/work"

[[image.assembly.files]]
tree = "rootfs"
src = "@assets/board/config.txt"
dest = "boot/config.txt"

[[image.assembly.archives]]
id = "manifest"
output = "$assembly.out/manifest.tar"
generated = [{ path = "sha", value = "${assembly.sha256:$assembly.work/rootfs.erofs}" }]
labels = ["${inputs.board}", "git@github.com:org/repo"]
"#;
    let v3 = assert_round_trip(v2);
    assert!(
        v3.starts_with("config = 3\nbuild_name = \"acme\"\n"),
        "{v3}"
    );
    for expected in [
        "extends = \"${self}/../workspace.toml\"",
        "external_tree = \"${self}/br2:${source.bsp}/br2\"",
        "[outputs]\n# Collected next to the other outputs: ${provider.images} is the raw tree.\n",
        "images_dir = \"${outputs.dir}/images\"\nreport = true\n",
        "work_dir = \"${provider.images}/work\"",
        "src = \"${assets}/board/config.txt\"",
        "\"${assembly.sha256:${assembly.work}/rootfs.erofs}\"",
        "\"${input.board}\", \"git@github.com:org/repo\"",
    ] {
        assert!(v3.contains(expected), "missing {expected:?} in\n{v3}");
    }
}

#[test]
fn a_file_without_top_level_keys_gets_config_after_its_comments() {
    let v3 =
        assert_round_trip("# Only tables.\n\n[workspace]\nout_dir = \"out\"\nroot_dir = \".\"\n");
    assert_eq!(
        v3,
        "# Only tables.\n\nconfig = 3\n\n[workspace]\nroot_dir = \".\"\n\n[outputs]\ndir = \"out\"\n\n"
    );
    let v3 = assert_round_trip("[stage]\n");
    assert_eq!(v3, "config = 3\n\n[stage]\n");
}

#[test]
fn a_table_left_empty_by_the_move_is_dropped() {
    let v3 =
        assert_round_trip("[workspace]\nout_dir = \"out\"\n\n[providers.rust]\nsccache = true\n");
    assert_eq!(
        v3,
        "config = 3\n\n[outputs]\ndir = \"out\"\n\n[cache]\nsccache = true\n\n"
    );
}

#[test]
fn a_v3_file_is_left_as_it_is() {
    let v3 = "config = 3\n\n[outputs]\ndir = \"out\"\n";
    assert_eq!(migrate_config_text(v3).expect("v3"), v3);
}

#[test]
fn reserved_aliases_and_dead_cache_tables_are_refused() {
    let error = migrate_config_text("src = \"@build/x\"\n").expect_err("reserved");
    assert!(error.contains("alias 'build'"), "{error}");
    let error =
        migrate_config_text("[providers.go.ccache]\nenabled = true\n").expect_err("go ccache");
    assert!(
        error.contains("[providers.go.ccache] has no effect"),
        "{error}"
    );
}

/// The `.toml` files under `dir`, skipping build output and VCS trees.
fn config_files(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.')
                && !matches!(name.as_ref(), "build" | "out" | "target" | "output")
            {
                config_files(&path, files);
            }
        } else if name.ends_with(".toml") {
            files.push(path);
        }
    }
}

/// Every config file shipped with Gaia (examples and docs fixtures) round
/// trips. `GAIA_MIGRATE_ROUND_TRIP_DIR` adds another tree, such as a
/// product's configs, read but never written.
#[test]
fn every_shipped_config_file_round_trips() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    config_files(&root.join("examples"), &mut files);
    if let Some(extra) = std::env::var_os("GAIA_MIGRATE_ROUND_TRIP_DIR") {
        config_files(std::path::Path::new(&extra), &mut files);
    }
    let mut checked = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).expect("readable");
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        // Cargo manifests and the like are not build files.
        if value.get("package").is_some()
            || value
                .get("workspace")
                .and_then(|w| w.get("members"))
                .is_some()
        {
            continue;
        }
        if value.get(super::super::VERSION_KEY).is_some() {
            // Already v3: it must read.
            let mut read = value.clone();
            super::super::read_version(&path, &text, &mut read)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            checked += 1;
            continue;
        }
        let v3 = migrate_config_text(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut read: toml::Value =
            toml::from_str(&v3).unwrap_or_else(|error| panic!("{}: {error}\n{v3}", path.display()));
        super::super::read_version(&path, &v3, &mut read)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let original: toml::Value =
            toml::from_str(&text.replace("${inputs.", "${input.")).expect("parses");
        assert_eq!(read, original, "{}\n{v3}", path.display());
        checked += 1;
    }
    assert!(checked > 10, "only {checked} files checked");
}
