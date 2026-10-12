pub mod support;

use gaia_plan::{OperationKind, operation_fingerprint};
use support::test_spec;

/// Renaming the collected image or its archive (an image flavour with
/// another name or version) changes nothing the prepare step does, so its
/// fingerprint stays; image:build collects and archives, so its changes.
#[test]
fn image_output_naming_changes_only_the_build_fingerprint() {
    let spec = test_spec();
    let mut renamed = spec.clone();
    renamed.image.output.archive_name = Some("other-flavour-1.2.3.tar".into());
    renamed.image.output.collect_dir = Some("/elsewhere/images".into());

    assert_eq!(
        operation_fingerprint(&spec, &OperationKind::PrepareImage),
        operation_fingerprint(&renamed, &OperationKind::PrepareImage)
    );
    assert_ne!(
        operation_fingerprint(&spec, &OperationKind::BuildImage),
        operation_fingerprint(&renamed, &OperationKind::BuildImage)
    );
}
