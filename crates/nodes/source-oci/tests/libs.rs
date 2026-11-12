//! The libraries this node names in its step key: the pinned ones are the
//! versions its Cargo.toml pins, and libzstd is the version the linked library
//! reports.

use gaia_source_oci::{LIBS, runtime_libs};

#[test]
fn the_key_names_the_pinned_libraries() {
    gaia_step::pins::check_pins(env!("CARGO_MANIFEST_DIR"), LIBS).expect("Cargo.toml");
}

#[test]
fn the_key_names_the_version_libzstd_reports() {
    let number = zstd::zstd_safe::version_number();
    let text = format!(
        "{}.{}.{}",
        number / 10_000,
        number / 100 % 100,
        number % 100
    );
    assert_eq!(runtime_libs(), [("libzstd", text.as_str())]);
}
