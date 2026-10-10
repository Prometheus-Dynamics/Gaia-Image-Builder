use crate::reuse::{command_signature, path_state_signature};
use gaia_spec::ResolvedBuildSpec;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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
            let sysroot = initramfs
                .sysroot
                .as_ref()
                .and_then(|template| roots.resolve_path(spec, template).ok());
            parts.extend(busybox_runtime_parts(&resolved, sysroot.as_deref()));
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

/// The inputs a BusyBox's runtime libraries add: the sysroot, the
/// interpreter, every symlink and every library file with its content digest.
/// A changed libc under the sysroot therefore invalidates the assembly. A
/// closure that cannot be resolved is named by its error, so the fingerprint
/// never matches a run that could not have been built.
fn busybox_runtime_parts(binary: &Path, sysroot: Option<&Path>) -> Vec<(String, String)> {
    let closure = match crate::resolve_runtime_closure(binary, sysroot) {
        Ok(closure) => closure,
        Err(error) => {
            return vec![(
                format!("busybox runtime {}", binary.display()),
                format!("busybox-runtime:error:{error}"),
            )];
        }
    };
    let mut parts = Vec::new();
    let sysroot_text = closure
        .sysroot
        .as_ref()
        .map_or_else(|| "none".to_string(), |path| path.display().to_string());
    parts.push((
        "busybox runtime sysroot".into(),
        format!(
            "busybox-runtime:sysroot:{sysroot_text}:dynamic={}",
            closure.dynamic
        ),
    ));
    if let Some(interpreter) = &closure.interpreter {
        parts.push((
            format!("busybox runtime interpreter {interpreter}"),
            format!("busybox-runtime:interpreter:{interpreter}"),
        ));
    }
    for entry in &closure.entries {
        let part = match entry {
            crate::RuntimeEntry::File { guest, source } => match file_digest(source) {
                Ok(digest) => format!("file:{guest}:sha256={digest}"),
                Err(error) => format!("file:{guest}:error:{error}"),
            },
            crate::RuntimeEntry::Symlink { guest, target } => {
                format!("link:{guest}->{target}")
            }
        };
        parts.push((
            format!("busybox runtime {}", entry.guest()),
            format!("busybox-runtime:{part}"),
        ));
    }
    parts
}

fn file_digest(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod busybox_runtime_tests {
    use super::busybox_runtime_parts;
    use crate::elf::tests::{EM_AARCH64, ElfFixture, build_elf};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "gaia-reuse-busybox-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_changed_library_in_the_sysroot_changes_the_busybox_inputs() {
        let root = temp_root("libc");
        let sysroot = root.join("target");
        let busybox = sysroot.join("bin/busybox");
        fs::create_dir_all(busybox.parent().expect("bin")).expect("bin dir");
        fs::write(
            &busybox,
            build_elf(&ElfFixture {
                interpreter: Some("/lib/ld-fp.so.1"),
                ..ElfFixture::dynamic(EM_AARCH64, &["libc.so.6"])
            }),
        )
        .expect("busybox");
        fs::create_dir_all(sysroot.join("lib")).expect("lib dir");
        let loader = build_elf(&ElfFixture::dynamic(EM_AARCH64, &[]));
        fs::write(sysroot.join("lib/ld-fp.so.1"), &loader).expect("loader");
        let libc = sysroot.join("lib/libc.so.6");
        fs::write(&libc, build_elf(&ElfFixture::dynamic(EM_AARCH64, &[]))).expect("libc");

        let before = busybox_runtime_parts(&busybox, None);
        assert!(
            before.iter().all(|(_, part)| !part.contains("error:")),
            "{before:?}"
        );
        assert!(
            before
                .iter()
                .any(|(name, _)| name == "busybox runtime /lib/libc.so.6"),
            "{before:?}"
        );

        // A new libc (a different file with the same name) must change the inputs.
        fs::write(
            &libc,
            build_elf(&ElfFixture {
                runpath: Some("/usr/lib"),
                ..ElfFixture::dynamic(EM_AARCH64, &[])
            }),
        )
        .expect("new libc");
        let after = busybox_runtime_parts(&busybox, None);
        assert_ne!(before, after);

        // A missing library makes the inputs an error, which never matches a reuse.
        fs::remove_file(&libc).expect("remove libc");
        let missing = busybox_runtime_parts(&busybox, None);
        assert_eq!(missing.len(), 1);
        assert!(
            missing[0].1.starts_with("busybox-runtime:error:"),
            "{missing:?}"
        );
        let _ = fs::remove_dir_all(root);
    }
}
