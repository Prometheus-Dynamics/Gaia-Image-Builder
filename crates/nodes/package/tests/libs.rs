//! The libraries this node names in its step key are the ones its Cargo.toml
//! pins. A new version fails here until the pin and the key both move.

#[test]
fn the_key_names_the_pinned_libraries() {
    gaia_step::pins::check_pins(env!("CARGO_MANIFEST_DIR"), gaia_package::LIBS)
        .expect("Cargo.toml");
}
