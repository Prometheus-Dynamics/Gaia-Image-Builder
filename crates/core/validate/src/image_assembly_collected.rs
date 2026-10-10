//! Assembly references into `$provider.images`. That directory holds only the
//! images the Buildroot provider collects (its `expected_images`), so a
//! reference to any other name would fail at assembly time. Names the assembly
//! produces itself (filesystem, disk or archive outputs) are not collected
//! inputs and are not checked.

use std::collections::HashSet;

use gaia_spec::{ImageDefinition, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::assembly_expected_image_names;

const PROVIDER_IMAGES: &str = "$provider.images";

/// Checks every `$provider.images/<name>` reference in `value`.
///
/// `label` names the field in the message (for example `files[3].src`) and
/// `path` is the config path of the diagnostic (for example
/// `image.assembly.files`). Only Buildroot images with a non-empty
/// `expected_images` list are checked.
pub(crate) fn validate_collected_reference(
    spec: &ResolvedBuildSpec,
    value: &str,
    label: &str,
    path: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        return;
    };
    if buildroot.expected_images.is_empty() {
        return;
    }
    let expected: HashSet<&str> = buildroot
        .expected_images
        .iter()
        .map(|image| image.name.as_str())
        .collect();
    let produced = assembly_expected_image_names(spec);
    for component in images_components(value) {
        if component.is_empty() {
            continue;
        }
        let collectable = match glob_literal_prefix(component) {
            None => expected.contains(component) || produced.contains(component),
            Some("") => true,
            Some(prefix) => {
                expected.iter().any(|name| name.starts_with(prefix))
                    || produced.iter().any(|name| name.starts_with(prefix))
            }
        };
        if !collectable {
            diagnostics.push(error(
                "assembly_provider_image_not_collected",
                format!(
                    "image.assembly {label} '{value}' is not collected: add '{component}' to the buildroot expected_images (or reference $provider.buildroot_output/images/{component})"
                ),
                Some(path.into()),
            ));
            return;
        }
    }
}

/// The first path component after each `$provider.images/` in `value`.
fn images_components(value: &str) -> Vec<&str> {
    let mut components = Vec::new();
    let mut rest = value;
    while let Some(index) = rest.find(PROVIDER_IMAGES) {
        let after = &rest[index + PROVIDER_IMAGES.len()..];
        rest = after;
        let Some(path) = after.strip_prefix('/') else {
            continue;
        };
        let end = path
            .find(|ch: char| matches!(ch, '/' | '}' | '$' | '"' | '\'') || ch.is_whitespace())
            .unwrap_or(path.len());
        components.push(&path[..end]);
    }
    components
}

/// `None` for a literal name; for a glob, the literal text before its first
/// wildcard character (possibly empty).
fn glob_literal_prefix(component: &str) -> Option<&str> {
    match component.find(['*', '?', '[', '{']) {
        None => None,
        Some(index) => Some(&component[..index]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_follow_each_provider_images_reference() {
        assert_eq!(
            images_components("$provider.images/flash-id"),
            vec!["flash-id"]
        );
        assert_eq!(
            images_components("${assembly.sha256:$provider.images/a.img/x}"),
            vec!["a.img"]
        );
        assert!(images_components("$provider.images").is_empty());
        assert!(images_components("$provider.imagesx/y").is_empty());
    }

    #[test]
    fn glob_prefix_is_the_text_before_the_first_wildcard() {
        assert_eq!(glob_literal_prefix("flash-id"), None);
        assert_eq!(glob_literal_prefix("flash-*.img"), Some("flash-"));
        assert_eq!(glob_literal_prefix("*.img"), Some(""));
    }
}
