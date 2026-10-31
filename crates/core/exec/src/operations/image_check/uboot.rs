//! U-Boot's boot files, for every tree a U-Boot board boots from: the
//! assembly trees, Buildroot's `images/` and the rootfs (`/boot`). Reads only.
//!
//! - `extlinux/extlinux.conf` (at the tree root or under `boot/`), as U-Boot's
//!   `sysboot` reads it: each label's `kernel`/`linux` (a `#<config>` suffix
//!   names a FIT configuration), `initrd` (comma-separated), `fdt`/
//!   `devicetree`, `fdtdir`/`devicetreedir` and `fdtoverlays` must be in the
//!   tree; an absolute path starts at the tree root and a relative one at the
//!   file's directory. `default` must name a label, an `fdt` must be a device
//!   tree, a `roothash=` in `append` must be 64 hex digits, and a kernel must
//!   be one U-Boot boots for the image's architecture.
//! - `boot.scr`: a legacy U-Boot image whose header and data CRCs hold.
//! - `*.itb` (at the tree root or under `boot/`): a FIT whose images' data
//!   lies in the file and matches each `crc32` and `sha256` hash, and whose
//!   configurations name images it has.
//!
//! A tree read from the boot tree snapshot holds only the small boot files;
//! its `.gaia-files` lists every file of the tree, so a kernel or initrd is
//! found there without its bytes.

use super::super::assembly::{BOOT_TREE_FILES_LIST as FILES_LIST, crc32};
use super::fdt::{self, Node};
use super::kernel_modules::Outcome;
use gaia_spec::TargetArch;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

const EXTLINUX: [&str; 2] = ["extlinux/extlinux.conf", "boot/extlinux/extlinux.conf"];
const SCRIPTS: [&str; 2] = ["boot.scr", "boot/boot.scr"];
const LEGACY_MAGIC: u32 = 0x2705_1956;
const LEGACY_HEADER: usize = 64;
const FDT_MAGIC: u32 = 0xd00d_feed;

/// A tree, and the files its snapshot says it has.
struct Tree {
    root: PathBuf,
    label: String,
    listed: BTreeSet<String>,
}

impl Tree {
    fn new(root: &Path, label: String) -> Self {
        let listed = std::fs::read_to_string(root.join(FILES_LIST))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default();
        Self {
            root: root.to_path_buf(),
            label,
            listed,
        }
    }

    fn is_file(&self, rel: &str) -> bool {
        self.root.join(rel).is_file() || self.listed.contains(rel)
    }

    fn is_dir(&self, rel: &str) -> bool {
        let prefix = format!("{}/", rel.trim_end_matches('/'));
        rel.is_empty()
            || self.root.join(rel).is_dir()
            || self.listed.iter().any(|file| file.starts_with(&prefix))
    }

    /// The file's bytes, when the tree (not just its list) has them.
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(rel)).ok()
    }
}

/// Checks the U-Boot boot files of every tree in `trees` (a tree and the
/// label its messages use). `arch` is the image's architecture, when known.
pub(super) fn check(
    build: &str,
    trees: &[(PathBuf, String)],
    arch: Option<TargetArch>,
    out: &mut Outcome,
) {
    for (root, label) in trees {
        let tree = Tree::new(root, label.clone());
        for conf in EXTLINUX {
            if let Some(text) = tree
                .read(conf)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            {
                check_extlinux(build, &tree, conf, &text, arch, out);
            }
        }
        for script in SCRIPTS {
            if let Some(bytes) = tree.read(script)
                && let Err(problem) = check_legacy(&bytes)
            {
                out.error(
                    "uboot_script_invalid",
                    None,
                    format!("{build}: {}/{script}: {problem}", tree.label),
                );
            }
        }
        for rel in fit_files(&tree) {
            let Some(bytes) = tree.read(&rel) else {
                continue;
            };
            for problem in check_fit(&bytes) {
                out.error(
                    "uboot_fit_invalid",
                    None,
                    format!("{build}: {}/{rel}: {problem}", tree.label),
                );
            }
        }
    }
}

/// The `*.itb` files at the tree root and under `boot/`.
fn fit_files(tree: &Tree) -> Vec<String> {
    let mut found = Vec::new();
    for dir in ["", "boot"] {
        let Ok(entries) = std::fs::read_dir(tree.root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".itb") && entry.path().is_file() {
                found.push(if dir.is_empty() {
                    name
                } else {
                    format!("{dir}/{name}")
                });
            }
        }
    }
    found.sort();
    found
}

/// One `label` of an extlinux.conf and what it names.
#[derive(Default)]
struct Label {
    name: String,
    line: usize,
    kernel: Option<String>,
    localboot: bool,
    files: Vec<(&'static str, String)>,
    dirs: Vec<(&'static str, String)>,
    append: Option<String>,
}

fn check_extlinux(
    build: &str,
    tree: &Tree,
    conf: &str,
    text: &str,
    arch: Option<TargetArch>,
    out: &mut Outcome,
) {
    let at = |line: usize| format!("{build}: {}/{conf}:{line}", tree.label);
    let mut default = None;
    let mut labels: Vec<Label> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, value) = line
            .split_once(char::is_whitespace)
            .map(|(keyword, value)| (keyword, value.trim()))
            .unwrap_or((line, ""));
        let keyword = keyword.to_ascii_lowercase();
        if keyword == "default" {
            default = Some((value.to_string(), index + 1));
            continue;
        }
        if keyword == "label" {
            labels.push(Label {
                name: value.to_string(),
                line: index + 1,
                ..Label::default()
            });
            continue;
        }
        let Some(label) = labels.last_mut() else {
            continue;
        };
        match keyword.as_str() {
            "kernel" | "linux" => label.kernel = Some(value.to_string()),
            "localboot" => label.localboot = true,
            "initrd" => label.files.extend(
                value
                    .split(',')
                    .filter(|file| !file.is_empty())
                    .map(|file| ("initrd", file.to_string())),
            ),
            "fdt" | "devicetree" => label.files.push(("fdt", value.to_string())),
            "fdtoverlays" => label.files.extend(
                value
                    .split_whitespace()
                    .map(|file| ("fdtoverlays", file.to_string())),
            ),
            "fdtdir" | "devicetreedir" => label.dirs.push(("fdtdir", value.to_string())),
            "append" => label.append = Some(value.to_string()),
            _ => {}
        }
    }
    let conf_dir = Path::new(conf).parent().unwrap_or(Path::new(""));
    if let Some((name, line)) = default
        && !labels.iter().any(|label| label.name == name)
    {
        out.error(
            "uboot_extlinux_default_missing",
            None,
            format!("{}: default '{name}' names no label", at(line)),
        );
    }
    for label in &labels {
        let missing = |what: &str, value: &str, out: &mut Outcome| {
            out.error(
                "uboot_extlinux_file_missing",
                None,
                format!(
                    "{}: label '{}' {what} '{value}' is not in the tree",
                    at(label.line),
                    label.name
                ),
            );
        };
        match &label.kernel {
            Some(kernel) => {
                let file = kernel.split('#').next().unwrap_or(kernel);
                match resolve(conf_dir, file) {
                    Some(rel) if tree.is_file(&rel) => {
                        if let Some(bytes) = tree.read(&rel)
                            && let Err(problem) = kernel_matches(&bytes, arch)
                        {
                            out.error(
                                "uboot_extlinux_kernel_invalid",
                                None,
                                format!(
                                    "{}: label '{}' kernel '{kernel}': {problem}",
                                    at(label.line),
                                    label.name
                                ),
                            );
                        }
                    }
                    _ => missing("kernel", kernel, out),
                }
            }
            None if !label.localboot => out.error(
                "uboot_extlinux_kernel_missing",
                None,
                format!("{}: label '{}' names no kernel", at(label.line), label.name),
            ),
            None => {}
        }
        for (what, value) in &label.files {
            match resolve(conf_dir, value) {
                Some(rel) if tree.is_file(&rel) => {
                    if *what == "fdt"
                        && let Some(bytes) = tree.read(&rel)
                        && let Err(problem) = fdt::parse(&bytes)
                    {
                        out.error(
                            "uboot_extlinux_fdt_invalid",
                            None,
                            format!(
                                "{}: label '{}' fdt '{value}': {problem}",
                                at(label.line),
                                label.name
                            ),
                        );
                    }
                }
                _ => missing(what, value, out),
            }
        }
        for (what, value) in &label.dirs {
            if !resolve(conf_dir, value).is_some_and(|rel| tree.is_dir(&rel)) {
                missing(what, value, out);
            }
        }
        if let Some(append) = &label.append {
            for hash in append
                .split_whitespace()
                .filter_map(|word| word.strip_prefix("roothash="))
            {
                if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    out.error(
                        "uboot_extlinux_roothash_invalid",
                        None,
                        format!(
                            "{}: label '{}' roothash '{hash}' is not 64 hex digits",
                            at(label.line),
                            label.name
                        ),
                    );
                }
            }
        }
    }
}

/// The path of `value` relative to the tree: from the root when absolute,
/// else from the config file's directory. `None` when it leaves the tree.
fn resolve(conf_dir: &Path, value: &str) -> Option<String> {
    let joined = match value.strip_prefix('/') {
        Some(absolute) => PathBuf::from(absolute),
        None => conf_dir.join(value),
    };
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => {}
        }
    }
    Some(parts.join("/"))
}

/// What U-Boot boots: an arm64 or riscv `Image`, an arm `zImage`, a FIT or
/// a legacy image, each maybe compressed. The architecture is checked where
/// the format names it.
fn kernel_matches(bytes: &[u8], arch: Option<TargetArch>) -> Result<(), String> {
    let le32 = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .map(|word| u32::from_le_bytes(word.try_into().expect("4 bytes")))
    };
    let named = if bytes.get(0x38..0x3c) == Some(b"ARM\x64") {
        Some(TargetArch::AArch64)
    } else if bytes.get(0x38..0x3c) == Some(b"RSC\x05") {
        Some(TargetArch::RiscV64)
    } else if le32(0x24) == Some(0x016f_2818) {
        Some(TargetArch::Arm)
    } else if be32(bytes, 0) == Some(FDT_MAGIC)
        || be32(bytes, 0) == Some(LEGACY_MAGIC)
        || bytes.starts_with(&[0x1f, 0x8b])
        || bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
        || bytes.starts_with(b"\xfd7zXZ")
    {
        None
    } else {
        return Err("not a kernel image U-Boot boots (no arm64 or riscv Image, zImage, FIT or legacy header)".into());
    };
    match (named, arch) {
        (Some(named), Some(arch)) if named != arch => Err(format!(
            "a {} kernel in a {} image",
            named.label(),
            arch.label()
        )),
        _ => Ok(()),
    }
}

fn be32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("4 bytes")))
}

/// A legacy U-Boot image (`mkimage -T script`): its magic, header CRC and
/// data CRC.
fn check_legacy(bytes: &[u8]) -> Result<(), String> {
    if be32(bytes, 0) != Some(LEGACY_MAGIC) {
        return Err("not a U-Boot image (bad magic); make it with mkimage -T script".into());
    }
    let header = bytes
        .get(..LEGACY_HEADER)
        .ok_or("shorter than a U-Boot image header")?;
    let mut zeroed = header.to_vec();
    zeroed[4..8].fill(0);
    if be32(header, 4) != Some(crc32(&zeroed)) {
        return Err("the header CRC does not match".into());
    }
    let size = be32(header, 12).unwrap_or(0) as usize;
    let data = bytes
        .get(LEGACY_HEADER..LEGACY_HEADER + size)
        .ok_or_else(|| format!("truncated: the header says {size} data bytes"))?;
    if be32(header, 24) != Some(crc32(data)) {
        return Err("the data CRC does not match: U-Boot refuses it".into());
    }
    Ok(())
}

/// A FIT's problems: data outside the file, hashes that do not match, and
/// configurations that name missing images.
fn check_fit(bytes: &[u8]) -> Vec<String> {
    let root = match fdt::parse(bytes) {
        Ok(root) => root,
        Err(problem) => return vec![problem],
    };
    // External data (`mkimage -E`) starts after the FDT, 4-byte aligned.
    let total = be32(bytes, 4).unwrap_or(0) as usize;
    let external_base = total.div_ceil(4) * 4;
    let mut problems = Vec::new();
    let child = |node: &'_ Node, name: &str| -> Option<Node> {
        node.children
            .iter()
            .find(|child| child.name == name)
            .cloned()
    };
    let images = child(&root, "images").unwrap_or_default();
    for image in &images.children {
        let data = if let Some(data) = image.prop("data") {
            Some(data)
        } else {
            let size = image.prop("data-size").and_then(|value| be32(value, 0));
            let start = image
                .prop("data-position")
                .and_then(|value| be32(value, 0))
                .map(|position| position as usize)
                .or_else(|| {
                    image
                        .prop("data-offset")
                        .and_then(|value| be32(value, 0))
                        .map(|offset| external_base + offset as usize)
                });
            match (start, size) {
                (Some(start), Some(size)) => {
                    let data = bytes.get(start..start + size as usize);
                    if data.is_none() {
                        problems.push(format!(
                            "image '{}' data ({size} bytes at {start}) lies outside the file",
                            image.name
                        ));
                    }
                    data
                }
                _ => {
                    problems.push(format!("image '{}' has no data", image.name));
                    None
                }
            }
        };
        let Some(data) = data else {
            continue;
        };
        for hash in image
            .children
            .iter()
            .filter(|child| child.name.starts_with("hash"))
        {
            let (Some(algo), Some(value)) = (hash.string("algo"), hash.prop("value")) else {
                continue;
            };
            let have: Option<Vec<u8>> = match algo.as_str() {
                "crc32" => Some(crc32(data).to_be_bytes().to_vec()),
                "sha256" => Some(Sha256::digest(data).to_vec()),
                _ => None,
            };
            if have.is_some_and(|have| have != value) {
                problems.push(format!(
                    "image '{}' {algo} does not match its data: U-Boot refuses it",
                    image.name
                ));
            }
        }
    }
    let names: BTreeSet<&str> = images
        .children
        .iter()
        .map(|image| image.name.as_str())
        .collect();
    let configurations = child(&root, "configurations").unwrap_or_default();
    if let Some(default) = configurations.string("default")
        && !configurations
            .children
            .iter()
            .any(|config| config.name == default)
    {
        problems.push(format!("default configuration '{default}' does not exist"));
    }
    for config in &configurations.children {
        for property in ["kernel", "fdt", "ramdisk", "firmware", "loadables"] {
            for name in config
                .prop(property)
                .map(fdt::strings_of)
                .unwrap_or_default()
            {
                if !names.contains(name.as_str()) {
                    problems.push(format!(
                        "configuration '{}' {property} '{name}' names no image",
                        config.name
                    ));
                }
            }
        }
    }
    problems
}

#[cfg(test)]
#[path = "uboot_tests.rs"]
mod tests;
