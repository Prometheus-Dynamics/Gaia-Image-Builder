//! The files a U-Boot build is asked to install under names of its own
//! (`BR2_TARGET_UBOOT_FORMAT_CUSTOM_NAME`: the board's raw areas name them,
//! `u-boot-rockchip.bin`), checked against the pinned U-Boot tree before
//! anything is built. A file U-Boot makes is named by a rule in one of its
//! makefiles or by a binman description in a device tree include; a name in
//! neither cannot be made, and without this the build fails after U-Boot
//! has compiled, when Buildroot copies a file that is not there.
//!
//! Only a pinned tree is read (a board's `[bootloader] source`). A U-Boot
//! that Buildroot downloads itself is not there to read before the build,
//! and is not checked.

use gaia_types::{ConfigFragment, UBOOT_FORMATS as FORMATS};
use std::path::Path;

/// The fragments' last line for `option`: whether it is `=y`, and for a
/// string its words.
fn last(fragments: &[ConfigFragment], option: &str) -> (bool, Vec<String>) {
    let (set, unset) = (format!("{option}="), format!("# {option} is not set"));
    let mut found = (false, Vec::new());
    for fragment in fragments {
        for line in fragment.text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix(set.as_str()) {
                let words = value
                    .trim_matches('"')
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
                found = (value == "y", words);
            } else if line == unset {
                found = (false, Vec::new());
            }
        }
    }
    found
}

/// The files the U-Boot build installs with these fragments: each format
/// that is on, the custom names, and the SPL's files.
pub fn installs(fragments: &[ConfigFragment]) -> Vec<String> {
    let mut names: Vec<String> = FORMATS
        .iter()
        .filter(|(option, _)| last(fragments, option).0)
        .map(|(_, file)| (*file).to_string())
        .collect();
    if last(fragments, "BR2_TARGET_UBOOT_FORMAT_CUSTOM").0 {
        names.extend(last(fragments, "BR2_TARGET_UBOOT_FORMAT_CUSTOM_NAME").1);
    }
    if last(fragments, "BR2_TARGET_UBOOT_SPL").0 {
        for path in last(fragments, "BR2_TARGET_UBOOT_SPL_NAME").1 {
            names.push(path.rsplit('/').next().unwrap_or(&path).to_string());
        }
    }
    names
}

/// Whether a file of the tree is one that names what the build makes: a
/// makefile, or a device tree source or include (binman's descriptions).
fn describes_outputs(name: &str) -> bool {
    name == "Makefile"
        || name.starts_with("Makefile.")
        || [".mk", ".dtsi", ".dts"]
            .iter()
            .any(|end| name.ends_with(end))
}

/// Whether a makefile or device tree source under `dir` has `needle`.
fn names(dir: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if entry.file_name() != ".git" && names(&path, needle) {
                return true;
            }
        } else if kind.is_file()
            && describes_outputs(&entry.file_name().to_string_lossy())
            && std::fs::read(&path)
                .is_ok_and(|text| text.windows(needle.len()).any(|part| part == needle))
        {
            return true;
        }
    }
    false
}

/// Why the build cannot install the custom-named files it is asked for:
/// each name that no makefile and no device tree source of the U-Boot tree
/// has. None when each is named there, or when none is asked for.
pub fn missing(fragments: &[ConfigFragment], uboot: &Path) -> Option<String> {
    if !last(fragments, "BR2_TARGET_UBOOT_FORMAT_CUSTOM").0 {
        return None;
    }
    let wanted = last(fragments, "BR2_TARGET_UBOOT_FORMAT_CUSTOM_NAME").1;
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|name| !names(uboot, name.as_bytes()))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let made: Vec<String> = installs(fragments)
        .into_iter()
        .filter(|name| !missing.contains(&name))
        .collect();
    Some(format!(
        "the pinned U-Boot cannot make {}: no makefile and no device tree source in it (a binman description) names {}. A file a board's area is written from (`content`) must be one this U-Boot's build makes; without {} the build would install {}",
        missing
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", "),
        if missing.len() == 1 { "it" } else { "them" },
        if missing.len() == 1 { "it" } else { "them" },
        if made.is_empty() {
            "nothing of U-Boot's".to_string()
        } else {
            made.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(name: &str, text: &str) -> ConfigFragment {
        ConfigFragment {
            name: name.into(),
            text: text.into(),
        }
    }

    /// A custom name is looked for in the tree's makefiles and device tree
    /// sources, anywhere under it; one that is in neither is named, with
    /// what the build would install without it. A name only some other file
    /// has (a document) does not count.
    #[test]
    fn a_custom_name_the_tree_does_not_make_is_named() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tree = dir.path();
        std::fs::create_dir_all(tree.join("arch/arm/dts")).expect("mkdir");
        std::fs::create_dir_all(tree.join("doc")).expect("mkdir");
        std::fs::write(
            tree.join("Makefile"),
            "u-boot-sunxi-with-spl.bin: spl/sunxi-spl.bin u-boot.img FORCE\n",
        )
        .expect("write");
        std::fs::write(
            tree.join("arch/arm/dts/rockchip-u-boot.dtsi"),
            "simple-bin { filename = \"u-boot-rockchip.bin\"; };\n",
        )
        .expect("write");
        std::fs::write(tree.join("doc/board.rst"), "write u-boot-other.bin\n").expect("write");
        let with = |names: &str| {
            [
                fragment(
                    "board.bootloader",
                    &format!(
                        "BR2_TARGET_UBOOT=y\nBR2_TARGET_UBOOT_FORMAT_BIN=y\nBR2_TARGET_UBOOT_FORMAT_CUSTOM=y\nBR2_TARGET_UBOOT_FORMAT_CUSTOM_NAME=\"{names}\"\n"
                    ),
                ),
                fragment(
                    "image.0.os.fragment",
                    "BR2_TARGET_UBOOT_SPL=y\nBR2_TARGET_UBOOT_SPL_NAME=\"spl/u-boot-spl.bin\"\n",
                ),
            ]
        };
        assert_eq!(missing(&with("u-boot-sunxi-with-spl.bin"), tree), None);
        assert_eq!(missing(&with("u-boot-rockchip.bin"), tree), None);
        assert_eq!(
            missing(&with("u-boot-rockchip.bin u-boot-other.bin"), tree).as_deref(),
            Some(
                "the pinned U-Boot cannot make `u-boot-other.bin`: no makefile and no device tree source in it (a binman description) names it. A file a board's area is written from (`content`) must be one this U-Boot's build makes; without it the build would install u-boot.bin, u-boot-rockchip.bin, u-boot-spl.bin"
            )
        );
        // No custom name asked for, or the option turned off after it: nothing to check.
        assert_eq!(
            missing(
                &[fragment("board.bootloader", "BR2_TARGET_UBOOT=y\n")],
                tree
            ),
            None
        );
        let mut off = with("u-boot-other.bin").to_vec();
        off.push(fragment(
            "image.1",
            "# BR2_TARGET_UBOOT_FORMAT_CUSTOM is not set\n",
        ));
        assert_eq!(missing(&off, tree), None);
    }
}
