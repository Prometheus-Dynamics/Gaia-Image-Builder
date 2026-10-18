//! A `${...}` reference that leads back to a token being resolved is a config
//! error naming the cycle, not a stack overflow.

pub mod support;

use support::write_temp_config;

fn resolve_error(contents: &str) -> String {
    let config = write_temp_config(contents);
    let result = gaia_config::try_resolve_config(config.as_path().to_str().expect("utf-8 path"));
    result
        .expect_err("interpolation cycle should fail")
        .to_string()
}

#[test]
fn two_token_cycle_through_build_names_is_reported() {
    let message = resolve_error(
        r#"
build_name = "x-${build.display_name}"
display_name = "${build.name}"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"
"#,
    );

    assert!(
        message.contains("interpolation cycle"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("build.name") && message.contains("build.display_name"),
        "cycle should name both tokens: {message}"
    );
}

#[test]
fn self_reference_through_interpolation_value_is_reported() {
    let message = resolve_error(
        r#"
build_name = "${interpolation.values.loop}"
version = "1.0.0"

[interpolation]
values = [["loop", "a-${interpolation.values.loop}"]]

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"
"#,
    );

    assert!(
        message.contains(
            "interpolation cycle: interpolation.values.loop -> interpolation.values.loop"
        ),
        "unexpected error: {message}"
    );
}

#[test]
fn repeated_token_without_a_cycle_still_resolves() {
    let config = write_temp_config(
        r#"
build_name = "app"
display_name = "${build.name}-${build.name}"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"
"#,
    );

    gaia_config::try_resolve_config(config.as_path().to_str().expect("utf-8 path"))
        .expect("a token used twice is not a cycle");
}
