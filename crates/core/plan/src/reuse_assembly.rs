use crate::reuse::{command_signature, path_state_signature};
use gaia_spec::ResolvedBuildSpec;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The image assembly's external inputs as `(name, part)` pairs. The name
/// says which input a part is (`src <path>`, `partition <disk>/<name>`, ...),
/// so a changed input can be named; the parts, joined in order, are the
/// fingerprint's input signature (see [`assembly_input_signature`]).
pub(crate) fn assembly_input_entries(spec: &ResolvedBuildSpec) -> Vec<(String, String)> {
    let Some(assembly) = &spec.image.assembly else {
        return vec![("assembly".into(), "assembly:none".into())];
    };
    let Ok(roots) = gaia_spec::AssemblyRoots::new(spec, assembly) else {
        return vec![("assembly".into(), "assembly:root_resolution_failed".into())];
    };
    let mut parts = Vec::new();
    let mut generated_filesystem_outputs = assembly
        .filesystems
        .iter()
        .filter_map(|filesystem| roots.resolve_path(spec, &filesystem.output).ok())
        .collect::<BTreeSet<_>>();
    for filesystem in &assembly.filesystems {
        let Some(template) = filesystem.publish_template() else {
            continue;
        };
        let target = roots
            .resolve_path(spec, template.as_str())
            .unwrap_or_else(|_| PathBuf::from(template.as_str()));
        parts.push((
            format!("published {} {}", filesystem.id, target.display()),
            format!("published:{}:{}", filesystem.id, target.display()),
        ));
        generated_filesystem_outputs.insert(target);
    }
    let mut glob_match_count = 0usize;
    let mut generated_partition_images = 0usize;
    let mut direct_partition_images = 0usize;
    let mut partition_resolution_errors = 0usize;
    for dir in &assembly.dirs {
        parts.push((
            format!("dir {}:{}", dir.tree, dir.path),
            format!(
                "dir:{}:{}:{}",
                dir.tree,
                dir.path,
                dir.mode.as_deref().unwrap_or("")
            ),
        ));
    }
    for symlink in &assembly.symlinks {
        parts.push((
            format!("symlink {}:{}", symlink.tree, symlink.path),
            format!(
                "symlink:{}:{}:{}",
                symlink.tree, symlink.path, symlink.target
            ),
        ));
    }
    for file in &assembly.files {
        if let Some(src) = &file.src {
            let resolved = roots
                .resolve_path(spec, src)
                .unwrap_or_else(|_| PathBuf::from(src.as_str()));
            parts.push((
                format!("src {}", resolved.display()),
                format!(
                    "src:{}:{}",
                    resolved.display(),
                    path_state_signature(&resolved)
                ),
            ));
        }
        if let Some(src_glob) = &file.src_glob {
            let matches = gaia_spec::expand_simple_glob(spec, &roots, src_glob).unwrap_or_default();
            glob_match_count += matches.len();
            parts.push((
                format!("glob {src_glob}"),
                format!("glob:{src_glob}:count={}", matches.len()),
            ));
            for matched in matches {
                parts.push((
                    format!("glob-match {}", matched.display()),
                    format!(
                        "glob-match:{}:{}",
                        matched.display(),
                        path_state_signature(&matched)
                    ),
                ));
            }
        }
    }
    for transform in &assembly.transforms {
        if let Some(src) = &transform.src {
            let resolved = roots
                .resolve_path(spec, src)
                .unwrap_or_else(|_| PathBuf::from(src.as_str()));
            parts.push((
                format!(
                    "transform {} {}",
                    transform.kind.as_str(),
                    resolved.display()
                ),
                format!(
                    "transform:{}:{}:{}",
                    transform.kind.as_str(),
                    resolved.display(),
                    path_state_signature(&resolved)
                ),
            ));
        }
        match transform.kind {
            gaia_spec::AssemblyTransformKindSpec::Gzip => {
                parts.push(("tool gzip".into(), command_signature("gzip", ["--version"])));
            }
            gaia_spec::AssemblyTransformKindSpec::Zstd => {
                parts.push(("tool zstd".into(), command_signature("zstd", ["--version"])));
            }
            _ => {}
        }
    }
    for initramfs in &assembly.busybox_initramfs {
        let resolved = roots
            .resolve_path(spec, &initramfs.busybox)
            .unwrap_or_else(|_| PathBuf::from(initramfs.busybox.as_str()));
        parts.push((
            format!("busybox {} {}", initramfs.tree, resolved.display()),
            format!(
                "busybox:{}:{}:{}:{}",
                initramfs.tree,
                resolved.display(),
                path_state_signature(&resolved),
                initramfs.applets.join(",")
            ),
        ));
        if initramfs.include_runtime_libs {
            parts.push(("tool ldd".into(), command_signature("ldd", ["--version"])));
        }
    }
    for filesystem in &assembly.filesystems {
        if filesystem.kind == gaia_spec::AssemblyFilesystemKindSpec::CpioZstd {
            parts.push((
                format!("tool zstd (cpio-zstd {})", filesystem.id),
                command_signature("zstd", ["--version"]),
            ));
        }
    }
    for modules in &assembly.kernel_modules {
        let from = roots
            .resolve_path(spec, &modules.from)
            .unwrap_or_else(|_| PathBuf::from(modules.from.as_str()));
        // The kernel's own module index: Buildroot rewrites it whenever the
        // modules are installed, so it stands for the module files.
        let kernel_dir = match &modules.kernel_version {
            Some(version) => from.join(version),
            None => single_subdir(&from).unwrap_or_else(|| from.clone()),
        };
        let mut names = modules.modules.clone();
        names.sort();
        parts.push((
            format!("kernel-modules {} {}", modules.tree, kernel_dir.display()),
            format!(
                "kernel-modules:{}:{}:dep={}:builtin={}:modules={}",
                modules.tree,
                kernel_dir.display(),
                path_state_signature(&kernel_dir.join("modules.dep")),
                path_state_signature(&kernel_dir.join("modules.builtin")),
                names.join(",")
            ),
        ));
        match modules.depmod.as_ref() {
            Some(template) => {
                let depmod = roots
                    .resolve_path(spec, template.as_str())
                    .unwrap_or_else(|_| PathBuf::from(template.as_str()));
                parts.push((
                    format!("tool depmod {}", depmod.display()),
                    format!("depmod-file:{}", path_state_signature(&depmod)),
                ));
            }
            None => parts.push((
                "tool depmod".into(),
                command_signature("depmod", ["--version"]),
            )),
        }
    }
    for disk in &assembly.disks {
        for partition in &disk.partitions {
            let key = format!("partition {}/{}", disk.id, partition.name);
            let Some(image) = &partition.image else {
                // An empty partition has no input file; its size and wipe
                // flag are covered by the hashed assembly config.
                parts.push((
                    key,
                    format!(
                        "partition-empty:{}:{}:{}:{}",
                        disk.id,
                        partition.name,
                        partition.size.as_deref().unwrap_or(""),
                        partition.wipe
                    ),
                ));
                continue;
            };
            match roots.resolve_path(spec, image) {
                Ok(resolved) if generated_filesystem_outputs.contains(&resolved) => {
                    generated_partition_images += 1;
                    parts.push((
                        key,
                        format!(
                            "partition-image-generated:{}:{}:{}",
                            disk.id,
                            partition.name,
                            resolved.display()
                        ),
                    ));
                }
                Ok(resolved) => {
                    direct_partition_images += 1;
                    parts.push((
                        key,
                        format!(
                            "partition-image:{}:{}:{}:{}",
                            disk.id,
                            partition.name,
                            resolved.display(),
                            path_state_signature(&resolved)
                        ),
                    ));
                }
                Err(error) => {
                    partition_resolution_errors += 1;
                    parts.push((
                        key,
                        format!(
                            "partition-image-resolution-error:{}:{}:{}:{}",
                            disk.id,
                            partition.name,
                            image.as_str(),
                            error
                        ),
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
    let archive_inputs = crate::reuse_assembly_archives::archive_input_entries(
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
    parts
}

/// The only subdirectory of `dir`, when it has exactly one.
fn single_subdir(dir: &std::path::Path) -> Option<PathBuf> {
    let mut subdirs = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir());
    let only = subdirs.next()?;
    subdirs.next().is_none().then_some(only)
}

/// The fingerprint's input signature: every entry's part, in order.
pub(crate) fn assembly_input_signature(spec: &ResolvedBuildSpec) -> String {
    assembly_input_entries(spec)
        .into_iter()
        .map(|(_, part)| part)
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_modules_and_published_copies_enter_the_fingerprint() {
        use gaia_spec::{
            AssemblyFilesystemKindSpec, AssemblyFilesystemSpec, AssemblyKernelModulesSpec,
            AssemblyTreeSpec, ImageAssemblySpec,
        };
        let root = std::env::temp_dir().join(format!("gaia-reuse-kernel-{}", std::process::id()));
        let kernel = root.join("images/modules/6.12.0");
        std::fs::create_dir_all(&kernel).expect("kernel dir");
        std::fs::write(kernel.join("modules.dep"), "kernel/a.ko:\n").expect("modules.dep");
        let mut spec = ResolvedBuildSpec::new("reuse-kernel-modules");
        spec.workspace.root_dir = root.display().to_string();
        spec.workspace.build_dir = root.join("build").display().to_string();
        spec.workspace.out_dir = root.join("out").display().to_string();
        spec.image.output.collect_dir = Some(root.join("images").display().to_string());
        spec.image.assembly = Some(ImageAssemblySpec {
            trees: vec![AssemblyTreeSpec {
                id: "initramfs".into(),
                path: "$assembly.work/initramfs".into(),
            }],
            kernel_modules: vec![AssemblyKernelModulesSpec {
                tree: "initramfs".into(),
                from: "$provider.images/modules".into(),
                kernel_version: Some("6.12.0".into()),
                modules: vec!["libcomposite".into()],
                depmod: None,
            }],
            filesystems: vec![AssemblyFilesystemSpec {
                id: "boot".into(),
                kind: AssemblyFilesystemKindSpec::CpioZstd,
                source_tree: "initramfs".into(),
                output: "$assembly.work/boot.cpio.zst".into(),
                size: None,
                deterministic: true,
                compression_level: None,
                publish: true,
            }],
            ..ImageAssemblySpec::default()
        });
        let before = assembly_input_signature(&spec);
        assert!(before.contains("kernel-modules:initramfs:"), "{before}");
        assert!(before.contains("published:boot:"), "{before}");
        if let Some(assembly) = spec.image.assembly.as_mut() {
            assembly.kernel_modules[0]
                .modules
                .push("usb_f_mass_storage".into());
        }
        assert_ne!(before, assembly_input_signature(&spec));
        let _ = std::fs::remove_dir_all(root);
    }
}
