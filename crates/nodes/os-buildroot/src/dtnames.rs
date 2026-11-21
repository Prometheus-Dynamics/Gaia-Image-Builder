//! The in-tree device trees a kernel build is asked for
//! (`BR2_LINUX_KERNEL_INTREE_DTS_NAME`: the board's own and its extra ones),
//! checked against the kernel tree, and the board's own device tree
//! directories (`BR2_LINUX_KERNEL_CUSTOM_DTS_DIR`, which Buildroot copies
//! into the kernel tree), before anything is built. A name with no
//! source fails the build at once, by name, instead of the kernel's make
//! failing after the kernel is built with "No rule to make target".

use gaia_types::ConfigFragment;
use std::path::Path;

/// The kernel's architecture directory (`arch/<dir>`) for the Buildroot
/// architecture the fragments select, or none for one not known here.
fn arch_dir(fragments: &[ConfigFragment]) -> Option<&'static str> {
    let on = |option: &str| {
        fragments
            .iter()
            .any(|f| f.text.lines().any(|l| l.trim() == format!("{option}=y")))
    };
    [
        ("BR2_aarch64", "arm64"),
        ("BR2_arm", "arm"),
        ("BR2_riscv", "riscv"),
        ("BR2_x86_64", "x86"),
    ]
    .into_iter()
    .find(|(option, _)| on(option))
    .map(|(_, dir)| dir)
}

/// The words of the fragments' last `option` line, in order.
fn words(fragments: &[ConfigFragment], option: &str) -> Vec<String> {
    let prefix = format!("{option}=");
    let mut found = Vec::new();
    for fragment in fragments {
        for line in fragment.text.lines() {
            if let Some(value) = line.trim().strip_prefix(prefix.as_str()) {
                found = value
                    .trim_matches('"')
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
            }
        }
    }
    found
}

/// Why the build cannot make the device trees it is asked for: each name
/// with no `<name>.dts` under the kernel tree's `arch/<arch>/boot/dts` or
/// under one of the board's device tree directories.
/// None when every one is there, when no device tree is asked for, or when
/// the architecture is not one known here.
pub fn missing(fragments: &[ConfigFragment], kernel: &Path) -> Option<String> {
    let wanted = words(fragments, "BR2_LINUX_KERNEL_INTREE_DTS_NAME");
    let arch = arch_dir(fragments)?;
    let mut dirs = vec![kernel.join("arch").join(arch).join("boot/dts")];
    dirs.extend(
        words(fragments, "BR2_LINUX_KERNEL_CUSTOM_DTS_DIR")
            .iter()
            .map(std::path::PathBuf::from),
    );
    let missing: Vec<String> = wanted
        .iter()
        .filter(|name| {
            !dirs
                .iter()
                .any(|dir| dir.join(format!("{name}.dts")).is_file())
        })
        .cloned()
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "the kernel tree has no source for {} (looked for arch/{arch}/boot/dts/<name>.dts, and <name>.dts in the board's device tree directory): a device tree the board names (`[kernel] device_tree` or `extra_device_trees`) must be in the pinned kernel or the board's directory",
        missing.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(text: &str) -> ConfigFragment {
        ConfigFragment {
            name: "board.kernel".into(),
            text: text.into(),
        }
    }

    /// Each name is looked for under the architecture's dts directory; a
    /// name in a subdirectory (an overlay map) is a path there; a missing
    /// one is named, and only it.
    #[test]
    fn a_device_tree_the_kernel_has_no_source_for_is_named() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dts = dir.path().join("arch/arm64/boot/dts");
        std::fs::create_dir_all(dts.join("broadcom")).expect("dir");
        std::fs::create_dir_all(dts.join("overlays")).expect("dir");
        std::fs::write(dts.join("broadcom/board.dts"), "/dts-v1/;").expect("dts");
        std::fs::write(dts.join("overlays/overlay_map.dts"), "/dts-v1/;").expect("dts");
        let target = fragment("BR2_aarch64=y\n");
        let asked = |names: &str| {
            [
                target.clone(),
                fragment(&format!("BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"{names}\"\n")),
            ]
        };
        assert_eq!(
            missing(&asked("broadcom/board overlays/overlay_map"), dir.path()),
            None
        );
        let why = missing(
            &asked("broadcom/board overlays/overlay_map overlays/hat_map"),
            dir.path(),
        )
        .expect("one is missing");
        assert!(
            why.starts_with("the kernel tree has no source for overlays/hat_map (looked for arch/arm64/boot/dts/<name>.dts,"),
            "{why}"
        );
        // A later fragment's line replaces an earlier one's, as in the
        // configuration Buildroot reads.
        let replaced = [
            target.clone(),
            fragment("BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"overlays/hat_map\"\n"),
            fragment("BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"broadcom/board\"\n"),
        ];
        assert_eq!(missing(&replaced, dir.path()), None);
        // The architecture's own directory, not another's.
        let riscv = [
            fragment("BR2_riscv=y\n"),
            fragment("BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"broadcom/board\"\n"),
        ];
        assert!(missing(&riscv, dir.path()).is_some());
        // The board's own directory is looked in too.
        let own = dir.path().join("board-dts");
        std::fs::create_dir_all(own.join("canaan")).expect("dir");
        std::fs::write(own.join("canaan/own.dts"), "/dts-v1/;").expect("dts");
        let with_dir = [
            target.clone(),
            fragment(&format!(
                "BR2_LINUX_KERNEL_INTREE_DTS_NAME=\"canaan/own overlays/overlay_map\"\nBR2_LINUX_KERNEL_CUSTOM_DTS_DIR=\"{}\"\n",
                own.display()
            )),
        ];
        assert_eq!(missing(&with_dir, dir.path()), None);
        assert!(missing(&asked("canaan/own"), dir.path()).is_some());
        // Nothing asked: nothing to check.
        assert_eq!(missing(&[target], dir.path()), None);
    }
}
