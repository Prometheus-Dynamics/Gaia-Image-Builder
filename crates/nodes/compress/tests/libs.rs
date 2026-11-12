//! The libraries the compress nodes name in their keys are the ones
//! Cargo.lock resolves.

use gaia_compress::libs;

fn check(set: &[(&str, &str)]) {
    gaia_step::cargo_lock::check_libs(env!("CARGO_MANIFEST_DIR"), set).expect("Cargo.lock");
}

#[test]
fn xz_names_the_resolved_encoder() {
    check(libs::XZ);
}

#[test]
fn zstd_names_the_resolved_encoder() {
    check(libs::ZSTD);
}

#[test]
fn gzip_names_the_resolved_encoder() {
    check(libs::GZIP);
}

#[test]
fn tar_names_the_resolved_writer() {
    check(libs::TAR);
}
