use crate::reuse::{command_signature, path_state_signature};
use gaia_spec::ResolvedBuildSpec;
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(crate) fn assembly_input_signature(spec: &ResolvedBuildSpec) -> String {
    let Some(assembly) = &spec.image.assembly else {
        return "assembly:none".into();
    };
    let Ok(roots) = gaia_spec::AssemblyRoots::new(spec, assembly) else {
        return "assembly:root_resolution_failed".into();
    };
    let mut parts = Vec::new();
    let generated_filesystem_outputs = assembly
        .filesystems
        .iter()
        .filter_map(|filesystem| roots.resolve_path(spec, &filesystem.output).ok())
        .collect::<BTreeSet<_>>();
    let mut glob_match_count = 0usize;
    let mut generated_partition_images = 0usize;
    let mut direct_partition_images = 0usize;
    let mut partition_resolution_errors = 0usize;
    for dir in &assembly.dirs {
        parts.push(format!(
            "dir:{}:{}:{}",
            dir.tree,
            dir.path,
            dir.mode.as_deref().unwrap_or("")
        ));
    }
    for symlink in &assembly.symlinks {
        parts.push(format!(
            "symlink:{}:{}:{}",
            symlink.tree, symlink.path, symlink.target
        ));
    }
    for file in &assembly.files {
        if let Some(src) = &file.src {
            let resolved = roots
                .resolve_path(spec, src)
                .unwrap_or_else(|_| PathBuf::from(src.as_str()));
            parts.push(format!(
                "src:{}:{}",
                resolved.display(),
                path_state_signature(&resolved)
            ));
        }
        if let Some(src_glob) = &file.src_glob {
            let matches = gaia_spec::expand_simple_glob(spec, &roots, src_glob).unwrap_or_default();
            glob_match_count += matches.len();
            parts.push(format!("glob:{src_glob}:count={}", matches.len()));
            for matched in matches {
                parts.push(format!(
                    "glob-match:{}:{}",
                    matched.display(),
                    path_state_signature(&matched)
                ));
            }
        }
    }
    for transform in &assembly.transforms {
        if let Some(src) = &transform.src {
            let resolved = roots
                .resolve_path(spec, src)
                .unwrap_or_else(|_| PathBuf::from(src.as_str()));
            parts.push(format!(
                "transform:{}:{}:{}",
                transform.kind.as_str(),
                resolved.display(),
                path_state_signature(&resolved)
            ));
        }
        match transform.kind {
            gaia_spec::AssemblyTransformKindSpec::Gzip => {
                parts.push(command_signature("gzip", ["--version"]));
            }
            gaia_spec::AssemblyTransformKindSpec::Zstd => {
                parts.push(command_signature("zstd", ["--version"]));
            }
            _ => {}
        }
    }
    for initramfs in &assembly.busybox_initramfs {
        let resolved = roots
            .resolve_path(spec, &initramfs.busybox)
            .unwrap_or_else(|_| PathBuf::from(initramfs.busybox.as_str()));
        parts.push(format!(
            "busybox:{}:{}:{}:{}",
            initramfs.tree,
            resolved.display(),
            path_state_signature(&resolved),
            initramfs.applets.join(",")
        ));
        if initramfs.include_runtime_libs {
            parts.push(command_signature("ldd", ["--version"]));
        }
    }
    for disk in &assembly.disks {
        for partition in &disk.partitions {
            let Some(image) = &partition.image else {
                // An empty partition has no input file; its size and wipe
                // flag are covered by the hashed assembly config.
                parts.push(format!(
                    "partition-empty:{}:{}:{}:{}",
                    disk.id,
                    partition.name,
                    partition.size.as_deref().unwrap_or(""),
                    partition.wipe
                ));
                continue;
            };
            match roots.resolve_path(spec, image) {
                Ok(resolved) if generated_filesystem_outputs.contains(&resolved) => {
                    generated_partition_images += 1;
                    parts.push(format!(
                        "partition-image-generated:{}:{}:{}",
                        disk.id,
                        partition.name,
                        resolved.display()
                    ));
                }
                Ok(resolved) => {
                    direct_partition_images += 1;
                    parts.push(format!(
                        "partition-image:{}:{}:{}:{}",
                        disk.id,
                        partition.name,
                        resolved.display(),
                        path_state_signature(&resolved)
                    ));
                }
                Err(error) => {
                    partition_resolution_errors += 1;
                    parts.push(format!(
                        "partition-image-resolution-error:{}:{}:{}:{}",
                        disk.id,
                        partition.name,
                        image.as_str(),
                        error
                    ));
                }
            }
        }
    }
    let generated_outputs = generated_filesystem_outputs
        .into_iter()
        .chain(
            assembly
                .transforms
                .iter()
                .filter_map(|transform| roots.resolve_path(spec, &transform.dest).ok()),
        )
        .chain(
            assembly
                .disks
                .iter()
                .filter_map(|disk| roots.resolve_path(spec, &disk.output).ok()),
        )
        .collect::<BTreeSet<_>>();
    let archive_inputs = crate::reuse_assembly_archives::archive_input_parts(
        spec,
        &roots,
        assembly,
        &generated_outputs,
    );
    let archive_input_count = archive_inputs.len();
    parts.extend(archive_inputs);
    tracing::debug!(
        archives = assembly.archives.len(),
        archive_inputs = archive_input_count,
        file_entries = assembly.files.len(),
        transforms = assembly.transforms.len(),
        filesystems = assembly.filesystems.len(),
        disks = assembly.disks.len(),
        busybox_initramfs = assembly.busybox_initramfs.len(),
        glob_matches = glob_match_count,
        generated_partition_images,
        direct_partition_images,
        partition_resolution_errors,
        fingerprint_parts = parts.len(),
        "computed image assembly reuse fingerprint inputs"
    );
    parts.join("|")
}
