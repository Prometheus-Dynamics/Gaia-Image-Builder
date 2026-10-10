//! The kernel modules step: copies named modules, with their dependency
//! closure from the kernel's `modules.dep`, into a tree, then runs `depmod`
//! over the tree so the copied set is what `modprobe` sees.

use super::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Module file suffixes, longest first, that a module name drops.
const MODULE_SUFFIXES: [&str; 6] = [".ko.xz", ".ko.zst", ".ko.gz", ".ko.bz2", ".ko.lz4", ".ko"];
/// Files of the kernel's module directory a target needs for builtins.
const MODULE_METADATA: [&str; 3] = [
    "modules.order",
    "modules.builtin",
    "modules.builtin.modinfo",
];

pub(super) struct KernelModulesSummary {
    pub(super) kernel_version: String,
    /// The module files copied, relative to the kernel directory.
    pub(super) copied: Vec<String>,
    /// Requested modules the kernel has built in: nothing to copy.
    pub(super) builtin: Vec<String>,
    pub(super) depmod: String,
    pub(super) depmod_version: Option<String>,
}

pub(super) fn execute_kernel_modules(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    modules: &gaia_spec::AssemblyKernelModulesSpec,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<KernelModulesSummary, AssemblyError> {
    let tree = roots.tree_path(&modules.tree)?.to_path_buf();
    let from = roots.resolve_path(spec, modules.from.as_str())?;
    let kernel_version = select_kernel_version(&from, modules.kernel_version.as_deref())?;
    let kernel_dir = from.join(&kernel_version);

    let dependencies = read_modules_dep(&kernel_dir)?;
    let builtin_names = read_builtin_names(&kernel_dir)?;
    let by_name = dependencies
        .keys()
        .map(|relative| (module_name(relative), relative.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut requested = BTreeSet::new();
    let mut builtin = Vec::new();
    for name in &modules.modules {
        let normalized = module_name(name);
        if let Some(relative) = by_name.get(&normalized) {
            requested.insert(relative.clone());
        } else if builtin_names.contains(&normalized) {
            builtin.push(name.clone());
        } else {
            return Err(format!(
                "kernel module '{name}' for tree '{}' was not found in '{}' (neither in modules.dep nor in modules.builtin)",
                modules.tree,
                kernel_dir.display()
            )
            .into());
        }
    }

    let closure = dependency_closure(&dependencies, requested)?;
    let target_dir = tree.join("lib/modules").join(&kernel_version);
    for relative in &closure {
        let source = kernel_dir.join(relative);
        let dest = target_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std_fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create kernel module dir '{}': {error}",
                    parent.display()
                )
            })?;
        }
        std_fs::copy(&source, &dest).map_err(|error| {
            format!(
                "failed to copy kernel module '{}' to '{}': {error}",
                source.display(),
                dest.display()
            )
        })?;
    }
    for name in MODULE_METADATA {
        let source = kernel_dir.join(name);
        if source.is_file() {
            std_fs::create_dir_all(&target_dir).map_err(|error| {
                format!(
                    "failed to create kernel module dir '{}': {error}",
                    target_dir.display()
                )
            })?;
            std_fs::copy(&source, target_dir.join(name)).map_err(|error| {
                format!(
                    "failed to copy kernel module metadata '{}': {error}",
                    source.display()
                )
            })?;
        }
    }

    let depmod = resolve_depmod(spec, roots, modules)?;
    let mut command = Command::new(&depmod.program);
    command.arg("-b").arg(&tree).arg(&kernel_version);
    let output = run_command_capture_tail(
        spec,
        &mut command,
        process_output_retention(spec),
        cancel_check,
    )?;
    if !output.status.success() {
        return Err(format!(
            "depmod failed for kernel modules of tree '{}' using '{}': {}",
            modules.tree,
            depmod.display,
            output.failure_context(&command)
        )
        .into());
    }

    Ok(KernelModulesSummary {
        kernel_version,
        copied: closure.into_iter().collect(),
        builtin,
        depmod_version: tool_version(&depmod, ["--version"]),
        depmod: depmod.display,
    })
}

/// The `<kernel version>` directory to take modules from.
fn select_kernel_version(from: &Path, requested: Option<&str>) -> Result<String, String> {
    if let Some(version) = requested {
        if from.join(version).is_dir() {
            return Ok(version.to_string());
        }
        return Err(format!(
            "kernel version '{version}' has no directory in '{}'",
            from.display()
        ));
    }
    let entries = std_fs::read_dir(from).map_err(|error| {
        format!(
            "failed to read kernel modules dir '{}': {error}",
            from.display()
        )
    })?;
    let mut versions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read kernel modules dir '{}': {error}",
                from.display()
            )
        })?;
        if entry.path().is_dir() {
            versions.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    versions.sort();
    match versions.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(format!(
            "kernel modules dir '{}' holds no kernel version directory",
            from.display()
        )),
        several => Err(format!(
            "kernel modules dir '{}' holds several kernel versions ({}); set kernel_version",
            from.display(),
            several.join(", ")
        )),
    }
}

/// `modules.dep`: each module (relative to the kernel dir) and its
/// dependencies, also relative.
fn read_modules_dep(kernel_dir: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let path = kernel_dir.join("modules.dep");
    let text = std_fs::read_to_string(&path).map_err(|error| {
        format!(
            "kernel modules need '{}' (the kernel's depmod output): {error}",
            path.display()
        )
    })?;
    let mut dependencies = BTreeMap::new();
    for line in text.lines() {
        let Some((module, deps)) = line.split_once(':') else {
            continue;
        };
        dependencies.insert(
            module.trim().to_string(),
            deps.split_whitespace().map(str::to_string).collect(),
        );
    }
    Ok(dependencies)
}

/// The normalized names of the modules the kernel has built in.
fn read_builtin_names(kernel_dir: &Path) -> Result<HashSet<String>, String> {
    let path = kernel_dir.join("modules.builtin");
    if !path.is_file() {
        return Ok(HashSet::new());
    }
    let text = std_fs::read_to_string(&path).map_err(|error| {
        format!(
            "failed to read kernel builtin list '{}': {error}",
            path.display()
        )
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(module_name)
        .collect())
}

/// Every module in `requested` and, transitively, its dependencies.
fn dependency_closure(
    dependencies: &BTreeMap<String, Vec<String>>,
    requested: BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let mut closure = BTreeSet::new();
    let mut pending: Vec<String> = requested.into_iter().collect();
    while let Some(relative) = pending.pop() {
        if !closure.insert(relative.clone()) {
            continue;
        }
        let deps = dependencies
            .get(&relative)
            .ok_or_else(|| format!("kernel module '{relative}' is missing from modules.dep"))?;
        for dep in deps {
            if !dependencies.contains_key(dep) {
                return Err(format!(
                    "kernel module '{relative}' depends on '{dep}', which modules.dep does not list"
                ));
            }
            pending.push(dep.clone());
        }
    }
    Ok(closure)
}

/// The name `modprobe` knows a module by: the file name without its
/// directories and module suffix, with `-` read as `_`.
fn module_name(raw: &str) -> String {
    let file = raw.rsplit('/').next().unwrap_or(raw);
    let stem = MODULE_SUFFIXES
        .iter()
        .find_map(|suffix| file.strip_suffix(suffix))
        .unwrap_or(file);
    stem.replace('-', "_")
}

/// The `depmod` to run: the one the spec names, else the provider's
/// `sbin/depmod`, else the host's.
fn resolve_depmod(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    modules: &gaia_spec::AssemblyKernelModulesSpec,
) -> Result<ResolvedTool, AssemblyError> {
    if let Some(template) = &modules.depmod {
        let path = roots.resolve_path(spec, template.as_str())?;
        if !path.is_file() {
            return Err(format!(
                "depmod '{}' for kernel modules of tree '{}' does not exist",
                path.display(),
                modules.tree
            )
            .into());
        }
        return Ok(ResolvedTool {
            display: path.display().to_string(),
            program: path,
        });
    }
    if let Some(host) = &roots.provider_host {
        let candidate = host.join("sbin/depmod");
        if candidate.is_file() {
            return Ok(ResolvedTool {
                display: candidate.display().to_string(),
                program: candidate,
            });
        }
    }
    resolve_assembly_tool(roots, "depmod").map_err(|_| {
        AssemblyError::from(format!(
            "kernel modules for tree '{}' need depmod: set depmod, or install kmod on the host",
            modules.tree
        ))
    })
}

#[cfg(test)]
mod name_tests {
    use super::module_name;

    #[test]
    fn module_names_drop_paths_suffixes_and_read_dashes_as_underscores() {
        assert_eq!(module_name("libcomposite"), "libcomposite");
        assert_eq!(module_name("usb-f-mass-storage.ko"), "usb_f_mass_storage");
        assert_eq!(
            module_name("kernel/drivers/usb/gadget/libcomposite.ko.xz"),
            "libcomposite"
        );
        assert_eq!(module_name("kernel/a/b.ko.zst"), "b");
    }
}
