//! The kernel module checks of `image:check`: every shipped module against the
//! kernel it ships with. Reads only; the tree is never written.

use super::hardware::{self, ModuleView};
use super::kernel::{Symvers, kernel_release};
use super::module::{KernelModule, module_name_of, read_module};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    /// The module the finding is about: its path under the modules directory.
    pub module: Option<String>,
    pub message: String,
}

pub(super) struct Inputs<'a> {
    /// The shipped kernel image; its release is the one every module must match.
    pub kernel_image: &'a Path,
    /// `target/lib/modules`: the release's directory is searched under it.
    pub modules_root: &'a Path,
    /// `modules_dir`, when set, instead of `<modules_root>/<release>`.
    pub modules_dir: Option<&'a Path>,
    pub modules_dir_set: bool,
    /// `Module.symvers`, found or set. `None` skips the symbol checks.
    pub symvers: Option<&'a Path>,
    pub symvers_set: bool,
    /// `target/lib/firmware`, for the `firmware=` entries.
    pub firmware_dir: Option<&'a Path>,
    /// Whether the module checks run (`[image.checks] kernel_modules`). The
    /// hardware list runs either way.
    pub module_checks: bool,
    /// The hardware list, and the paths its links are read from.
    pub hardware: hardware::Sources<'a>,
}

#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub release: Option<String>,
    pub modules_dir: Option<PathBuf>,
    pub modules: usize,
    pub imports: usize,
    pub symvers_checked: bool,
    pub findings: Vec<Finding>,
}

impl Outcome {
    pub(super) fn error(&mut self, code: &'static str, module: Option<&str>, message: String) {
        self.push(Severity::Error, code, module, message);
    }

    pub(super) fn warning(&mut self, code: &'static str, module: Option<&str>, message: String) {
        self.push(Severity::Warning, code, module, message);
    }

    fn push(
        &mut self,
        severity: Severity,
        code: &'static str,
        module: Option<&str>,
        message: String,
    ) {
        self.findings.push(Finding {
            severity,
            code,
            module: module.map(str::to_string),
            message,
        });
    }
}

/// Runs every kernel module check and returns what it found.
pub(super) fn check(inputs: &Inputs<'_>) -> Outcome {
    let mut out = Outcome::default();
    let release = match kernel_release(inputs.kernel_image) {
        Ok(release) => release,
        Err(message) => {
            out.error(
                "kernel_release",
                None,
                format!("kernel image {}: {message}", inputs.kernel_image.display()),
            );
            return out;
        }
    };
    let dir = inputs
        .modules_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| inputs.modules_root.join(&release));
    out.release = Some(release.clone());
    out.modules_dir = Some(dir.clone());
    let dir_ready = dir.is_dir();
    if !dir_ready {
        let message = format!("no module directory {} for kernel {release}", dir.display());
        if inputs.modules_dir_set {
            out.error("modules_dir_missing", None, message);
        } else if inputs.module_checks {
            out.warning(
                "modules_dir_missing",
                None,
                format!("{message}: kernel modules are not checked"),
            );
        }
    }
    // With no module directory the hardware list still runs: every module it
    // names is then missing.
    let modules = if dir_ready {
        match read_modules(&dir) {
            Ok(modules) => modules,
            Err(message) => {
                out.error("modules_dir_unreadable", None, message);
                return out;
            }
        }
    } else {
        Vec::new()
    };
    // Every shipped file by name and path: the copies modprobe chooses between.
    let files: Vec<(String, String)> = modules
        .iter()
        .filter_map(|(_, rel_path)| Some((rel_name(rel_path)?, rel_path.clone())))
        .collect();
    let mut parsed = Vec::with_capacity(modules.len());
    for (path, rel_path) in modules {
        match read_module(&path, rel_path.clone()) {
            Ok(module) => parsed.push(module),
            Err(message) => out.error(
                "module_invalid",
                Some(rel_path.as_str()),
                format!("{rel_path} is not a readable kernel module: {message}"),
            ),
        }
    }
    out.modules = parsed.len();
    let builtin = if dir_ready {
        read_builtin(&dir)
    } else {
        HashSet::new()
    };
    let winners = modprobe_winners(&files, &dir);
    if inputs.module_checks {
        let shipped: HashSet<&str> = parsed.iter().map(|module| module.name.as_str()).collect();
        check_duplicates(&parsed, &winners, &mut out);
        check_vermagic(&parsed, &release, &mut out);
        check_depends(&parsed, &shipped, &builtin, &mut out);
        check_firmware(&parsed, inputs.firmware_dir, &mut out);

        let symvers = read_symvers(inputs, &mut out);
        if let Some(symvers) = &symvers {
            out.symvers_checked = true;
            let shipped_exports = exports_by_symbol(&parsed);
            for module in &parsed {
                out.imports += module.imports.len();
                check_imports(
                    module,
                    symvers,
                    &shipped,
                    &builtin,
                    &shipped_exports,
                    &mut out,
                );
                check_crcs(module, symvers, &mut out);
            }
        }
    }
    let module_errors = errors_by_module(&out.findings);
    let view = ModuleView {
        parsed: &parsed,
        winners: &winners,
        builtin: &builtin,
        dir: dir_ready.then_some(dir.as_path()),
        module_errors: &module_errors,
    };
    hardware::check(&inputs.hardware, &view, &mut out);
    out
}

/// The module name of a path under the modules directory (`updates/ov9282.ko`
/// is `ov9282`).
fn rel_name(rel_path: &str) -> Option<String> {
    rel_path.rsplit('/').next().and_then(module_name_of)
}

/// The copy modprobe loads for a shipped module name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Winner {
    /// The copy's path under the modules directory.
    pub rel: String,
    /// Whether `modules.dep` lists this copy (depmod's own choice).
    pub listed: bool,
}

/// The copy modprobe loads for each shipped name: the one `modules.dep` lists,
/// else the first by depmod's search order.
pub(super) fn modprobe_winners(files: &[(String, String)], dir: &Path) -> BTreeMap<String, Winner> {
    let listed_by_dep = read_dep_winners(dir);
    let mut copies_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, rel) in files {
        copies_of
            .entry(name.as_str())
            .or_default()
            .push(rel.as_str());
    }
    copies_of
        .into_iter()
        .map(|(name, copies)| {
            let listed = listed_by_dep
                .get(name)
                .filter(|rel| copies.contains(&rel.as_str()));
            let winner = match listed {
                Some(rel) => Winner {
                    rel: rel.clone(),
                    listed: true,
                },
                None => Winner {
                    rel: copies
                        .iter()
                        .min_by(|left, right| {
                            search_rank(left)
                                .cmp(&search_rank(right))
                                .then(left.cmp(right))
                        })
                        .map(|rel| rel.to_string())
                        .unwrap_or_default(),
                    listed: false,
                },
            };
            (name.to_string(), winner)
        })
        .collect()
}

/// The error codes the findings name, for each module path.
pub(super) fn errors_by_module(findings: &[Finding]) -> HashMap<String, Vec<&'static str>> {
    let mut by_module: HashMap<String, Vec<&'static str>> = HashMap::new();
    for finding in findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
    {
        if let Some(module) = &finding.module {
            let codes = by_module.entry(module.clone()).or_default();
            if !codes.contains(&finding.code) {
                codes.push(finding.code);
            }
        }
    }
    by_module
}

/// Every `.ko`, `.ko.xz`, `.ko.zst` and `.ko.gz` under `dir`, as its path and
/// its path relative to `dir`, sorted by the relative path.
fn read_modules(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|error| format!("cannot list {}: {error}", current.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot list {}: {error}", current.display()))?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let is_module = entry
                .file_name()
                .to_str()
                .is_some_and(|name| module_name_of(name).is_some());
            if is_module {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                found.push((path, rel));
            }
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// The module names `modules.builtin` lists: built into the kernel, so they
/// need no file. Empty when there is no such file.
fn read_builtin(dir: &Path) -> HashSet<String> {
    std::fs::read_to_string(dir.join("modules.builtin"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| module_name_of(line.trim().rsplit('/').next()?))
        .collect()
}

/// The first `modules.dep` entry for each module name: depmod writes one line
/// per module it lists, under the path modprobe loads.
fn read_dep_winners(dir: &Path) -> HashMap<String, String> {
    let mut winners = HashMap::new();
    let text = std::fs::read_to_string(dir.join("modules.dep")).unwrap_or_default();
    for line in text.lines() {
        let Some((rel, _)) = line.split_once(':') else {
            continue;
        };
        let rel = rel.trim();
        if let Some(name) = rel.rsplit('/').next().and_then(module_name_of) {
            winners.entry(name).or_insert_with(|| rel.to_string());
        }
    }
    winners
}

/// Where a module shipped under `rel` ranks for modprobe when another copy
/// of the same name exists: depmod searches `updates` first, then `extra`,
/// then `weak-updates`, then the kernel tree.
fn search_rank(rel: &str) -> u8 {
    match rel.split('/').next().unwrap_or(rel) {
        "updates" => 0,
        "extra" => 1,
        "weak-updates" => 2,
        _ => 3,
    }
}

fn check_duplicates(
    modules: &[KernelModule],
    winners: &BTreeMap<String, Winner>,
    out: &mut Outcome,
) {
    let mut by_name: BTreeMap<&str, Vec<&KernelModule>> = BTreeMap::new();
    for module in modules {
        by_name
            .entry(module.name.as_str())
            .or_default()
            .push(module);
    }
    for (name, copies) in by_name {
        if copies.len() < 2 {
            continue;
        }
        let Some(winner) = winners.get(name) else {
            continue;
        };
        let reason = if winner.listed {
            "as modules.dep lists it"
        } else {
            "by depmod's search order"
        };
        let shadowed: Vec<&str> = copies
            .iter()
            .map(|copy| copy.rel_path.as_str())
            .filter(|rel| *rel != winner.rel)
            .collect();
        out.warning(
            "module_duplicate",
            Some(winner.rel.as_str()),
            format!(
                "module {name} ships {} times: modprobe loads {} ({reason}); shadowed: {}",
                copies.len(),
                winner.rel,
                shadowed.join(", ")
            ),
        );
    }
}

fn check_vermagic(modules: &[KernelModule], release: &str, out: &mut Outcome) {
    let mut flags_of: Vec<(&KernelModule, &str)> = Vec::new();
    for module in modules {
        let Some(vermagic) = &module.vermagic else {
            out.error(
                "vermagic_missing",
                Some(module.rel_path.as_str()),
                format!("{} has no vermagic", module.rel_path),
            );
            continue;
        };
        match vermagic.strip_prefix(release) {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
                flags_of.push((module, rest.trim()));
            }
            _ => out.error(
                "vermagic_mismatch",
                Some(module.rel_path.as_str()),
                format!(
                    "{} has vermagic '{vermagic}', but the kernel is {release}",
                    module.rel_path
                ),
            ),
        }
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, flags) in &flags_of {
        *counts.entry(*flags).or_default() += 1;
    }
    // The most common flags win; a tie goes to the smaller text.
    let majority = counts
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(flags, _)| *flags);
    let Some(majority) = majority else {
        return;
    };
    for (module, flags) in flags_of {
        if flags != majority {
            out.error(
                "vermagic_flags_differ",
                Some(module.rel_path.as_str()),
                format!(
                    "{} has vermagic flags '{flags}', but most modules have '{majority}'",
                    module.rel_path
                ),
            );
        }
    }
}

fn check_depends(
    modules: &[KernelModule],
    shipped: &HashSet<&str>,
    builtin: &HashSet<String>,
    out: &mut Outcome,
) {
    for module in modules {
        for dependency in &module.depends {
            if !shipped.contains(dependency.as_str()) && !builtin.contains(dependency) {
                out.error(
                    "dependency_missing",
                    Some(module.rel_path.as_str()),
                    format!(
                        "{} depends on {dependency}, which is neither shipped nor built in",
                        module.rel_path
                    ),
                );
            }
        }
    }
}

fn check_firmware(modules: &[KernelModule], firmware_dir: Option<&Path>, out: &mut Outcome) {
    let Some(firmware_dir) = firmware_dir else {
        return;
    };
    for module in modules {
        for firmware in &module.firmware {
            let present = [String::new(), ".xz".into(), ".zst".into(), ".gz".into()]
                .iter()
                .any(|suffix| firmware_dir.join(format!("{firmware}{suffix}")).is_file());
            if !present {
                out.warning(
                    "firmware_missing",
                    Some(module.rel_path.as_str()),
                    format!(
                        "{} asks for firmware {firmware}, which is not under lib/firmware",
                        module.rel_path
                    ),
                );
            }
        }
    }
}

fn read_symvers(inputs: &Inputs<'_>, out: &mut Outcome) -> Option<Symvers> {
    let Some(path) = inputs.symvers else {
        out.warning(
            "symvers_missing",
            None,
            "no Module.symvers: kernel exports are not checked".into(),
        );
        return None;
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Some(Symvers::parse(&text)),
        Err(error) => {
            let message = format!("cannot read {}: {error}", path.display());
            if inputs.symvers_set {
                out.error("symvers_unreadable", None, message);
            } else {
                out.warning(
                    "symvers_unreadable",
                    None,
                    format!("{message}: kernel exports are not checked"),
                );
            }
            None
        }
    }
}

/// The module that exports each symbol, over every shipped module.
fn exports_by_symbol(modules: &[KernelModule]) -> HashMap<&str, &str> {
    let mut exports = HashMap::new();
    for module in modules {
        for symbol in &module.exports {
            exports
                .entry(symbol.as_str())
                .or_insert(module.name.as_str());
        }
    }
    exports
}

/// Every import resolves to the kernel (`Module.symvers`) or to a shipped
/// module. A kernel export by a module that is neither shipped nor built in
/// is an error, since that module will not be there to provide it.
fn check_imports(
    module: &KernelModule,
    symvers: &Symvers,
    shipped: &HashSet<&str>,
    builtin: &HashSet<String>,
    shipped_exports: &HashMap<&str, &str>,
    out: &mut Outcome,
) {
    for symbol in &module.imports {
        match symvers.get(symbol) {
            Some(export) => {
                let provided = export.module == "vmlinux"
                    || shipped.contains(export.module.as_str())
                    || builtin.contains(&export.module);
                if !provided {
                    out.error(
                        "export_not_shipped",
                        Some(module.rel_path.as_str()),
                        format!(
                            "{} imports {symbol}, which module {} exports, but that module is neither shipped nor built in",
                            module.rel_path, export.module
                        ),
                    );
                }
            }
            None if shipped_exports.contains_key(symbol.as_str()) => {}
            None => out.error(
                "undefined_symbol",
                Some(module.rel_path.as_str()),
                format!(
                    "{} imports {symbol}, which neither the kernel nor a shipped module exports",
                    module.rel_path
                ),
            ),
        }
    }
}

/// With `CONFIG_MODVERSIONS`, each import records the CRC it was built
/// against. A different CRC in the kernel is an ABI mismatch: the module
/// loads on one kernel and is refused by another.
fn check_crcs(module: &KernelModule, symvers: &Symvers, out: &mut Outcome) {
    for (symbol, crc) in &module.versions {
        let Some(export) = symvers.get(symbol) else {
            continue;
        };
        if *crc != u64::from(export.crc) {
            out.error(
                "abi_mismatch",
                Some(module.rel_path.as_str()),
                format!(
                    "{} was built against {symbol} with CRC {crc:#010x}, but {} exports it with CRC {:#010x} (ABI mismatch)",
                    module.rel_path, export.module, export.crc
                ),
            );
        }
    }
}
