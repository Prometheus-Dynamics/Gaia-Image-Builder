pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, validate_spec};
use std::fs;
use support::write_temp_config;

#[test]
fn exclude_packages_must_be_well_formed_and_unique() {
    let path = write_temp_config(
        r#"
build_name = "rootfs-exclude-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.rootfs]
exclude_packages = ["libcamera", "Not A Package", "libcamera"]
"#,
    );
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);

    let has = |code: &str| {
        report
            .diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error && d.code == code)
    };
    assert!(has("buildroot_rootfs_exclude_package_invalid"));
    assert!(has("buildroot_rootfs_exclude_package_duplicate"));
}
