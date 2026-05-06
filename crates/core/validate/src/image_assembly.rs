use std::collections::{HashMap, HashSet};
use std::path::Path;

use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_image_assembly(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Some(assembly) = &spec.image.assembly else {
        return;
    };

    let mut tree_ids = HashSet::new();
    let mut tree_paths = HashMap::new();
    for tree in &assembly.trees {
        if !tree.id.is_valid() {
            diagnostics.push(error(
                "assembly_tree_id_empty",
                "assembly tree id cannot be empty".into(),
                Some("image.assembly.trees".into()),
            ));
        } else if !tree_ids.insert(tree.id.clone()) {
            diagnostics.push(error(
                "assembly_tree_duplicate",
                format!("assembly tree '{}' is declared more than once", tree.id),
                Some("image.assembly.trees".into()),
            ));
        }
        if tree.path.trim().is_empty() {
            diagnostics.push(error(
                "assembly_tree_path_empty",
                format!("assembly tree '{}' path cannot be empty", tree.id),
                Some("image.assembly.trees".into()),
            ));
        }
        tree_paths.insert(tree.id.as_str(), &tree.path);
        validate_assembly_path_template(
            spec,
            &tree_ids,
            &tree.path,
            "image.assembly.trees",
            diagnostics,
        );
    }

    for dir in &assembly.dirs {
        if !tree_ids.contains(&dir.tree) {
            diagnostics.push(error(
                "assembly_dir_tree_unknown",
                format!("assembly dir references unknown tree '{}'", dir.tree),
                Some("image.assembly.dirs".into()),
            ));
        }
        validate_relative_dest(&dir.path, "assembly_dir_path_invalid", diagnostics);
        if let Err(parse_error) = dir.parsed_mode() {
            diagnostics.push(error(
                "assembly_dir_mode_invalid",
                format!("invalid assembly dir mode: {parse_error}"),
                Some("image.assembly.dirs".into()),
            ));
        }
    }

    for symlink in &assembly.symlinks {
        if !tree_ids.contains(&symlink.tree) {
            diagnostics.push(error(
                "assembly_symlink_tree_unknown",
                format!(
                    "assembly symlink references unknown tree '{}'",
                    symlink.tree
                ),
                Some("image.assembly.symlinks".into()),
            ));
        }
        validate_relative_dest(&symlink.path, "assembly_symlink_path_invalid", diagnostics);
        if symlink.target.trim().is_empty() {
            diagnostics.push(error(
                "assembly_symlink_target_empty",
                format!("assembly symlink '{}' target cannot be empty", symlink.path),
                Some("image.assembly.symlinks".into()),
            ));
        }
    }

    for file in &assembly.files {
        if !tree_ids.contains(&file.tree) {
            diagnostics.push(error(
                "assembly_file_tree_unknown",
                format!("assembly file references unknown tree '{}'", file.tree),
                Some("image.assembly.files".into()),
            ));
        }
        match (file.src.as_ref(), file.src_glob.as_ref()) {
            (Some(_), Some(_)) | (None, None) => diagnostics.push(error(
                "assembly_file_source_invalid",
                "assembly file entries must set exactly one of src or src_glob".into(),
                Some("image.assembly.files".into()),
            )),
            (Some(src), None) => validate_assembly_path_template(
                spec,
                &tree_ids,
                src,
                "image.assembly.files.src",
                diagnostics,
            ),
            (None, Some(src_glob)) => {
                validate_assembly_path_template(
                    spec,
                    &tree_ids,
                    src_glob,
                    "image.assembly.files.src_glob",
                    diagnostics,
                );
                validate_simple_glob_pattern(src_glob, diagnostics);
            }
        }
        validate_relative_dest(&file.dest, "assembly_file_dest_invalid", diagnostics);
        if let Err(parse_error) = file.parsed_mode() {
            diagnostics.push(error(
                "assembly_file_mode_invalid",
                format!("invalid assembly file mode: {parse_error}"),
                Some("image.assembly.files".into()),
            ));
        }
    }

    for transform in &assembly.transforms {
        if transform.dest.trim().is_empty() {
            diagnostics.push(error(
                "assembly_transform_dest_empty",
                "assembly transform dest cannot be empty".into(),
                Some("image.assembly.transforms".into()),
            ));
        }
        if transform.src.as_deref().is_none_or(str::is_empty) {
            diagnostics.push(error(
                "assembly_transform_src_required",
                format!(
                    "assembly transform kind '{}' requires src",
                    transform.kind.as_str()
                ),
                Some("image.assembly.transforms".into()),
            ));
        }
        if let Some(src) = &transform.src {
            validate_assembly_path_template(
                spec,
                &tree_ids,
                src,
                "image.assembly.transforms.src",
                diagnostics,
            );
        }
        validate_assembly_path_template(
            spec,
            &tree_ids,
            &transform.dest,
            "image.assembly.transforms.dest",
            diagnostics,
        );
        if !transform.deterministic {
            diagnostics.push(error(
                "assembly_transform_deterministic_unsupported",
                format!(
                    "assembly transform kind '{}' always produces deterministic output; deterministic=false is not supported",
                    transform.kind.as_str()
                ),
                Some("image.assembly.transforms".into()),
            ));
        }
    }

    let mut filesystem_outputs = HashSet::new();
    for filesystem in &assembly.filesystems {
        if !filesystem.id.is_valid() {
            diagnostics.push(error(
                "assembly_filesystem_id_empty",
                "assembly filesystem id cannot be empty".into(),
                Some("image.assembly.filesystems".into()),
            ));
        }
        if !tree_ids.contains(&filesystem.source_tree) {
            diagnostics.push(error(
                "assembly_filesystem_tree_unknown",
                format!(
                    "assembly filesystem '{}' references unknown tree '{}'",
                    filesystem.id, filesystem.source_tree
                ),
                Some("image.assembly.filesystems".into()),
            ));
        }
        if filesystem.output.trim().is_empty() {
            diagnostics.push(error(
                "assembly_filesystem_output_empty",
                format!(
                    "assembly filesystem '{}' output cannot be empty",
                    filesystem.id
                ),
                Some("image.assembly.filesystems".into()),
            ));
        } else {
            filesystem_outputs.insert(filesystem.output.clone());
            validate_assembly_path_template(
                spec,
                &tree_ids,
                &filesystem.output,
                "image.assembly.filesystems.output",
                diagnostics,
            );
        }
        if let Err(parse_error) = filesystem.parsed_size() {
            diagnostics.push(error(
                "assembly_filesystem_size_invalid",
                format!(
                    "assembly filesystem '{}' has invalid size: {parse_error}",
                    filesystem.id
                ),
                Some("image.assembly.filesystems".into()),
            ));
        }
        match filesystem.kind {
            gaia_spec::AssemblyFilesystemKindSpec::Cpio
            | gaia_spec::AssemblyFilesystemKindSpec::CpioGzip
                if !filesystem.deterministic =>
            {
                diagnostics.push(error(
                    "assembly_filesystem_deterministic_unsupported",
                    format!(
                        "assembly filesystem kind '{}' always produces deterministic output; deterministic=false is not supported",
                        filesystem.kind.as_str()
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            }
            gaia_spec::AssemblyFilesystemKindSpec::Vfat if filesystem.deterministic => {
                diagnostics.push(error(
                    "assembly_filesystem_deterministic_unsupported",
                    "assembly filesystem kind 'vfat' cannot currently guarantee deterministic output; deterministic=true is not supported".into(),
                    Some("image.assembly.filesystems".into()),
                ));
            }
            _ => {}
        }
    }

    for disk in &assembly.disks {
        if disk.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_disk_id_empty",
                "assembly disk id cannot be empty".into(),
                Some("image.assembly.disks".into()),
            ));
        }
        if disk.partition_table != gaia_spec::AssemblyPartitionTableSpec::Mbr {
            diagnostics.push(error(
                "assembly_disk_partition_table_unsupported",
                format!(
                    "assembly disk '{}' uses unsupported partition table '{}'",
                    disk.id,
                    disk.partition_table.as_str()
                ),
                Some("image.assembly.disks".into()),
            ));
        }
        if disk.signature.is_some() && disk.signature_text.is_some() {
            diagnostics.push(error(
                "assembly_disk_signature_invalid",
                format!(
                    "assembly disk '{}' cannot set both signature and signature_text",
                    disk.id
                ),
                Some("image.assembly.disks".into()),
            ));
        }
        if let Some(signature) = &disk.signature
            && !is_hex_u32(signature)
        {
            diagnostics.push(error(
                "assembly_disk_signature_raw_invalid",
                format!(
                    "assembly disk '{}' signature '{}' must be a raw u32 hex value",
                    disk.id, signature
                ),
                Some("image.assembly.disks".into()),
            ));
        }
        validate_assembly_path_template(
            spec,
            &tree_ids,
            &disk.output,
            "image.assembly.disks.output",
            diagnostics,
        );
        for partition in &disk.partitions {
            if partition.kind.is_some() && partition.type_alias.is_some() {
                diagnostics.push(error(
                    "assembly_partition_type_invalid",
                    format!(
                        "assembly disk '{}' partition '{}' cannot set both type and type_alias",
                        disk.id, partition.name
                    ),
                    Some("image.assembly.disks.partitions".into()),
                ));
            }
            if partition.kind.is_some() || partition.type_alias.is_some() {
                match partition.partition_type() {
                    Ok(_) => {}
                    Err(gaia_spec::MbrPartitionTypeParseError::InvalidRaw(raw)) => {
                        diagnostics.push(error(
                            "assembly_partition_type_raw_invalid",
                            format!(
                                "assembly disk '{}' partition '{}' type '{}' must be a raw u8 hex value",
                                disk.id, partition.name, raw
                            ),
                            Some("image.assembly.disks.partitions".into()),
                        ));
                    }
                    Err(gaia_spec::MbrPartitionTypeParseError::UnknownAlias(alias)) => {
                        diagnostics.push(error(
                            "assembly_partition_type_alias_invalid",
                            format!(
                                "assembly disk '{}' partition '{}' uses unknown type_alias '{}'",
                                disk.id, partition.name, alias
                            ),
                            Some("image.assembly.disks.partitions".into()),
                        ));
                    }
                }
            }
            if partition.image.trim().is_empty() {
                diagnostics.push(error(
                    "assembly_partition_image_empty",
                    format!(
                        "assembly disk '{}' partition '{}' image cannot be empty",
                        disk.id, partition.name
                    ),
                    Some("image.assembly.disks.partitions".into()),
                ));
            } else if !filesystem_outputs.contains(&partition.image)
                && !uses_supported_provider_variable(spec, &partition.image)
                && !partition.image.starts_with("@assets/")
            {
                diagnostics.push(error(
                    "assembly_partition_image_unknown",
                    format!(
                        "assembly disk '{}' partition '{}' image '{}' does not reference a generated filesystem output or supported provider root",
                        disk.id, partition.name, partition.image
                    ),
                    Some("image.assembly.disks.partitions".into()),
                ));
            }
            validate_assembly_path_template(
                spec,
                &tree_ids,
                &partition.image,
                "image.assembly.disks.partitions.image",
                diagnostics,
            );
        }
    }

    for initramfs in &assembly.busybox_initramfs {
        if !tree_ids.contains(&initramfs.tree) {
            diagnostics.push(error(
                "assembly_busybox_tree_unknown",
                format!(
                    "busybox initramfs references unknown tree '{}'",
                    initramfs.tree
                ),
                Some("image.assembly.busybox_initramfs".into()),
            ));
        }
        if initramfs.busybox.trim().is_empty() {
            diagnostics.push(error(
                "assembly_busybox_path_empty",
                "busybox initramfs busybox path cannot be empty".into(),
                Some("image.assembly.busybox_initramfs".into()),
            ));
        }
        validate_assembly_path_template(
            spec,
            &tree_ids,
            &initramfs.busybox,
            "image.assembly.busybox_initramfs.busybox",
            diagnostics,
        );
    }

    if let Some(work_dir) = &assembly.work_dir {
        validate_assembly_path_template(
            spec,
            &tree_ids,
            work_dir,
            "image.assembly.work_dir",
            diagnostics,
        );
    }
    if let Some(out_dir) = &assembly.out_dir {
        validate_assembly_path_template(
            spec,
            &tree_ids,
            out_dir,
            "image.assembly.out_dir",
            diagnostics,
        );
    }

    for (tree_id, path) in tree_paths {
        if path.referenced_tree_ids().contains(&tree_id) {
            diagnostics.push(error(
                "assembly_tree_self_reference",
                format!("assembly tree '{tree_id}' path cannot reference itself"),
                Some("image.assembly.trees".into()),
            ));
        }
    }
}

pub(crate) fn assembly_expected_image_names(spec: &ResolvedBuildSpec) -> HashSet<String> {
    let Some(assembly) = &spec.image.assembly else {
        return HashSet::new();
    };
    assembly
        .filesystems
        .iter()
        .map(|filesystem| filesystem.output.as_str())
        .chain(assembly.disks.iter().map(|disk| disk.output.as_str()))
        .filter_map(expected_image_name_from_output)
        .collect()
}

fn validate_simple_glob_pattern(pattern: &str, diagnostics: &mut Vec<ValidationDiagnostic>) {
    let _ = pattern;
    let _ = diagnostics;
}

fn validate_assembly_path_template(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    value: &gaia_spec::AssemblyPathTemplate,
    location: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if let Err(message) =
        value.validate_tokens(spec.image.provider_kind(), |id| tree_ids.contains(id))
    {
        diagnostics.push(error(
            "assembly_path_template_invalid",
            message.to_string(),
            Some(location.into()),
        ));
    }
}

fn expected_image_name_from_output(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn validate_relative_dest(
    dest: &str,
    code: &'static str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let path = Path::new(dest);
    if dest.trim().is_empty() || path.is_absolute() || dest.split('/').any(|part| part == "..") {
        diagnostics.push(error(
            code,
            format!("assembly destination '{dest}' must be relative and stay inside its tree"),
            Some("image.assembly".into()),
        ));
    }
}

fn uses_supported_provider_variable(spec: &ResolvedBuildSpec, value: &str) -> bool {
    provider_variables(value)
        .into_iter()
        .any(|variable| provider_root_supported(spec, variable))
}

fn provider_variables(value: &str) -> Vec<&str> {
    const VARIABLES: &[&str] = &[
        "provider.images",
        "provider.buildroot_output",
        "provider.target",
        "provider.host",
        "provider.staging",
    ];
    VARIABLES
        .iter()
        .copied()
        .filter(|variable| value.contains(&format!("${variable}")))
        .collect()
}

fn provider_root_supported(spec: &ResolvedBuildSpec, variable: &str) -> bool {
    match spec.image.provider_kind() {
        gaia_spec::ImageProviderKind::Buildroot => matches!(
            variable,
            "provider.images"
                | "provider.buildroot_output"
                | "provider.target"
                | "provider.host"
                | "provider.staging"
        ),
        gaia_spec::ImageProviderKind::StartingPoint => {
            matches!(variable, "provider.images" | "provider.target")
        }
    }
}

fn is_hex_u32(value: &str) -> bool {
    parse_prefixed_hex(value).is_some_and(|parsed| parsed <= u32::MAX as u64)
}

fn parse_prefixed_hex(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))?;
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(digits, 16).ok()
}
