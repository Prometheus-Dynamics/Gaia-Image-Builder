//! Config fragments: how a Buildroot `.config` is made from descriptions.
//!
//! The base is what Buildroot itself gives for `allnoconfig`. Each fragment
//! is text of `BR2_...=value` lines and `# BR2_... is not set` lines, from a
//! description. Fragments are applied in the order given; a later line for a
//! symbol replaces an earlier one for it, `is not set` included. The merged
//! file is then resolved by Buildroot's own `olddefconfig`. No defconfig is
//! written or edited by hand.

use crate::ConfigFragment;

/// The symbol a config line sets or unsets, `None` for a blank or comment
/// line, and an error for anything else.
fn symbol(line: &str) -> Result<Option<&str>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    // A comment is any `#` line, a bare `#` included; `# NAME is not set`
    // is the one comment that says something, when NAME is one word.
    if let Some(rest) = line.strip_prefix('#') {
        if let Some(name) = rest
            .strip_prefix(' ')
            .and_then(|rest| rest.strip_suffix(" is not set"))
            && !name.contains(char::is_whitespace)
        {
            return check(name).map(Some);
        }
        return Ok(None);
    }
    match line.split_once('=') {
        Some((name, _)) => check(name).map(Some),
        None => Err(format!("{line:?} is not NAME=value or # NAME is not set")),
    }
}

/// A Buildroot symbol: the `BR2_` prefix, then letters, digits and
/// underscores (`BR2_aarch64` is one).
fn check(name: &str) -> Result<&str, String> {
    let valid =
        name.starts_with("BR2_") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if valid {
        Ok(name)
    } else {
        Err(format!("{name:?} is not a BR2_ symbol"))
    }
}

/// The toolchain a build starts from when no description picks one: Bootlin's
/// prebuilt glibc toolchain, the stable release, for the build's architecture.
/// Each variant is named with the description lines that name its
/// architecture (see [`VARIANTS`]).
///
/// Buildroot's choice of Bootlin variant has no default of its own, so Kconfig
/// takes the first variant that fits, the bleeding-edge one, unless a variant
/// is named. Only the variant for the build's own architecture is written: a
/// line for another variant takes the choice over even when it does not fit,
/// and Kconfig then drops it and falls back.
const BOOTLIN: &str = "\
BR2_TOOLCHAIN_EXTERNAL=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y
";

/// The stable glibc variant for each architecture: the description lines that
/// name the architecture, and the variant's line. The 32-bit ARM variant is
/// the hard-float ARMv7 one; an ARM build that is not ARMv7 hard-float gets
/// Kconfig's own pick, as a build with no architecture line does.
const VARIANTS: [(&[&str], &str); 5] = [
    (
        &["BR2_aarch64=y"],
        "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_STABLE=y",
    ),
    (
        &["BR2_x86_64=y"],
        "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_X86_64_GLIBC_STABLE=y",
    ),
    (
        &["BR2_arm=y"],
        "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_ARMV7_EABIHF_GLIBC_STABLE=y",
    ),
    (
        &["BR2_riscv=y", "BR2_RISCV_64=y"],
        "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_RISCV64_LP64D_GLIBC_STABLE=y",
    ),
    (
        &["BR2_riscv=y", "BR2_RISCV_32=y"],
        "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_RISCV32_ILP32D_GLIBC_STABLE=y",
    ),
];

/// The fragment that starts every build with the toolchain default: none when
/// a description sets or unsets a Buildroot or external toolchain option, since
/// that description chose its own toolchain. `descriptions` are the config's
/// fragments as written, in order. Lines that do not parse are left for
/// [`merge`] to refuse.
pub fn defaults(descriptions: &[ConfigFragment]) -> Vec<ConfigFragment> {
    let chooses = descriptions.iter().any(|fragment| {
        fragment
            .text
            .lines()
            .filter_map(|line| symbol(line).ok().flatten())
            .any(chooses_toolchain)
    });
    if chooses {
        return Vec::new();
    }
    let mut text = BOOTLIN.to_string();
    if let Some(variant) = variant(descriptions) {
        text.push_str(variant);
        text.push('\n');
    }
    vec![ConfigFragment {
        name: "gaia.toolchain".into(),
        text,
    }]
}

/// The variant line for the architecture the descriptions name, if one does.
fn variant(descriptions: &[ConfigFragment]) -> Option<&'static str> {
    let names = |line: &str| {
        descriptions
            .iter()
            .any(|fragment| fragment.text.lines().any(|text| text.trim() == line))
    };
    VARIANTS
        .iter()
        .find(|(lines, _)| lines.iter().all(|line| names(line)))
        .map(|(_, variant)| *variant)
}

/// Whether a symbol is one of the options that pick a toolchain.
fn chooses_toolchain(name: &str) -> bool {
    name.starts_with("BR2_TOOLCHAIN_BUILDROOT") || name.starts_with("BR2_TOOLCHAIN_EXTERNAL")
}

/// `base` with every fragment applied in order. Each line of a fragment
/// drops the lines of the same symbol before it, then is added. The result
/// ends with a newline.
pub fn merge(base: &str, fragments: &[ConfigFragment]) -> Result<String, String> {
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
    for fragment in fragments {
        for (number, line) in fragment.text.lines().enumerate() {
            let name = match symbol(line) {
                Ok(Some(name)) => name.to_string(),
                Ok(None) => continue,
                Err(why) => {
                    return Err(format!(
                        "fragment {:?}, line {}: {why}",
                        fragment.name,
                        number + 1
                    ));
                }
            };
            lines.retain(|kept| !matches_symbol(kept, &name));
            lines.push(line.trim().to_string());
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    Ok(text)
}

/// `text` with each `@name@` of an input tree replaced by its path. An
/// `@...@` of letters, digits, `.`, `-` and `_` that names no input is an
/// error, so a misspelled name is not left for Buildroot to find.
pub fn substitute(text: &str, inputs: &[(String, std::path::PathBuf)]) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
            .unwrap_or(after.len());
        if name_end > 0 && after[name_end..].starts_with('@') {
            let name = &after[..name_end];
            let Some((_, path)) = inputs.iter().find(|(n, _)| n == name) else {
                return Err(format!("@{name}@ names no input tree"));
            };
            out.push_str(&path.display().to_string());
            rest = &after[name_end + 1..];
        } else {
            out.push('@');
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The lines of `fragments` that `resolved` (the `.config` after Buildroot's
/// own `olddefconfig`) does not have as written: each as `fragment: line`,
/// with what the config has for that symbol.
///
/// Buildroot drops a symbol it does not know and changes one whose
/// dependencies are not met, and says nothing. Without this, a misspelled
/// option, or a CPU Buildroot has no option for, builds something else.
/// A later fragment's line for the same symbol is the one that counts.
pub fn not_taken(resolved: &str, fragments: &[ConfigFragment]) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut wanted: BTreeMap<String, (String, String)> = BTreeMap::new();
    for fragment in fragments {
        for line in fragment.text.lines() {
            if let Ok(Some(name)) = symbol(line) {
                wanted.insert(
                    name.to_string(),
                    (fragment.name.clone(), line.trim().to_string()),
                );
            }
        }
    }
    let mut have: BTreeMap<&str, &str> = BTreeMap::new();
    for line in resolved.lines() {
        if let Ok(Some(name)) = symbol(line) {
            have.insert(name, line.trim());
        }
    }
    let mut missing = Vec::new();
    for (name, (fragment, line)) in &wanted {
        let unset = line.starts_with('#');
        match have.get(name.as_str()) {
            Some(got) if *got == line => {}
            // An unset symbol may simply be absent from the config.
            None if unset => {}
            Some(got) => missing.push(format!(
                "{fragment}: `{line}` became `{got}`{}",
                string_hint(line, got)
            )),
            None => missing.push(format!(
                "{fragment}: `{line}` was dropped (no such option, or its dependencies are not met)"
            )),
        }
    }
    missing
}

/// A Kconfig line of a kernel's or a bootloader's own config: the symbol it
/// sets or unsets, and its value (`None` for unset: `# CONFIG_X is not set`
/// or `CONFIG_X=n`).
fn kconfig_line(line: &str) -> Option<(&str, Option<&str>)> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("# ") {
        let name = rest.strip_suffix(" is not set")?;
        return name.starts_with("CONFIG_").then_some((name, None));
    }
    let (name, value) = line.split_once('=')?;
    if !name.starts_with("CONFIG_") || name.contains(char::is_whitespace) {
        return None;
    }
    Some((name, (value != "n").then_some(value)))
}

/// The lines of the fragment files (`name`, text) that a package's final
/// `.config` does not have as written, each as `file: line`, with what the
/// config has.
///
/// A kernel's or a bootloader's config merge drops a symbol that does not
/// exist and changes one whose dependencies are not met, and says nothing,
/// or only warns in a log nobody reads. So a fragment that asks for a
/// driver the tree does not have, or leaves a dependency off, builds a
/// kernel without it. A later file's line for a symbol is the one checked.
pub fn kconfig_not_taken(resolved: &str, fragments: &[(String, String)]) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut wanted: BTreeMap<&str, (&str, &str, Option<&str>)> = BTreeMap::new();
    for (file, text) in fragments {
        for line in text.lines() {
            if let Some((name, value)) = kconfig_line(line) {
                wanted.insert(name, (file.as_str(), line.trim(), value));
            }
        }
    }
    let mut have: BTreeMap<&str, (&str, Option<&str>)> = BTreeMap::new();
    for line in resolved.lines() {
        if let Some((name, value)) = kconfig_line(line) {
            have.insert(name, (line.trim(), value));
        }
    }
    let mut missing = Vec::new();
    for (name, (file, line, value)) in &wanted {
        match (value, have.get(name)) {
            // Unset is unset, however either side writes it.
            (None, None) | (None, Some((_, None))) => {}
            (Some(want), Some((_, Some(got)))) if want == got => {}
            (_, Some((got, _))) => missing.push(format!(
                "{file}: `{line}` became `{got}`{}",
                string_hint(line, got)
            )),
            (Some(_), None) => missing.push(format!(
                "{file}: `{line}` was dropped (no such symbol in this tree, or its dependencies are not met)"
            )),
        }
    }
    missing
}

/// The value of a string option in a resolved Buildroot config, unquoted.
pub fn string_option<'a>(resolved: &'a str, name: &str) -> Option<&'a str> {
    resolved.lines().find_map(|line| {
        let value = line.trim().strip_prefix(name)?.strip_prefix('=')?;
        value.strip_prefix('"')?.strip_suffix('"')
    })
}

fn matches_symbol(line: &str, name: &str) -> bool {
    matches!(symbol(line), Ok(Some(found)) if found == name)
}

/// What to write, when a line that unsets an option came out as an empty
/// string: the option is a string, which has no "is not set" form.
fn string_hint(line: &str, got: &str) -> String {
    match got.strip_suffix("=\"\"") {
        Some(name) if line.starts_with('#') => {
            format!(" ({name} is a string option: write {name}=\"\")")
        }
        _ => String::new(),
    }
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

    /// The toolchain default is there when no description chooses one: the
    /// two Bootlin lines, and the stable variant for the architecture the
    /// descriptions name. A description's own line replaces the default's.
    #[test]
    fn the_toolchain_default_is_there_unless_a_description_chooses_one() {
        let arm64 = [fragment("board.target", "BR2_aarch64=y\n")];
        let defaults = defaults(&arm64);
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].name, "gaia.toolchain");
        assert_eq!(
            defaults[0].text,
            "BR2_TOOLCHAIN_EXTERNAL=y\nBR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y\n\
             BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_STABLE=y\n"
        );
        let merged = merge(
            "BR2_aarch64=y\n",
            &[
                defaults[0].clone(),
                fragment("image", "BR2_PACKAGE_BUSYBOX=y\n"),
            ],
        )
        .expect("merge");
        assert_eq!(
            merged,
            "BR2_aarch64=y\nBR2_TOOLCHAIN_EXTERNAL=y\nBR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y\n\
             BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_STABLE=y\nBR2_PACKAGE_BUSYBOX=y\n"
        );
        // A description's line for the same symbol replaces the default's.
        let replaced = merge(
            "",
            &[
                defaults[0].clone(),
                fragment(
                    "board",
                    "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_STABLE=n\n",
                ),
            ],
        )
        .expect("merge");
        assert!(!replaced.contains("AARCH64_GLIBC_STABLE=y"), "{replaced}");
    }

    /// Only the variant for the build's own architecture is written, so no
    /// other variant's line takes the choice over.
    #[test]
    fn the_default_names_only_its_own_architectures_variant() {
        let variant_of = |descriptions: &[ConfigFragment]| {
            let text = &defaults(descriptions)[0].text;
            text.lines()
                .filter(|line| line.starts_with("BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            variant_of(&[fragment("t", "BR2_x86_64=y\n")]),
            ["BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_X86_64_GLIBC_STABLE=y"]
        );
        assert_eq!(
            variant_of(&[fragment("t", "BR2_arm=y\n")]),
            ["BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_ARMV7_EABIHF_GLIBC_STABLE=y"]
        );
        // RISC-V names its width as well as its architecture.
        assert_eq!(
            variant_of(&[fragment("t", "BR2_riscv=y\nBR2_RISCV_64=y\n")]),
            ["BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_RISCV64_LP64D_GLIBC_STABLE=y"]
        );
        assert_eq!(
            variant_of(&[fragment("t", "BR2_riscv=y\nBR2_RISCV_32=y\n")]),
            ["BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_RISCV32_ILP32D_GLIBC_STABLE=y"]
        );
        // No architecture line, no variant: Kconfig's own pick.
        let none = defaults(&[fragment("t", "BR2_PACKAGE_BUSYBOX=y\n")]);
        assert_eq!(
            none[0].text,
            "BR2_TOOLCHAIN_EXTERNAL=y\nBR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y\n"
        );
    }

    /// A description that sets or unsets a Buildroot or external toolchain
    /// option, in any form, chooses the toolchain, and the default is left out.
    #[test]
    fn the_toolchain_default_is_left_out_when_a_description_chooses_one() {
        for text in [
            "BR2_TOOLCHAIN_BUILDROOT=y\n",
            "BR2_TOOLCHAIN_BUILDROOT_GLIBC=y\n",
            "# BR2_TOOLCHAIN_BUILDROOT is not set\n",
            "BR2_TOOLCHAIN_EXTERNAL=y\n",
            "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_BLEEDING_EDGE=y\n",
            "BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_RISCV64_LP64D_GLIBC_STABLE=y\n",
            "# BR2_TOOLCHAIN_EXTERNAL_BOOTLIN is not set\n",
        ] {
            let descriptions = [
                fragment("board.target", "BR2_aarch64=y\n"),
                fragment("image", text),
            ];
            assert!(defaults(&descriptions).is_empty(), "{text:?}");
        }
        // A symbol that only starts like one of them is not a toolchain option.
        assert_eq!(
            defaults(&[fragment("image", "BR2_TOOLCHAIN_HEADERS_AT_LEAST_5_4=y\n")]).len(),
            1
        );
        // And with no descriptions at all, the default is there.
        assert_eq!(defaults(&[]).len(), 1);
    }

    #[test]
    fn a_later_fragment_replaces_an_earlier_line_for_the_same_symbol() {
        let base = "BR2_aarch64=y\n# BR2_PACKAGE_BUSYBOX is not set\n";
        let merged = merge(
            base,
            &[
                fragment("a", "BR2_PACKAGE_BUSYBOX=y\n"),
                fragment("b", "BR2_TARGET_GENERIC_HOSTNAME=\"one\"\n"),
                fragment("c", "BR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"),
            ],
        )
        .expect("merge");
        assert_eq!(
            merged,
            "BR2_aarch64=y\nBR2_PACKAGE_BUSYBOX=y\nBR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"
        );
    }

    #[test]
    fn an_unset_line_replaces_a_set_one() {
        let merged = merge(
            "BR2_PACKAGE_BUSYBOX=y\n",
            &[fragment("off", "# BR2_PACKAGE_BUSYBOX is not set\n")],
        )
        .expect("merge");
        assert_eq!(merged, "# BR2_PACKAGE_BUSYBOX is not set\n");
    }

    /// The compiler cache is on from the default fragment, and an image's
    /// `# BR2_CCACHE is not set` after it turns it off: no ccache line is
    /// left in the merged config.
    #[test]
    fn a_description_turns_the_compiler_cache_off_after_the_default() {
        let defaults = [fragment("gaia.ccache", "BR2_CCACHE=y\n")];
        let on = merge("", &defaults).expect("merge");
        assert_eq!(on, "BR2_CCACHE=y\n");
        let image = [
            fragment("gaia.ccache", "BR2_CCACHE=y\n"),
            fragment("image.0.os.fragment", "# BR2_CCACHE is not set\n"),
        ];
        let off = merge("", &image).expect("merge");
        assert_eq!(off, "# BR2_CCACHE is not set\n");
        assert!(!off.contains("BR2_CCACHE=y"));
    }

    /// The root is stripped from the `gaia.strip` default over the base's
    /// "not set", and an image's `# BR2_STRIP_strip is not set` after it
    /// keeps the symbols; either way the line that counts is checked as
    /// taken, and a default Buildroot dropped is named.
    #[test]
    fn a_description_keeps_the_symbols_after_the_strip_default() {
        let base = "# BR2_STRIP_strip is not set\n";
        let default = fragment("gaia.strip", "BR2_STRIP_strip=y\n");
        let on = merge(base, std::slice::from_ref(&default)).expect("merge");
        assert_eq!(on, "BR2_STRIP_strip=y\n");
        assert!(not_taken(&on, std::slice::from_ref(&default)).is_empty());
        let image = [
            default.clone(),
            fragment("image.0.os.fragment", "# BR2_STRIP_strip is not set\n"),
        ];
        let off = merge(base, &image).expect("merge");
        assert_eq!(off, "# BR2_STRIP_strip is not set\n");
        assert!(not_taken(&off, &image).is_empty());
        // Resolved without it (a target with no ELF binaries): said by name.
        assert_eq!(
            not_taken(base, std::slice::from_ref(&default)),
            ["gaia.strip: `BR2_STRIP_strip=y` became `# BR2_STRIP_strip is not set`"]
        );
    }

    #[test]
    fn blank_and_comment_lines_are_dropped() {
        let merged =
            merge("", &[fragment("c", "# a comment\n\n  BR2_aarch64=y  \n")]).expect("merge");
        assert_eq!(merged, "BR2_aarch64=y\n");
    }

    #[test]
    fn an_empty_fragment_list_keeps_the_base() {
        assert_eq!(
            merge("BR2_aarch64=y", &[]).expect("merge"),
            "BR2_aarch64=y\n"
        );
    }

    #[test]
    fn a_line_buildroot_dropped_or_changed_is_reported_with_its_fragment() {
        let fragments = [
            fragment(
                "board.target",
                "BR2_aarch64=y\nBR2_cortex_a76_cortex_a55=y\n",
            ),
            fragment(
                "image",
                "BR2_PACKAGE_BUSYBOX=y\nBR2_INIT_SYSTEMD=y\n# BR2_PACKAGE_FOO is not set\n",
            ),
            fragment("later", "BR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"),
        ];
        let resolved = "BR2_aarch64=y\nBR2_PACKAGE_BUSYBOX=y\n# BR2_INIT_SYSTEMD is not set\nBR2_TARGET_GENERIC_HOSTNAME=\"two\"\n";
        assert_eq!(
            not_taken(resolved, &fragments),
            [
                "image: `BR2_INIT_SYSTEMD=y` became `# BR2_INIT_SYSTEMD is not set`",
                "board.target: `BR2_cortex_a76_cortex_a55=y` was dropped (no such option, or its dependencies are not met)",
            ]
        );
        // Every line taken: nothing to report.
        assert!(not_taken(resolved, &fragments[2..]).is_empty());
        // A string option unset as a bool is: the refusal says what to write.
        assert_eq!(
            not_taken(
                "BR2_SYSTEM_DHCP=\"\"\n",
                &[fragment("image", "# BR2_SYSTEM_DHCP is not set\n")]
            ),
            [
                "image: `# BR2_SYSTEM_DHCP is not set` became `BR2_SYSTEM_DHCP=\"\"` (BR2_SYSTEM_DHCP is a string option: write BR2_SYSTEM_DHCP=\"\")"
            ]
        );
    }

    #[test]
    fn a_kernel_fragment_line_that_did_not_take_is_reported_with_its_file() {
        let fragments = vec![
            (
                "family/linux.fragment".to_string(),
                "CONFIG_EROFS_FS=y\nCONFIG_DM_VERITY=y\nCONFIG_USB_DEVICEFS=y\n# CONFIG_DEBUG_INFO is not set\nCONFIG_WLAN=n\n".to_string(),
            ),
            (
                "board/linux.fragment".to_string(),
                "CONFIG_USB_ETH=m\nCONFIG_LOCALVERSION=\"-board\"\nCONFIG_EROFS_FS=y\n".to_string(),
            ),
        ];
        let resolved = "CONFIG_EROFS_FS=y\n# CONFIG_DM_VERITY is not set\nCONFIG_USB_ETH=y\nCONFIG_LOCALVERSION=\"-board\"\n";
        assert_eq!(
            kconfig_not_taken(resolved, &fragments),
            [
                "family/linux.fragment: `CONFIG_DM_VERITY=y` became `# CONFIG_DM_VERITY is not set`",
                "family/linux.fragment: `CONFIG_USB_DEVICEFS=y` was dropped (no such symbol in this tree, or its dependencies are not met)",
                "board/linux.fragment: `CONFIG_USB_ETH=m` became `CONFIG_USB_ETH=y`",
            ]
        );
        // Unset both ways, and `=n`, are the same thing.
        assert!(
            kconfig_not_taken(
                "",
                &[("f".into(), "# CONFIG_A is not set\nCONFIG_B=n\n".into())]
            )
            .is_empty()
        );
        assert!(
            kconfig_not_taken(
                "# CONFIG_A is not set\n",
                &[("f".into(), "CONFIG_A=n\n".into())]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_string_option_is_read_unquoted() {
        let config = "BR2_A=y\nBR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"/a/x.fragment /b/y.fragment\"\nBR2_EMPTY=\"\"\n";
        assert_eq!(
            string_option(config, "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES"),
            Some("/a/x.fragment /b/y.fragment")
        );
        assert_eq!(string_option(config, "BR2_EMPTY"), Some(""));
        assert_eq!(string_option(config, "BR2_A"), None);
        assert_eq!(string_option(config, "BR2_MISSING"), None);
    }

    #[test]
    fn an_input_name_becomes_its_path() {
        let inputs = vec![
            (
                "zero.linux".to_string(),
                std::path::PathBuf::from("/s/objects/aa"),
            ),
            (
                "files.0".to_string(),
                std::path::PathBuf::from("/s/objects/bb"),
            ),
        ];
        let text = "BR2_X=\"@files.0@/a.fragment @files.0@/b\"\nBR2_Y=\"me@host\"\n";
        assert_eq!(
            substitute(text, &inputs).expect("substituted"),
            "BR2_X=\"/s/objects/bb/a.fragment /s/objects/bb/b\"\nBR2_Y=\"me@host\"\n"
        );
        assert_eq!(
            substitute("BR2_X=\"@zero.linux@\"", &inputs).expect("one"),
            "BR2_X=\"/s/objects/aa\""
        );
    }

    #[test]
    fn a_name_that_is_no_input_is_an_error() {
        let why = substitute("BR2_X=\"@nothing@/f\"", &[]).expect_err("unknown");
        assert_eq!(why, "@nothing@ names no input tree");
    }

    #[test]
    fn a_line_that_is_not_a_symbol_is_an_error_naming_it() {
        let why =
            merge("", &[fragment("bad", "BR2_aarch64=y\nCONFIG_X=y\n")]).expect_err("bad line");
        assert_eq!(
            why,
            "fragment \"bad\", line 2: \"CONFIG_X\" is not a BR2_ symbol"
        );
        let why = merge("", &[fragment("bad", "just words\n")]).expect_err("bad line");
        assert!(why.contains("is not NAME=value"), "{why}");
    }

    #[test]
    fn any_hash_line_is_a_comment_but_the_one_that_unsets_a_symbol() {
        // A bare `#`, a `#` with no space, prose, and prose that happens to
        // end in "is not set" are comments; one word before it is a symbol.
        for line in [
            "#",
            "#  ",
            "#no space",
            "# a note",
            "# this option is not set",
        ] {
            assert_eq!(symbol(line), Ok(None), "{line:?}");
        }
        assert_eq!(symbol("# BR2_X is not set"), Ok(Some("BR2_X")));
        assert!(
            symbol("# X is not set").is_err(),
            "one word that is no symbol"
        );
        assert!(symbol("words").is_err());
    }
}
