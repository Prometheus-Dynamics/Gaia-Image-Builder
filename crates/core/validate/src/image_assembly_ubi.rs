//! `[[image.assembly.ubis]]` entries: ids and outputs, the flash geometry,
//! the volume set, the flash size and bad-block reserve, and the images the
//! volumes hold. Whether the written volumes fit `flash_size` is checked
//! when the image is written, once the images' sizes are known.

use std::collections::HashSet;

use gaia_spec::{AssemblyTreeId, ImageAssemblySpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::{uses_supported_provider_variable, validate_assembly_path_template};

/// The largest bad-block reserve Linux accepts (`MTD_UBI_BEB_LIMIT` caps
/// `ubi.mtd=...,<limit>` at 768 per 1024).
const BAD_PEBS_PER_1024_MAX: u32 = 768;

pub(crate) fn validate_ubis(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.ubis".into());
    let volume_location = || Some("image.assembly.ubis.volumes".into());
    let generated: HashSet<&gaia_spec::AssemblyPathTemplate> = assembly
        .filesystems
        .iter()
        .map(|filesystem| &filesystem.output)
        .chain(assembly.transforms.iter().map(|transform| &transform.dest))
        .collect();
    let mut ids = HashSet::new();
    for ubi in &assembly.ubis {
        if ubi.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_ubi_id_empty",
                "assembly ubi id cannot be empty".into(),
                location(),
            ));
        } else if !ids.insert(ubi.id.as_str()) {
            diagnostics.push(error(
                "assembly_ubi_duplicate",
                format!("assembly ubi '{}' is declared more than once", ubi.id),
                location(),
            ));
        }
        if ubi.output.trim().is_empty() {
            diagnostics.push(error(
                "assembly_ubi_output_empty",
                format!("assembly ubi '{}' output cannot be empty", ubi.id),
                location(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &ubi.output,
                "image.assembly.ubis.output",
                diagnostics,
            );
        }
        match ubi.geometry() {
            Ok(geometry) => {
                for problem in ubi.volume_problems(&geometry) {
                    diagnostics.push(error(
                        "assembly_ubi_volume_invalid",
                        format!("assembly ubi '{}': {problem}", ubi.id),
                        volume_location(),
                    ));
                }
            }
            Err(problem) => diagnostics.push(error(
                "assembly_ubi_geometry_invalid",
                format!("assembly ubi '{}': {problem}", ubi.id),
                location(),
            )),
        }
        if ubi.volumes.is_empty() {
            diagnostics.push(error(
                "assembly_ubi_volumes_empty",
                format!("assembly ubi '{}' has no volumes", ubi.id),
                location(),
            ));
        }
        if let Err(problem) = ubi.parsed_flash_size() {
            diagnostics.push(error(
                "assembly_ubi_flash_size_invalid",
                format!("assembly ubi '{}': {problem}", ubi.id),
                location(),
            ));
        }
        if ubi.bad_pebs_per_1024 > BAD_PEBS_PER_1024_MAX {
            diagnostics.push(error(
                "assembly_ubi_bad_pebs_invalid",
                format!(
                    "assembly ubi '{}' bad_pebs_per_1024 {} is more than Linux's limit of {BAD_PEBS_PER_1024_MAX}",
                    ubi.id, ubi.bad_pebs_per_1024
                ),
                location(),
            ));
        }
        for volume in &ubi.volumes {
            let Some(image) = &volume.image else {
                continue;
            };
            if image.trim().is_empty() {
                diagnostics.push(error(
                    "assembly_ubi_volume_image_empty",
                    format!(
                        "assembly ubi '{}' volume '{}' image cannot be empty",
                        ubi.id, volume.name
                    ),
                    volume_location(),
                ));
                continue;
            }
            if !generated.contains(image)
                && !uses_supported_provider_variable(spec, image)
                && !image.starts_with("@assets/")
            {
                diagnostics.push(error(
                    "assembly_ubi_volume_image_unknown",
                    format!(
                        "assembly ubi '{}' volume '{}' image '{}' does not reference a generated filesystem or transform output or a supported provider root",
                        ubi.id, volume.name, image
                    ),
                    volume_location(),
                ));
            }
            validate_assembly_path_template(
                spec,
                tree_ids,
                image,
                "image.assembly.ubis.volumes.image",
                diagnostics,
            );
        }
    }
}
