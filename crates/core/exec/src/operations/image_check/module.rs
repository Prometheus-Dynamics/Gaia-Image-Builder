//! One shipped kernel module: its bytes (decompressed with the external tool
//! its extension names), and what the kernel checks read from it.

use super::elf::{ElfObject, SHN_UNDEF, STB_WEAK, STT_FILE, STT_SECTION};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};

/// `struct modversion_info` on 64-bit: a CRC word, then a 56-byte name.
const MODVERSION_SIZE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KernelModule {
    /// The path under the modules directory, such as `updates/ov9282.ko`.
    pub rel_path: String,
    /// The module name modprobe matches: the file name without its suffixes,
    /// with `-` read as `_`.
    pub name: String,
    pub vermagic: Option<String>,
    /// `depends=`, normalised as [`normalise_name`] does.
    pub depends: Vec<String>,
    /// Every `firmware=` value.
    pub firmware: Vec<String>,
    /// Every `alias=` value: the device-table aliases (`of:N*T*C<compatible>`).
    pub aliases: Vec<String>,
    /// The `__versions` entries: each imported symbol with the CRC the module
    /// was built against.
    pub versions: Vec<(String, u64)>,
    /// Undefined, non-weak symbols: what the module imports.
    pub imports: Vec<String>,
    /// The names in `__ksymtab_strings`: what the module exports.
    pub exports: Vec<String>,
}

/// The name modprobe compares: `-` and `_` are the same character.
pub(super) fn normalise_name(name: &str) -> String {
    name.replace('-', "_")
}

/// The module name of a file name such as `ov9282.ko.xz`, or `None` when it
/// is not a module.
pub(super) fn module_name_of(file_name: &str) -> Option<String> {
    let stem = ["ko.xz", "ko.zst", "ko.gz", "ko"]
        .iter()
        .find_map(|suffix| file_name.strip_suffix(&format!(".{suffix}")))?;
    Some(normalise_name(stem))
}

/// Reads and parses one module. A compressed module is decompressed with
/// the tool its suffix names (`xz`, `zstd`, `gzip`), run from `PATH`.
pub(super) fn read_module(path: &Path, rel_path: String) -> Result<KernelModule, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let name = module_name_of(file_name).ok_or("not a kernel module file name")?;
    let bytes = read_bytes(path)?;
    parse_module(&bytes, rel_path, name)
}

/// The bytes of a file, decompressed when its suffix says so.
pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let tool = match path.extension().and_then(|extension| extension.to_str()) {
        Some("xz") => "xz",
        Some("zst") => "zstd",
        Some("gz") => "gzip",
        _ => return std::fs::read(path).map_err(|error| format!("cannot read: {error}")),
    };
    decompress(tool, path)
}

/// The decompressed bytes of `path`, from `tool -dc <path>` run via `PATH`.
pub(super) fn decompress(tool: &str, path: &Path) -> Result<Vec<u8>, String> {
    let output = Command::new(tool)
        .arg("-dc")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {tool} to decompress: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{tool} could not decompress it: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Parses a module's bytes. `name` is the module name from its file name.
pub(super) fn parse_module(
    bytes: &[u8],
    rel_path: String,
    name: String,
) -> Result<KernelModule, String> {
    let elf = ElfObject::parse(bytes)?;
    let mut vermagic = None;
    let mut depends = Vec::new();
    let mut firmware = Vec::new();
    let mut aliases = Vec::new();
    for entry in elf
        .section(".modinfo")
        .unwrap_or_default()
        .split(|byte| *byte == 0)
    {
        let Ok(text) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        match key {
            "vermagic" if vermagic.is_none() => vermagic = Some(value.to_string()),
            "depends" => depends.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|dependency| !dependency.is_empty())
                    .map(normalise_name),
            ),
            "firmware" => firmware.push(value.to_string()),
            "alias" => aliases.push(value.to_string()),
            _ => {}
        }
    }
    let versions = elf
        .section("__versions")
        .unwrap_or_default()
        .as_chunks::<MODVERSION_SIZE>()
        .0
        .iter()
        .filter_map(|entry| {
            let crc = u64::from_le_bytes(entry[..8].try_into().ok()?);
            let end = entry[8..].iter().position(|byte| *byte == 0).unwrap_or(56);
            let symbol = std::str::from_utf8(&entry[8..8 + end]).ok()?;
            Some((symbol.to_string(), crc))
        })
        .collect();
    let mut imports = BTreeSet::new();
    for symbol in elf.symbols()? {
        let defined_elsewhere = symbol.shndx != SHN_UNDEF;
        if defined_elsewhere
            || symbol.bind == STB_WEAK
            || symbol.kind == STT_SECTION
            || symbol.kind == STT_FILE
            || symbol.name.is_empty()
        {
            continue;
        }
        imports.insert(symbol.name.to_string());
    }
    let exports = elf
        .section("__ksymtab_strings")
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .filter_map(|text| std::str::from_utf8(text).ok())
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect();
    Ok(KernelModule {
        rel_path,
        name,
        vermagic,
        depends,
        firmware,
        aliases,
        versions,
        imports: imports.into_iter().collect(),
        exports,
    })
}
