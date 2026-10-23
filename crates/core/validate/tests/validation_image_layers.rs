pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

fn codes(layers: &str) -> Vec<&'static str> {
    let contents = format!(
        r#"
build_name = "layers"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

{layers}
"#
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let _ = fs::remove_file(path);
    validate_spec(&spec)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn erofs_options_may_tune_compression_but_not_what_gaia_sets() {
    let tuned = codes(
        r#"[image.layers]
order = ["base", "app"]
erofs_options = ["-zlzma,level=6", "-C1048576", "-Eall-fragments,dedupe,ztailpacking"]

[image.layers.app]
paths = ["/usr/lib/app"]
erofs_options = ["-zlzma,level=6", "-C262144"]
"#,
    );
    assert!(
        !tuned.contains(&"image_layers_erofs_option_reserved"),
        "{tuned:?}"
    );

    for reserved in ["--tar=f", "--all-root", "-T0", "-Uclear", "-x-1"] {
        let found = codes(&format!(
            "[image.layers]\norder = [\"base\", \"app\"]\n\n[image.layers.app]\npaths = [\"/usr/lib/app\"]\nerofs_options = [\"{reserved}\"]\n"
        ));
        assert!(
            found.contains(&"image_layers_erofs_option_reserved"),
            "{reserved}: {found:?}"
        );
    }
}
