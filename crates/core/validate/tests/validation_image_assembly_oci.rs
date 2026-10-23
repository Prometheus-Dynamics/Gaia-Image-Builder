pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A buildroot image with one tree (`app`) and the given `[image.assembly]`
/// extras.
fn config(extra: &str) -> String {
    format!(
        r#"
build_name = "oci"
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
id = "app"
path = "$assembly.work/app"

{extra}
"#
    )
}

/// The `assembly_` diagnostic codes the config raises.
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

/// One OCI image: `keys` are its own lines (after `id` and `dest`), `layers`
/// its layer line, and `config_lines` its `[image.assembly.oci.config]` table.
fn oci(id: &str, dest: &str, keys: &str, layers: &str, config_lines: &str) -> String {
    format!(
        r#"
[[image.assembly.oci]]
id = "{id}"
dest = "{dest}"
{keys}
{layers}

[image.assembly.oci.config]
{config_lines}
"#
    )
}

const LAYERS: &str = r#"layers = [ { tar = "$provider.images/base.tar" }, { tree = "app" } ]"#;
const AMD64: &str = r#"architecture = "amd64""#;

#[test]
fn a_complete_oci_image_validates() {
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            r#"ref = "sim:dev""#,
            LAYERS,
            r#"architecture = "amd64"
os = "linux"
entrypoint = ["/sbin/init"]
env = ["container=docker"]
working_dir = "/"
stop_signal = "SIGRTMIN+3"
labels = { "org.opencontainers.image.revision" = "abc" }"#,
        )),
        Vec::<&str>::new()
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.tar",
            "format = \"docker-archive\"\ncompression = \"none\"",
            LAYERS,
            AMD64,
        )),
        Vec::<&str>::new()
    );
}

#[test]
fn oci_images_need_an_architecture_unique_ids_and_layers() {
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            LAYERS,
            "os = \"linux\""
        )),
        vec!["assembly_oci_architecture_missing"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            LAYERS,
            "architecture = \"riscv64\""
        )),
        vec!["assembly_oci_architecture_invalid"]
    );
    let twice = format!(
        "{}{}",
        oci("sim", "$assembly.out/a.oci", "", LAYERS, AMD64),
        oci("sim", "$assembly.out/b.oci", "", LAYERS, AMD64)
    );
    assert_eq!(codes(&twice), vec!["assembly_oci_duplicate"]);
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            "layers = []",
            AMD64
        )),
        vec!["assembly_oci_layers_empty"]
    );
}

#[test]
fn oci_layers_name_exactly_one_known_source() {
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            r#"layers = [ { tar = "$provider.images/base.tar", tree = "app" } ]"#,
            AMD64,
        )),
        vec!["assembly_oci_layer_source_invalid"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            r#"layers = [ { tree = "missing" } ]"#,
            AMD64,
        )),
        vec!["assembly_oci_layer_tree_unknown"]
    );
}

#[test]
fn oci_formats_compressions_and_dests_must_agree() {
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "format = \"tar\"",
            LAYERS,
            AMD64
        )),
        vec!["assembly_oci_format_invalid"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.tar",
            "format = \"docker-archive\"\ncompression = \"zstd\"",
            LAYERS,
            AMD64,
        )),
        vec!["assembly_oci_compression_unsupported"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "format = \"docker-archive\"",
            LAYERS,
            AMD64,
        )),
        vec!["assembly_oci_dest_not_tar"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "compression = \"lz4\"",
            LAYERS,
            AMD64,
        )),
        vec!["assembly_oci_compression_invalid"]
    );
}

#[test]
fn oci_config_values_are_checked() {
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            LAYERS,
            "architecture = \"amd64\"\nenv = [\"NOEQUALS\"]",
        )),
        vec!["assembly_oci_env_invalid"]
    );
    assert_eq!(
        codes(&oci(
            "sim",
            "$assembly.out/sim.oci",
            "",
            LAYERS,
            "architecture = \"amd64\"\nworking_dir = \"relative\"",
        )),
        vec!["assembly_oci_working_dir_invalid"]
    );
}
