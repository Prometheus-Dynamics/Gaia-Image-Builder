use super::*;

fn digest_of(config: &str) -> String {
    let dir = temp_path("buildroot-config-digest");
    fs::create_dir_all(&dir).expect("output dir");
    fs::write(dir.join(".config"), config).expect("config");
    let digest = buildroot_config_digest(&dir).expect("digest");
    let _ = fs::remove_dir_all(dir);
    digest
}

const BASE: &str = "#\n# Automatically generated file; DO NOT EDIT.\n# Buildroot 2025.02 Configuration\n#\nBR2_aarch64=y\nBR2_DL_DIR=\"/work/a/dl\"\nBR2_JLEVEL=0\n# BR2_PACKAGE_FFMPEG is not set\n";

#[test]
fn version_header_and_cache_locations_do_not_change_the_digest() {
    let moved = BASE
        .replace(
            "Buildroot 2025.02 Configuration",
            "Buildroot 2025.02-12-gabc123 Configuration",
        )
        .replace("/work/a/dl", "/home/user/.cache/gaia/dl")
        .replace("BR2_JLEVEL=0", "BR2_JLEVEL=16");

    assert_eq!(digest_of(BASE), digest_of(&moved));
}

#[test]
fn package_selection_changes_the_digest() {
    let enabled = BASE.replace("# BR2_PACKAGE_FFMPEG is not set", "BR2_PACKAGE_FFMPEG=y");

    assert_ne!(digest_of(BASE), digest_of(&enabled));
}
