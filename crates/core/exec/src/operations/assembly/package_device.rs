//! The device checks of `[image.assembly.packages.device]`: every ELF file of a
//! package's tree must run on the device its sysroot describes.
//!
//! The device is read from the sysroot, never from the host: its C library
//! gives the architecture (class, byte order and machine) every file must
//! have, and, unless `max_glibc` says otherwise, the highest `GLIBC_x.y` a
//! file may need. Each file's needed symbol versions, its `PT_LOAD`
//! alignments and its `DT_NEEDED` libraries are checked; a library is found
//! in the sysroot or in the package itself. The ELF reader only reads the file
//! bytes, so a file of another architecture is reported, not run.

use std::fs;
use std::path::{Path, PathBuf};

use gaia_plan::{ElfObject, find_sysroot_library, missing_runtime_libraries_in_roots, parse_elf};
use gaia_spec::{
    AssemblyPackageDeviceSpec, AssemblyPackageSpec, DEFAULT_DEVICE_MIN_PAGE_SIZE,
    ResolvedBuildSpec, parse_glibc_version, resolve_toolchain, resolve_workspace_path,
};

/// What the device checks read: the files that were ELF objects.
pub(super) struct DeviceCheck {
    /// ELF files in the package's tree.
    pub(super) elf_files: usize,
}

/// Checks every ELF file under `tree` (the package's data, as installed)
/// against the device `device` names. Every problem is reported, one per line.
pub(super) fn check_device(
    spec: &ResolvedBuildSpec,
    package: &AssemblyPackageSpec,
    device: &AssemblyPackageDeviceSpec,
    tree: &Path,
) -> Result<DeviceCheck, String> {
    let id = package.id.as_str();
    let sysroot =
        device_sysroot(spec, device).map_err(|error| format!("package '{id}': {error}"))?;
    let libc_path = find_sysroot_library(&sysroot, "libc.so.6")
        .map_err(|error| format!("package '{id}': {error}"))?
        .ok_or_else(|| {
            format!(
                "package '{id}': the device sysroot '{}' has no libc.so.6",
                sysroot.display()
            )
        })?;
    let libc = read_object(&libc_path).map_err(|error| format!("package '{id}': {error}"))?;
    let architecture = libc.architecture();
    let max_glibc = match &device.max_glibc {
        Some(text) => parse_glibc_version(text).ok_or_else(|| {
            format!("package '{id}': max_glibc '{text}' is not a GLIBC version")
        })?,
        None => highest_glibc(&libc.version_defs).ok_or_else(|| {
            format!(
                "package '{id}': the sysroot's libc.so.6 defines no GLIBC_x.y version to take max_glibc from"
            )
        })?,
    };

    let min_page_size =
        min_page_size(spec, device).map_err(|error| format!("package '{id}': {error}"))?;
    let mut files = Vec::new();
    list_regular_files(tree, &mut files).map_err(|error| format!("package '{id}': {error}"))?;
    files.sort();
    let mut problems = Vec::new();
    let mut elf_files = 0;
    for path in &files {
        let Ok(bytes) = fs::read(path) else {
            problems.push(format!("{}: cannot be read", guest_path(tree, path)));
            continue;
        };
        if !bytes.starts_with(b"\x7fELF") {
            continue;
        }
        elf_files += 1;
        let guest = guest_path(tree, path);
        let object = match parse_elf(&bytes) {
            Ok(object) => object,
            Err(error) => {
                problems.push(format!("{guest}: is not a readable ELF file ({error})"));
                continue;
            }
        };
        if object.architecture() != architecture {
            problems.push(format!(
                "{guest}: is {}, but the device is {}",
                describe(&object),
                describe_architecture(architecture)
            ));
            continue;
        }
        for need in &object.version_needs {
            if !need.version.starts_with("GLIBC_") {
                continue;
            }
            // GLIBC_PRIVATE and other non-numeric names are not a level a
            // device can be too old for.
            if let Some(version) = parse_glibc_version(&need.version)
                && version > max_glibc
            {
                problems.push(format!(
                    "{guest}: needs {} from {}, above the device's max_glibc {}.{}",
                    need.version, need.file, max_glibc.0, max_glibc.1
                ));
            }
        }
        if let Some(align) = object
            .load_alignments
            .iter()
            .copied()
            .find(|align| *align < min_page_size)
        {
            problems.push(format!(
                "{guest}: has a PT_LOAD with p_align {align:#x}, below the device's min_page_size {:#x}",
                min_page_size
            ));
        }
        if device.check_needed && object.is_dynamic() {
            match missing_runtime_libraries_in_roots(path, &[tree, sysroot.as_path()]) {
                Ok(missing) => {
                    for message in missing {
                        problems.push(format!("{guest}: cannot find {message}"));
                    }
                }
                Err(error) => problems.push(format!("{guest}: {error}")),
            }
        }
    }
    if problems.is_empty() {
        Ok(DeviceCheck { elf_files })
    } else {
        Err(format!(
            "package '{id}' fails its device checks against sysroot '{}' ({} problem{}):\n  {}",
            sysroot.display(),
            problems.len(),
            if problems.len() == 1 { "" } else { "s" },
            problems.join("\n  ")
        ))
    }
}

/// The smallest `PT_LOAD` alignment checked: the device table's
/// `min_page_size` when it sets one, else the device toolchain's `page_size`,
/// else the default (4 KiB).
fn min_page_size(
    spec: &ResolvedBuildSpec,
    device: &AssemblyPackageDeviceSpec,
) -> Result<u64, String> {
    if let Some(explicit) = device.min_page_size {
        return Ok(explicit);
    }
    if let Some(id) = &device.toolchain
        && let Some(page) = resolve_toolchain(spec, id)?.page_size
    {
        return Ok(page);
    }
    Ok(DEFAULT_DEVICE_MIN_PAGE_SIZE)
}

/// The sysroot the device names: a toolchain's, or the directory given.
fn device_sysroot(
    spec: &ResolvedBuildSpec,
    device: &AssemblyPackageDeviceSpec,
) -> Result<PathBuf, String> {
    let sysroot = match (&device.toolchain, &device.sysroot) {
        (Some(id), _) => resolve_toolchain(spec, id)?.sysroot,
        (None, Some(path)) => resolve_workspace_path(&spec.workspace, path)
            .map_err(|error| format!("device sysroot: {error}"))?,
        (None, None) => return Err("the device names no sysroot".into()),
    };
    if !sysroot.is_dir() {
        return Err(format!(
            "the device sysroot '{}' is not a directory (a toolchain's sysroot is there once its source is materialized)",
            sysroot.display()
        ));
    }
    Ok(sysroot)
}

fn read_object(path: &Path) -> Result<ElfObject, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    parse_elf(&bytes).map_err(|error| format!("'{}' is not an ELF file: {error}", path.display()))
}

/// The highest `GLIBC_x.y` among the versions a C library defines.
fn highest_glibc(defs: &[String]) -> Option<(u32, u32)> {
    defs.iter()
        .filter(|name| name.starts_with("GLIBC_"))
        .filter_map(|name| parse_glibc_version(name))
        .max()
}

/// Every regular file under `dir`, found without following symlinks.
fn list_regular_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to stat '{}': {error}", entry.path().display()))?;
        if file_type.is_dir() {
            list_regular_files(&entry.path(), out)?;
        } else if file_type.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// The package's file as the device sees it: `/usr/bin/tool`.
fn guest_path(tree: &Path, path: &Path) -> String {
    match path.strip_prefix(tree) {
        Ok(relative) => format!("/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

/// `(is_64bit, little_endian, machine)` in words a message can use.
fn describe(object: &ElfObject) -> String {
    let (is_64bit, little_endian, machine) = object.architecture();
    format!(
        "a {}-bit {}-endian {} file",
        if is_64bit { 64 } else { 32 },
        if little_endian { "little" } else { "big" },
        machine_name(machine)
    )
}

fn describe_architecture((is_64bit, little_endian, machine): (bool, bool, u16)) -> String {
    format!(
        "a {}-bit {}-endian {} system",
        if is_64bit { 64 } else { 32 },
        if little_endian { "little" } else { "big" },
        machine_name(machine)
    )
}

fn machine_name(machine: u16) -> String {
    match machine {
        3 => "x86".into(),
        8 => "MIPS".into(),
        20 => "PowerPC".into(),
        21 => "PowerPC64".into(),
        40 => "ARM".into(),
        62 => "x86-64".into(),
        183 => "AArch64".into(),
        243 => "RISC-V".into(),
        other => format!("e_machine {other}"),
    }
}
