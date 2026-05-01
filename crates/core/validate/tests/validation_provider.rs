pub mod support;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_config::resolve_config;
use gaia_image_providers::ImageProviderCatalog;
use gaia_source_providers::SourceProviderCatalog;
use gaia_validate::validate_spec_with_providers;
use std::fs;
use support::{DuplicatePrepareImageProvider, write_temp_config};

#[test]
fn provider_prepare_image_plan_is_validation_error() {
    let path = write_temp_config(
        r#"
build_name = "prepare-provider"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.output]
collect_dir = "out/images"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let source_catalog = SourceProviderCatalog::new();
    let artifact_catalog = ArtifactProviderCatalog::new();
    let mut image_catalog = ImageProviderCatalog::new();
    image_catalog.register(Box::new(DuplicatePrepareImageProvider));

    let report =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "image_provider_prepare_count_invalid")
    );
    assert!(
        report
            .errors
            .iter()
            .any(|message| message.contains("planned 2 prepare operations"))
    );

    let _ = fs::remove_file(path);
}
