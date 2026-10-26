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
build_name = "packages"
version = "2.0.0"

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

/// One package. `id`, `format` and `tree` are its keys, `control` and
/// `scripts` the lines of its `control` and `scripts` tables.
fn package(id: &str, format: &str, tree: &str, control: &str, scripts: &str) -> String {
    format!(
        r#"
[[image.assembly.packages]]
id = "{id}"
format = "{format}"
tree = "{tree}"
dest = "$assembly.out/"
allowed_prefixes = ["/usr/local", "/etc/systemd/system"]

[image.assembly.packages.control]
{control}

[image.assembly.packages.scripts]
{scripts}
"#
    )
}

const CONTROL: &str = r#"Package = "helios"
Version = "${build.version}"
Architecture = "aarch64"
Maintainer = "Gaia Tests <tests@example.invalid>"
Description = "Helios SystemCore services"
Depends = ["libc6 (>= 2.38)"]
extra = { "X-Port" = "5801" }"#;

const SCRIPTS: &str = r#"services = ["helios-api.service"]"#;

#[test]
fn a_complete_package_validates() {
    assert_eq!(
        codes(&package("helios", "ipk", "app", CONTROL, SCRIPTS)),
        Vec::<&str>::new()
    );
}

#[test]
fn the_output_name_follows_the_control_fields_and_the_version() {
    let path = write_temp_config(&config(&package("helios", "ipk", "app", CONTROL, SCRIPTS)));
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let _ = fs::remove_file(path);
    let assembly = spec.image.assembly.as_ref().expect("assembly");
    let package = &assembly.packages[0];
    assert_eq!(
        package.output.as_str(),
        "$assembly.out/helios_2.0.0_aarch64.ipk"
    );
    assert!(package.debian_binary, "debian-binary defaults to on");
    assert_eq!(package.format, "ipk");
}

#[test]
fn architecture_has_no_default() {
    let control = CONTROL.replace("Architecture = \"aarch64\"\n", "");
    assert_eq!(
        codes(&package("helios", "ipk", "app", &control, SCRIPTS)),
        vec!["assembly_package_architecture_missing"]
    );
}

#[test]
fn the_required_control_fields_are_reported_when_missing() {
    let control = CONTROL.replace("Maintainer = \"Gaia Tests <tests@example.invalid>\"\n", "");
    assert_eq!(
        codes(&package("helios", "ipk", "app", &control, SCRIPTS)),
        vec!["assembly_package_control_missing"]
    );
}

#[test]
fn the_format_is_ipk_or_deb_and_a_deb_keeps_debian_binary() {
    assert!(codes(&package("helios", "deb", "app", CONTROL, SCRIPTS)).is_empty());
    let without = package("helios", "deb", "app", CONTROL, SCRIPTS).replace(
        "dest = \"$assembly.out/\"\n",
        "dest = \"$assembly.out/\"\ndebian_binary = false\n",
    );
    assert_eq!(
        codes(&without),
        vec!["assembly_package_deb_needs_debian_binary"]
    );
    assert_eq!(
        codes(&package("helios", "rpm", "app", CONTROL, SCRIPTS)),
        vec!["assembly_package_format_invalid"]
    );
}

#[test]
fn the_package_name_follows_the_debian_rule() {
    let control = CONTROL.replace("Package = \"helios\"", "Package = \"Helios\"");
    assert_eq!(
        codes(&package("helios", "ipk", "app", &control, SCRIPTS)),
        vec!["assembly_package_name_invalid"]
    );
}

#[test]
fn extra_field_names_are_checked_and_may_not_repeat_a_set_field() {
    let bad_name = CONTROL.replace("\"X-Port\"", "\"X_Port\"");
    assert_eq!(
        codes(&package("helios", "ipk", "app", &bad_name, SCRIPTS)),
        vec!["assembly_package_field_name_invalid"]
    );
    let reserved = CONTROL.replace("\"X-Port\" = \"5801\"", "\"Package\" = \"other\"");
    assert_eq!(
        codes(&package("helios", "ipk", "app", &reserved, SCRIPTS)),
        vec!["assembly_package_field_reserved"]
    );
}

#[test]
fn package_ids_are_unique_and_the_tree_must_exist() {
    let twice = format!(
        "{}{}",
        package("helios", "ipk", "app", CONTROL, SCRIPTS),
        package("helios", "ipk", "app", CONTROL, SCRIPTS)
    );
    assert_eq!(codes(&twice), vec!["assembly_package_duplicate"]);
    assert_eq!(
        codes(&package("helios", "ipk", "nope", CONTROL, SCRIPTS)),
        vec!["assembly_package_tree_unknown"]
    );
}

#[test]
fn allowed_prefixes_must_be_absolute() {
    let relative = package("helios", "ipk", "app", CONTROL, SCRIPTS)
        .replace("\"/usr/local\"", "\"usr/local\"");
    assert_eq!(codes(&relative), vec!["assembly_package_prefix_invalid"]);
}
