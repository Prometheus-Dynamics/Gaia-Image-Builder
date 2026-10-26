pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{ImageDefinition, ImageProviderKind};
use support::write_temp_config;

#[test]
fn kind_none_resolves_to_an_image_definition_with_no_provider() {
    let path = write_temp_config(
        r#"
build_name = "image-none"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "none"

[image.output]
collect_dir = "${workspace.out_dir}"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    assert!(matches!(spec.image.definition, ImageDefinition::None));
    assert_eq!(spec.image.provider_kind(), ImageProviderKind::None);
    assert_eq!(spec.image.provider_kind().as_str(), "none");
    assert!(spec.image.assembly.is_none());
    assert!(spec.image.feed.install_entries.is_empty());
    assert!(spec.image.checks.is_default());
    assert!(spec.image.hardware.is_empty());
}
