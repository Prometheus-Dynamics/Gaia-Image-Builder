pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use support::write_temp_config;

/// The diagnostic codes of a build whose `[[image.hardware]]` tables are `hardware`,
/// plus the hardware entries the resolved spec carries.
fn codes_for(hardware: &str) -> (Vec<String>, usize) {
    let path = write_temp_config(&format!(
        r#"
build_name = "hardware-check"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "raspberrypi4_64_defconfig"

{hardware}
"#
    ));
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let _ = std::fs::remove_file(path);
    let codes = validate_spec(&spec)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    (codes, spec.image.hardware.len())
}

#[test]
fn a_hardware_list_is_known_config_and_reaches_the_spec() {
    let (codes, entries) = codes_for(
        "[[image.hardware]]\nid = \"ov9782\"\nkind = \"camera-sensor\"\nmodules = [\"ov9282\"]\n\
         compatible = [\"ovti,ov9782\"]\noverlays = [\"ov9782\"]\n\
         [[image.hardware]]\nid = \"usb-gadget\"\nkind = \"usb-device\"\nmodules = [\"dwc2\"]\n",
    );
    assert_eq!(entries, 2);
    assert!(
        !codes.iter().any(|code| code == "config_unknown_key"),
        "{codes:?}"
    );
    assert!(
        !codes.iter().any(|code| code.starts_with("image_hardware_")),
        "{codes:?}"
    );
}

#[test]
fn a_hardware_id_listed_twice_is_an_error() {
    let (codes, _) = codes_for(
        "[[image.hardware]]\nid = \"fan\"\nmodules = [\"pwm-fan\"]\n\
         [[image.hardware]]\nid = \"fan\"\nmodules = [\"pwm-fan\"]\n",
    );
    assert!(
        codes
            .iter()
            .any(|code| code == "image_hardware_id_duplicate"),
        "{codes:?}"
    );
}

#[test]
fn a_hardware_list_on_a_starting_point_image_is_an_error_not_ignored() {
    let path = write_temp_config(
        r#"
build_name = "hardware-starting-point"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/gaia-missing-rootfs"
rootfs_validation_mode = "allow-missing"
output_mode = "copy-rootfs"

[[image.hardware]]
id = "fan"
modules = ["pwm-fan"]
"#,
    );
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let _ = std::fs::remove_file(path);
    let codes: Vec<String> = validate_spec(&spec)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    assert!(
        codes
            .iter()
            .any(|code| code == "image_hardware_not_buildroot"),
        "{codes:?}"
    );
}

#[test]
fn a_hardware_entry_without_an_id_is_an_error() {
    let (codes, _) = codes_for("[[image.hardware]]\nmodules = [\"pwm-fan\"]\n");
    assert!(
        codes.iter().any(|code| code == "image_hardware_id_missing"),
        "{codes:?}"
    );
}
