//! The libraries this node names in its step key are the ones Cargo.lock
//! resolves. A new version fails here until the key names it.

#[test]
fn the_key_names_the_resolved_libraries() {
    gaia_step::cargo_lock::check_libs(env!("CARGO_MANIFEST_DIR"), gaia_package::LIBS)
        .expect("Cargo.lock");
}
