pub mod support;

use gaia_plan::{OperationKind, operation_fingerprint};
use gaia_spec::ImageDefinition;
use support::test_spec;

/// Leaving packages out of the rootfs is applied by image:build's make, so
/// prepare's fingerprint ignores it and build's changes with it.
#[test]
fn rootfs_exclusion_changes_only_the_build_fingerprint() {
    let spec = test_spec();
    assert!(
        matches!(spec.image.definition, ImageDefinition::Buildroot(_)),
        "the default config builds a Buildroot image"
    );
    let mut excluding = spec.clone();
    if let ImageDefinition::Buildroot(buildroot) = &mut excluding.image.definition {
        buildroot.rootfs_exclude_packages = vec!["libcamera".into(), "libpisp".into()];
    }

    assert_eq!(
        operation_fingerprint(&spec, &OperationKind::PrepareImage),
        operation_fingerprint(&excluding, &OperationKind::PrepareImage)
    );
    assert_ne!(
        operation_fingerprint(&spec, &OperationKind::BuildImage),
        operation_fingerprint(&excluding, &OperationKind::BuildImage)
    );
}
