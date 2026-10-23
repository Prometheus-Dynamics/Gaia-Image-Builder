//! `[[image.assembly.oci]]` entries: ids, the destination, the format and
//! compression choices, the layers, and the image config.

use std::collections::HashSet;

use gaia_spec::{
    AssemblyOciCompressionSpec, AssemblyOciFormatSpec, AssemblyOciLayerSource, AssemblyTreeId,
    ImageAssemblySpec, ResolvedBuildSpec,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// The architectures an OCI config may name.
const OCI_ARCHITECTURES: [&str; 2] = ["amd64", "arm64"];

pub(crate) fn validate_ocis(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.oci".to_string());
    let mut ids = HashSet::new();
    for oci in &assembly.ocis {
        let label = format!("assembly oci image '{}'", oci.id);
        if oci.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_oci_id_empty",
                "assembly oci image id cannot be empty".into(),
                location(),
            ));
        } else if !ids.insert(oci.id.as_str()) {
            diagnostics.push(error(
                "assembly_oci_duplicate",
                format!("{label} is declared more than once"),
                location(),
            ));
        }

        if oci.dest.as_str().trim().is_empty() {
            diagnostics.push(error(
                "assembly_oci_dest_empty",
                format!("{label} dest cannot be empty"),
                location(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &oci.dest,
                "image.assembly.oci.dest",
                diagnostics,
            );
        }

        let format = AssemblyOciFormatSpec::parse(&oci.format);
        let compression = AssemblyOciCompressionSpec::parse(&oci.compression);
        match format {
            None => diagnostics.push(error(
                "assembly_oci_format_invalid",
                format!(
                    "{label} format '{}' is not known; use oci-layout or docker-archive",
                    oci.format
                ),
                location(),
            )),
            Some(format) => {
                let dest = oci.dest.as_str();
                let is_tar = dest.ends_with(".tar");
                if format == AssemblyOciFormatSpec::DockerArchive && !is_tar {
                    diagnostics.push(error(
                        "assembly_oci_dest_not_tar",
                        format!("{label} with format docker-archive needs a dest ending in .tar"),
                        location(),
                    ));
                }
                if format == AssemblyOciFormatSpec::OciLayout && is_tar {
                    diagnostics.push(error(
                        "assembly_oci_dest_is_tar",
                        format!(
                            "{label} with format oci-layout is a directory; use format = \"docker-archive\" for a .tar"
                        ),
                        location(),
                    ));
                }
            }
        }
        match compression {
            None => diagnostics.push(error(
                "assembly_oci_compression_invalid",
                format!(
                    "{label} compression '{}' is not known; use gzip, zstd or none",
                    oci.compression
                ),
                location(),
            )),
            Some(AssemblyOciCompressionSpec::Zstd)
                if oci.parsed_format() == Some(AssemblyOciFormatSpec::DockerArchive) =>
            {
                diagnostics.push(error(
                    "assembly_oci_compression_unsupported",
                    format!(
                        "{label} uses zstd layers, which docker-archive does not load; use gzip or none"
                    ),
                    location(),
                ));
            }
            Some(_) => {}
        }

        if let Some(reference) = &oci.reference
            && (reference.trim().is_empty() || reference.contains(char::is_whitespace))
        {
            diagnostics.push(error(
                "assembly_oci_ref_invalid",
                format!("{label} ref '{reference}' must be a non-empty name without spaces"),
                location(),
            ));
        }

        if oci.layers.is_empty() {
            diagnostics.push(error(
                "assembly_oci_layers_empty",
                format!("{label} has no layers"),
                location(),
            ));
        }
        for (index, layer) in oci.layers.iter().enumerate() {
            let number = index + 1;
            match layer.source() {
                None => diagnostics.push(error(
                    "assembly_oci_layer_source_invalid",
                    format!("{label} layer {number} must set exactly one of tar or tree"),
                    Some("image.assembly.oci.layers".into()),
                )),
                Some(AssemblyOciLayerSource::Tar(tar)) => validate_assembly_path_template(
                    spec,
                    tree_ids,
                    tar,
                    "image.assembly.oci.layers.tar",
                    diagnostics,
                ),
                Some(AssemblyOciLayerSource::Tree(tree)) => {
                    if !tree_ids.contains(tree) {
                        diagnostics.push(error(
                            "assembly_oci_layer_tree_unknown",
                            format!(
                                "{label} layer {number} names tree '{tree}', which is not defined"
                            ),
                            Some("image.assembly.oci.layers.tree".into()),
                        ));
                    }
                }
            }
        }

        validate_config(oci, &label, diagnostics);
    }
}

fn validate_config(
    oci: &gaia_spec::AssemblyOciSpec,
    label: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.oci.config".to_string());
    let config = &oci.config;
    if config.architecture.is_empty() {
        diagnostics.push(error(
            "assembly_oci_architecture_missing",
            format!("{label} config.architecture is required (amd64 or arm64)"),
            location(),
        ));
    } else if !OCI_ARCHITECTURES.contains(&config.architecture.as_str()) {
        diagnostics.push(error(
            "assembly_oci_architecture_invalid",
            format!(
                "{label} config.architecture '{}' is not supported; use amd64 or arm64",
                config.architecture
            ),
            location(),
        ));
    }
    if config.os != "linux" {
        diagnostics.push(error(
            "assembly_oci_os_invalid",
            format!(
                "{label} config.os '{}' is not supported; use linux",
                config.os
            ),
            location(),
        ));
    }
    for entry in &config.env {
        match entry.split_once('=') {
            Some((key, _)) if !key.is_empty() => {}
            _ => diagnostics.push(error(
                "assembly_oci_env_invalid",
                format!("{label} config.env entry '{entry}' must be KEY=value"),
                location(),
            )),
        }
    }
    if let Some(working_dir) = &config.working_dir
        && !working_dir.starts_with('/')
    {
        diagnostics.push(error(
            "assembly_oci_working_dir_invalid",
            format!("{label} config.working_dir '{working_dir}' must be an absolute path"),
            location(),
        ));
    }
    for (key, _) in &config.labels {
        if key.trim().is_empty() {
            diagnostics.push(error(
                "assembly_oci_label_key_empty",
                format!("{label} config.labels has an empty key"),
                location(),
            ));
        }
    }
}
