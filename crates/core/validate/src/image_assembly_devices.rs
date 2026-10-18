//! `[[image.assembly.devices]]`: device nodes and FIFOs the cpio filesystems
//! add to their archives. Checks the tree, the path, the kind's device
//! numbers and the permission bits, and that no two entries name one path.

use std::collections::HashSet;

use gaia_spec::{AssemblyDeviceKindSpec, AssemblyTreeId, ImageAssemblySpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_relative_dest;

/// The largest permission bits a device node takes (setuid, setgid, sticky
/// and rwx for each class).
const MODE_MAX: u32 = 0o7777;

pub(crate) fn validate_devices(
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.devices".into());
    let mut seen: HashSet<(AssemblyTreeId, String)> = HashSet::new();
    for device in &assembly.devices {
        if !tree_ids.contains(&device.tree) {
            diagnostics.push(error(
                "assembly_device_tree_unknown",
                format!(
                    "assembly device '{}' references unknown tree '{}'",
                    device.path, device.tree
                ),
                location(),
            ));
        }
        validate_relative_dest(&device.path, "assembly_device_path_invalid", diagnostics);
        if !seen.insert((device.tree.clone(), device.path.clone())) {
            diagnostics.push(error(
                "assembly_device_duplicate",
                format!(
                    "assembly device '{}' is listed twice for tree '{}'",
                    device.path, device.tree
                ),
                location(),
            ));
        }
        match (device.kind, device.major, device.minor) {
            (AssemblyDeviceKindSpec::Char | AssemblyDeviceKindSpec::Block, Some(_), Some(_)) => {}
            (AssemblyDeviceKindSpec::Char | AssemblyDeviceKindSpec::Block, _, _) => {
                diagnostics.push(error(
                    "assembly_device_numbers_missing",
                    format!(
                        "assembly {} device '{}' needs both major and minor",
                        device.kind.as_str(),
                        device.path
                    ),
                    location(),
                ));
            }
            (AssemblyDeviceKindSpec::Fifo, None, None) => {}
            (AssemblyDeviceKindSpec::Fifo, _, _) => {
                diagnostics.push(error(
                    "assembly_device_numbers_unsupported",
                    format!(
                        "assembly fifo '{}' takes no major or minor device number",
                        device.path
                    ),
                    location(),
                ));
            }
        }
        if let Some(mode) = &device.mode {
            let valid = (1..=4).contains(&mode.len())
                && u32::from_str_radix(mode, 8).is_ok_and(|bits| bits <= MODE_MAX)
                && mode.bytes().all(|byte| (b'0'..=b'7').contains(&byte));
            if !valid {
                diagnostics.push(error(
                    "assembly_device_mode_invalid",
                    format!(
                        "assembly device '{}' mode '{mode}' must be octal permission bits such as 0600",
                        device.path
                    ),
                    location(),
                ));
            }
        }
    }
}
