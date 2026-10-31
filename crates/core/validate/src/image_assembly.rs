use std::collections::{HashMap, HashSet};
use std::path::Path;

use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly_collected::validate_collected_reference;
use crate::image_assembly_generated;
use crate::image_assembly_layout;
use crate::image_assembly_strip;

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

    if let Some(tool) = &assembly.strip_tool {
        image_assembly_strip::validate_strip_tool(spec, &tree_ids, tool, diagnostics);
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

    for (index, file) in assembly.files.iter().enumerate() {
        if !tree_ids.contains(&file.tree) {
            diagnostics.push(error(
                "assembly_file_tree_unknown",
                format!("assembly file references unknown tree '{}'", file.tree),
                Some("image.assembly.files".into()),
            ));
        }
        validate_exclude(
            &file.exclude,
            file.src_glob.is_some() || file.src_dir.is_some(),
            &format!("files[{index}].exclude"),
            "image.assembly.files",
            diagnostics,
        );
        image_assembly_strip::validate_include(file, index, diagnostics);
        image_assembly_strip::validate_strip_file(file, index, diagnostics);
        if file.allow_missing_libs && (file.content.is_some() || file.json.is_some()) {
            diagnostics.push(error(
                "assembly_allow_missing_libs_not_allowed",
                format!(
                    "files[{index}].allow_missing_libs only applies to a copied file (src, src_glob or src_dir), not generated content"
                ),
                Some("image.assembly.files".into()),
            ));
        }
        if file.content.is_some() || file.json.is_some() {
            image_assembly_generated::validate_generated_file(spec, &tree_ids, file, diagnostics);
            continue;
        }
        let source_count = [
            file.src.is_some(),
            file.src_glob.is_some(),
            file.src_dir.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        if source_count != 1 {
            diagnostics.push(error(
                "assembly_file_source_invalid",
                "assembly file entries must set exactly one of src, src_glob, src_dir, content or json"
                    .into(),
                Some("image.assembly.files".into()),
            ));
        } else if let Some(src) = &file.src {
            validate_assembly_path_template(
                spec,
                &tree_ids,
                src,
                "image.assembly.files.src",
                diagnostics,
            );
            validate_collected_reference(
                spec,
                src,
                &format!("files[{index}].src"),
                "image.assembly.files",
                diagnostics,
            );
        } else if let Some(src_glob) = &file.src_glob {
            validate_assembly_path_template(
                spec,
                &tree_ids,
                src_glob,
                "image.assembly.files.src_glob",
                diagnostics,
            );
            validate_collected_reference(
                spec,
                src_glob,
                &format!("files[{index}].src_glob"),
                "image.assembly.files",
                diagnostics,
            );
            validate_simple_glob_pattern(src_glob, diagnostics);
        } else if let Some(src_dir) = &file.src_dir {
            validate_assembly_path_template(
                spec,
                &tree_ids,
                src_dir,
                "image.assembly.files.src_dir",
                diagnostics,
            );
            validate_collected_reference(
                spec,
                src_dir,
                &format!("files[{index}].src_dir"),
                "image.assembly.files",
                diagnostics,
            );
            if file.preserve_symlink {
                diagnostics.push(error(
                    "assembly_preserve_symlink_not_allowed",
                    format!(
                        "files[{index}].preserve_symlink does not apply to src_dir: its symbolic links are always kept as links"
                    ),
                    Some("image.assembly.files".into()),
                ));
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
        if file.dir_mode.is_some() && file.src_dir.is_none() {
            diagnostics.push(error(
                "assembly_file_dir_mode_not_allowed",
                format!("files[{index}].dir_mode only applies to src_dir entries"),
                Some("image.assembly.files".into()),
            ));
        } else if let Err(parse_error) = file.parsed_dir_mode() {
            diagnostics.push(error(
                "assembly_file_mode_invalid",
                format!("invalid assembly file dir_mode: {parse_error}"),
                Some("image.assembly.files".into()),
            ));
        }
    }

    for (index, transform) in assembly.transforms.iter().enumerate() {
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
            validate_collected_reference(
                spec,
                src,
                &format!("transforms[{index}].src"),
                "image.assembly.transforms",
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
        if let Some(free) = &filesystem.free {
            if filesystem.size.as_deref() != Some("auto") {
                diagnostics.push(error(
                    "assembly_filesystem_free_without_auto",
                    format!(
                        "assembly filesystem '{}' sets free, which only applies with size = \"auto\"",
                        filesystem.id
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            }
            if filesystem.parsed_free().is_err() {
                diagnostics.push(error(
                    "assembly_filesystem_free_invalid",
                    format!(
                        "assembly filesystem '{}' free '{free}' is not a byte size",
                        filesystem.id
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            }
        }
        if filesystem.size.as_deref() == Some("auto") {
            if !matches!(
                filesystem.kind,
                gaia_spec::AssemblyFilesystemKindSpec::Vfat
                    | gaia_spec::AssemblyFilesystemKindSpec::Ext4
            ) {
                diagnostics.push(error(
                    "assembly_filesystem_size_auto_unsupported",
                    format!(
                        "assembly filesystem '{}' uses size = \"auto\", which is only supported for kinds 'vfat' and 'ext4'",
                        filesystem.id
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            }
        } else if let Err(parse_error) = filesystem.parsed_size() {
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
            | gaia_spec::AssemblyFilesystemKindSpec::CpioZstd
            | gaia_spec::AssemblyFilesystemKindSpec::CpioXz
            | gaia_spec::AssemblyFilesystemKindSpec::Ext4
            | gaia_spec::AssemblyFilesystemKindSpec::Squashfs
            | gaia_spec::AssemblyFilesystemKindSpec::Erofs
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

    // A partition may hold any image the assembly writes: a filesystem, a
    // transform's output (a U-Boot environment, a verity hash tree) or a UBI
    // image.
    let generated_images: HashSet<&gaia_spec::AssemblyPathTemplate> = filesystem_outputs
        .iter()
        .chain(assembly.transforms.iter().map(|transform| &transform.dest))
        .chain(assembly.ubis.iter().map(|ubi| &ubi.output))
        .collect();
    for disk in &assembly.disks {
        if disk.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_disk_id_empty",
                "assembly disk id cannot be empty".into(),
                Some("image.assembly.disks".into()),
            ));
        }
        crate::image_assembly_gpt::validate_disk_partition_table(disk, diagnostics);
        if disk.truncate.is_some()
            && disk.partition_table != gaia_spec::AssemblyPartitionTableSpec::Mbr
        {
            diagnostics.push(error(
                "assembly_disk_truncate_partition_table_unsupported",
                format!(
                    "assembly disk '{}' sets truncate with partition table '{}'; truncate is only supported for mbr, because a gpt backup header lives at the end of the full-size disk",
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
            if disk.partition_table == gaia_spec::AssemblyPartitionTableSpec::Mbr
                && (partition.kind.is_some() || partition.type_alias.is_some())
            {
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
            let Some(image) = &partition.image else {
                continue;
            };
            if image.trim().is_empty() {
                diagnostics.push(error(
                    "assembly_partition_image_empty",
                    format!(
                        "assembly disk '{}' partition '{}' image cannot be empty",
                        disk.id, partition.name
                    ),
                    Some("image.assembly.disks.partitions".into()),
                ));
            } else if !generated_images.contains(image)
                && !uses_supported_provider_variable(spec, image)
                && !image.starts_with("@assets/")
            {
                diagnostics.push(error(
                    "assembly_partition_image_unknown",
                    format!(
                        "assembly disk '{}' partition '{}' image '{}' does not reference a generated filesystem, transform or UBI output or a supported provider root",
                        disk.id, partition.name, image
                    ),
                    Some("image.assembly.disks.partitions".into()),
                ));
            }
            validate_assembly_path_template(
                spec,
                &tree_ids,
                image,
                "image.assembly.disks.partitions.image",
                diagnostics,
            );
            validate_collected_reference(
                spec,
                image,
                &format!("disks[{}].partitions[{}].image", disk.id, partition.name),
                "image.assembly.disks.partitions",
                diagnostics,
            );
        }
        image_assembly_layout::validate_disk_layout(disk, diagnostics);
    }

    crate::image_assembly_devices::validate_devices(assembly, &tree_ids, diagnostics);
    image_assembly_layout::validate_transform_levels(assembly, diagnostics);
    crate::image_assembly_options::validate_transform_options(
        spec,
        assembly,
        &tree_ids,
        diagnostics,
    );
    crate::image_assembly_options::validate_filesystem_options(assembly, diagnostics);
    crate::image_assembly_compressed::validate_compressed_filesystems(assembly, diagnostics);
    crate::image_assembly_options::validate_verity_token_targets(assembly, diagnostics);
    crate::image_assembly_outputs::validate_filesystem_outputs(spec, assembly, diagnostics);
    crate::image_assembly_outputs::validate_kernel_modules(spec, assembly, &tree_ids, diagnostics);
    image_assembly_layout::validate_archives(spec, assembly, &tree_ids, diagnostics);
    crate::image_assembly_oci::validate_ocis(spec, assembly, &tree_ids, diagnostics);
    crate::image_assembly_package::validate_packages(spec, assembly, &tree_ids, diagnostics);
    crate::image_assembly_ubi::validate_ubis(spec, assembly, &tree_ids, diagnostics);
    crate::image_assembly_checks::validate_checks(assembly, &tree_ids, diagnostics);
    image_assembly_generated::validate_binaries(spec, &tree_ids, assembly, diagnostics);
    image_assembly_generated::validate_token_reads(assembly, diagnostics);
    crate::image_assembly_order::validate_assembly_order(assembly, diagnostics);

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
        if let Some(sysroot) = &initramfs.sysroot {
            if sysroot.as_str().trim().is_empty() {
                diagnostics.push(error(
                    "assembly_busybox_sysroot_empty",
                    "busybox initramfs sysroot cannot be empty".into(),
                    Some("image.assembly.busybox_initramfs".into()),
                ));
            }
            validate_assembly_path_template(
                spec,
                &tree_ids,
                sysroot,
                "image.assembly.busybox_initramfs.sysroot",
                diagnostics,
            );
        }
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
        .chain(assembly.ubis.iter().map(|ubi| ubi.output.as_str()))
        .chain(assembly.disks.iter().map(|disk| disk.output.as_str()))
        .chain(
            assembly
                .archives
                .iter()
                .map(|archive| archive.output.as_str()),
        )
        .filter_map(expected_image_name_from_output)
        .collect()
}

/// `exclude` lists file names; it is only allowed where the source is a
/// glob or tree (`allowed`), and each entry must be a bare name.
pub(crate) fn validate_exclude(
    exclude: &[String],
    allowed: bool,
    label: &str,
    location: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if !exclude.is_empty() && !allowed {
        diagnostics.push(error(
            "assembly_exclude_not_allowed",
            format!(
                "{label} sets exclude, which only applies to src_glob or src_dir (or tree for archive members)"
            ),
            Some(location.into()),
        ));
    }
    for name in exclude {
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            diagnostics.push(error(
                "assembly_exclude_invalid",
                format!("{label} entry '{name}' must be a file name without '/'"),
                Some(location.into()),
            ));
        }
    }
}

fn validate_simple_glob_pattern(pattern: &str, diagnostics: &mut Vec<ValidationDiagnostic>) {
    let _ = pattern;
    let _ = diagnostics;
}

pub(crate) fn validate_assembly_path_template(
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

pub(crate) fn validate_relative_dest(
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

pub(crate) fn uses_supported_provider_variable(spec: &ResolvedBuildSpec, value: &str) -> bool {
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
        gaia_spec::ImageProviderKind::None => false,
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
