//! The libraries the flash node names in its step key are the ones its
//! Cargo.toml pins. A new version fails here until the pin and the key both move.

#[test]
fn the_key_names_the_pinned_libraries() {
    gaia_step::pins::check_pins(env!("CARGO_MANIFEST_DIR"), gaia_flash_image::LIBS)
        .expect("Cargo.toml");
}
