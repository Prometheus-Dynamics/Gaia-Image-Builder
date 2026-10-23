//! `[image.layers]` validation.

use gaia_spec::{ImageDefinition, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image::is_buildroot_package_name;

/// `[image.layers]`: the names, the default, the overrides and the selector
/// ids. Ids that name nothing are errors only where they are not globs (a
/// glob may match nothing in a build that does not use it).
pub(super) fn validate_image_layers(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Some(layers) = &spec.image.layers else {
        return;
    };
    let location = Some("image.layers".to_string());
    if !matches!(spec.image.definition, ImageDefinition::Buildroot(_)) {
        diagnostics.push(error(
            "image_layers_kind_unsupported",
            "image.layers needs a buildroot image: layers are split from its rootfs.tar".into(),
            location.clone(),
        ));
    }
    if layers.order.is_empty() {
        diagnostics.push(error(
            "image_layers_order_missing",
            "image.layers order lists no layer".into(),
            location.clone(),
        ));
    }
    for (index, name) in layers.order.iter().enumerate() {
        if name.trim().is_empty() || layers.order[..index].contains(name) {
            diagnostics.push(error(
                "image_layers_name_invalid",
                format!("image.layers order names '{name}' more than once, or an empty name"),
                location.clone(),
            ));
        }
    }
    if !layers.order.contains(&layers.default) {
        diagnostics.push(error(
            "image_layers_default_unknown",
            format!("image.layers default '{}' is not in order", layers.default),
            location.clone(),
        ));
    }
    for layer in &layers.layers {
        if !layers.order.contains(&layer.name) {
            diagnostics.push(error(
                "image_layers_layer_unknown",
                format!(
                    "image.layers.{} is not in order: add it to order or remove the table",
                    layer.name
                ),
                location.clone(),
            ));
        }
        for package in &layer.packages {
            if !is_buildroot_package_glob(package) {
                diagnostics.push(error(
                    "image_layers_package_invalid",
                    format!(
                        "image.layers.{} packages entry '{package}' is not a Buildroot package name",
                        layer.name
                    ),
                    location.clone(),
                ));
            }
        }
        for path in &layer.paths {
            if !path.starts_with('/') {
                diagnostics.push(error(
                    "image_layers_path_relative",
                    format!(
                        "image.layers.{} paths entry '{path}' must be an absolute rootfs path",
                        layer.name
                    ),
                    location.clone(),
                ));
            }
        }
        let feed_ids = |ids: &[String], known: Vec<&str>, what: &str, code: &'static str| {
            ids.iter()
                .filter(|id| !id.contains('*') && !known.contains(&id.as_str()))
                .map(|id| {
                    error(
                        code,
                        format!(
                            "image.layers.{} {what} '{id}' names no {what} in this build",
                            layer.name
                        ),
                        Some("image.layers".to_string()),
                    )
                })
                .collect::<Vec<_>>()
        };
        diagnostics.extend(feed_ids(
            &layer.installs,
            spec.install.entries.iter().map(|e| e.id.as_str()).collect(),
            "installs",
            "image_layers_install_unknown",
        ));
        diagnostics.extend(feed_ids(
            &layer.stage,
            spec.stage.files.iter().map(|f| f.id.as_str()).collect(),
            "stage",
            "image_layers_stage_unknown",
        ));
        diagnostics.extend(feed_ids(
            &layer.services,
            spec.stage.services.iter().map(|s| s.id.as_str()).collect(),
            "services",
            "image_layers_service_unknown",
        ));
    }
    for (index, path) in layers.overrides.iter().enumerate() {
        if !path.starts_with('/') || layers.overrides[..index].contains(path) {
            diagnostics.push(error(
                "image_layers_override_invalid",
                format!(
                    "image.layers overrides entry '{path}' must be an absolute path, listed once"
                ),
                location.clone(),
            ));
        }
    }
    if layers.verity && !layers.erofs {
        diagnostics.push(error(
            "image_layers_verity_needs_erofs",
            "image.layers verity = true needs erofs = true: the verity tree covers the erofs image"
                .into(),
            location,
        ));
    }
}

/// A Buildroot package name, or a glob over them (`linux-ext-*`).
fn is_buildroot_package_glob(name: &str) -> bool {
    let plain: String = name.chars().filter(|c| *c != '*').collect();
    plain.is_empty() || is_buildroot_package_name(&plain) || plain.ends_with('-')
}
