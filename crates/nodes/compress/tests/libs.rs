//! The libraries the compress nodes name in their keys: the pinned ones are
//! the versions Cargo.toml pins, and libzstd is the version the linked
//! library reports.

use gaia_compress::libs;
use gaia_step::pins::check_pins;

#[test]
fn xz_names_the_pinned_encoder() {
    check_pins(env!("CARGO_MANIFEST_DIR"), libs::XZ).expect("Cargo.toml");
}

#[test]
fn gzip_names_the_pinned_encoder() {
    check_pins(env!("CARGO_MANIFEST_DIR"), libs::GZIP).expect("Cargo.toml");
}

#[test]
fn tar_names_the_pinned_writer() {
    check_pins(env!("CARGO_MANIFEST_DIR"), libs::TAR).expect("Cargo.toml");
}

#[test]
fn zstd_names_the_version_libzstd_reports() {
    let number = zstd::zstd_safe::version_number();
    let text = format!(
        "{}.{}.{}",
        number / 10_000,
        number / 100 % 100,
        number % 100
    );
    assert_eq!(libs::libzstd(), [("libzstd", text.as_str())]);
}
