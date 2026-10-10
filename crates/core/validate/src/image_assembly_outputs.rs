//! Validation of the assembly's filesystem images (zstd levels, published
//! copies) and of kernel module closures.

use std::collections::HashSet;
use std::path::PathBuf;

use gaia_spec::{
    AssemblyFilesystemKindSpec, AssemblyRoots, AssemblyTreeId, ImageAssemblySpec, ResolvedBuildSpec,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// Which output a path is: a filesystem, disk or archive, by index in its
/// list, or a filesystem's published copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Filesystem(usize),
    Disk(usize),
    Archive(usize),
    Published(usize),
}

pub(crate) fn validate_filesystem_outputs(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for filesystem in &assembly.filesystems {
        if let Some(level) = filesystem.compression_level {
            if filesystem.kind != AssemblyFilesystemKindSpec::CpioZstd {
                diagnostics.push(error(
                    "assembly_filesystem_compression_level_unsupported",
                    format!(
                        "assembly filesystem '{}' kind '{}' does not accept compression_level",
                        filesystem.id,
                        filesystem.kind.as_str()
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            } else if !(1..=19).contains(&level) {
                diagnostics.push(error(
                    "assembly_filesystem_compression_level_invalid",
                    format!(
                        "assembly filesystem '{}' compression_level {level} must be between 1 and 19",
                        filesystem.id
                    ),
                    Some("image.assembly.filesystems".into()),
                ));
            }
        }
        if filesystem.publish && filesystem.publish_template().is_none() {
            diagnostics.push(error(
                "assembly_filesystem_publish_name_missing",
                format!(
                    "assembly filesystem '{}' is published, but its output '{}' has no file name to publish it under",
                    filesystem.id, filesystem.output
                ),
                Some("image.assembly.filesystems".into()),
            ));
        }
    }

    // A published copy goes to the image output dir under the output's file
    // name: no other output, and no other copy, may land on the same file.
    let Ok(roots) = AssemblyRoots::new(spec, assembly) else {
        return;
    };
    let resolve = |template: &str| roots.resolve_path(spec, template).ok();
    let mut outputs: Vec<(Owner, PathBuf)> = Vec::new();
    for (index, filesystem) in assembly.filesystems.iter().enumerate() {
        outputs.extend(
            resolve(filesystem.output.as_str()).map(|path| (Owner::Filesystem(index), path)),
        );
    }
    for (index, disk) in assembly.disks.iter().enumerate() {
        outputs.extend(resolve(disk.output.as_str()).map(|path| (Owner::Disk(index), path)));
    }
    for (index, archive) in assembly.archives.iter().enumerate() {
        outputs.extend(resolve(archive.output.as_str()).map(|path| (Owner::Archive(index), path)));
    }
    let mut published: Vec<(Owner, PathBuf)> = Vec::new();
    for (index, filesystem) in assembly.filesystems.iter().enumerate() {
        let Some(template) = filesystem.publish_template() else {
            continue;
        };
        if let Some(path) = resolve(template.as_str()) {
            published.push((Owner::Published(index), path));
        }
    }
    for (owner, target) in &published {
        let Owner::Published(index) = owner else {
            continue;
        };
        let filesystem = &assembly.filesystems[*index];
        let collides = outputs
            .iter()
            .filter(|(other, _)| *other != Owner::Filesystem(*index))
            .find(|(_, path)| path == target)
            .map(|(other, _)| describe_owner(assembly, *other))
            .or_else(|| {
                published
                    .iter()
                    .filter(|(other, _)| other != owner)
                    .find(|(_, path)| path == target)
                    .map(|(other, _)| describe_owner(assembly, *other))
            });
        if let Some(other) = collides {
            diagnostics.push(error(
                "assembly_filesystem_publish_collision",
                format!(
                    "assembly filesystem '{}' is published to '{}', which {other} also writes; give one of them another file name",
                    filesystem.id,
                    target.display()
                ),
                Some("image.assembly.filesystems".into()),
            ));
        }
    }
}

fn describe_owner(assembly: &ImageAssemblySpec, owner: Owner) -> String {
    match owner {
        Owner::Filesystem(index) => format!("filesystem '{}'", assembly.filesystems[index].id),
        Owner::Disk(index) => format!("disk '{}'", assembly.disks[index].id),
        Owner::Archive(index) => format!("archive '{}'", assembly.archives[index].id),
        Owner::Published(index) => {
            format!(
                "the published copy of filesystem '{}'",
                assembly.filesystems[index].id
            )
        }
    }
}

pub(crate) fn validate_kernel_modules(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for (index, modules) in assembly.kernel_modules.iter().enumerate() {
        let location = Some("image.assembly.kernel_modules".to_string());
        if !tree_ids.contains(&modules.tree) {
            diagnostics.push(error(
                "assembly_kernel_modules_tree_unknown",
                format!(
                    "kernel modules entry {} references unknown tree '{}'",
                    index + 1,
                    modules.tree
                ),
                location.clone(),
            ));
        }
        if modules.from.trim().is_empty() {
            diagnostics.push(error(
                "assembly_kernel_modules_from_empty",
                format!(
                    "kernel modules entry {} needs a 'from' directory of kernel version directories",
                    index + 1
                ),
                location.clone(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &modules.from,
                "image.assembly.kernel_modules.from",
                diagnostics,
            );
        }
        if let Some(depmod) = &modules.depmod {
            if depmod.trim().is_empty() {
                diagnostics.push(error(
                    "assembly_kernel_modules_depmod_empty",
                    format!(
                        "kernel modules entry {} depmod path cannot be empty",
                        index + 1
                    ),
                    location.clone(),
                ));
            } else {
                validate_assembly_path_template(
                    spec,
                    tree_ids,
                    depmod,
                    "image.assembly.kernel_modules.depmod",
                    diagnostics,
                );
            }
        }
        if let Some(version) = &modules.kernel_version
            && (version.trim().is_empty() || version.contains('/'))
        {
            diagnostics.push(error(
                "assembly_kernel_modules_version_invalid",
                format!(
                    "kernel modules entry {} kernel_version '{version}' must be one directory name",
                    index + 1
                ),
                location.clone(),
            ));
        }
        if modules.modules.is_empty() {
            diagnostics.push(error(
                "assembly_kernel_modules_modules_empty",
                format!("kernel modules entry {} lists no modules", index + 1),
                location.clone(),
            ));
        }
        for name in &modules.modules {
            if name.trim().is_empty() || name.contains('/') {
                diagnostics.push(error(
                    "assembly_kernel_modules_name_invalid",
                    format!(
                        "kernel modules entry {} module name '{name}' must be a bare module name",
                        index + 1
                    ),
                    location.clone(),
                ));
            }
        }
    }
}
