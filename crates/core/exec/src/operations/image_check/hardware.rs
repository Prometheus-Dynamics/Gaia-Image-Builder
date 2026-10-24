//! The hardware list of `image:check` (`[[image.hardware]]`): what each device
//! needs in the image, and whether its chain holds. Every link is checked
//! against what the image ships: a module (shipped and passing the module
//! checks, or built in) claims each compatible; the compatible is named by an
//! overlay of the entry or by a shipped board DTB; each overlay ships in a boot
//! overlays directory; the files and firmware are in the rootfs. Reads only.

use super::boot::{self, OverlayDir};
use super::fdt::{self, Fragment};
use super::kernel_modules::{Outcome, Winner};
use super::module::{KernelModule, normalise_name};
use gaia_spec::ImageHardwareSpec;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Where the hardware list is read from, and the build it names in errors.
pub(super) struct Sources<'a> {
    pub build: &'a str,
    pub entries: &'a [ImageHardwareSpec],
    /// Buildroot's `images/`: the board DTBs, and `rpi-firmware/`, a boot tree.
    pub images_dir: &'a Path,
    /// Buildroot's `target/`: the rootfs that `files` are read from.
    pub rootfs: &'a Path,
    /// Each assembly tree's directory (the disk path the run leaves it at):
    /// its `overlays/`, its top-level `config.txt` and `*.dtb` are read too.
    pub assembly_trees: &'a [PathBuf],
    /// Rootfs paths (relative to `target/`) that the final rootfs does not
    /// ship: the files of the excluded packages (`image.rootfs.exclude_packages`).
    pub removed: &'a HashSet<String>,
    /// The Raspberry Pi model whose boot-config sections are evaluated
    /// (`[image.checks] pi_model`); unset, every section counts.
    pub pi_model: Option<&'a str>,
}

/// The module tree as the module checks left it.
pub(super) struct ModuleView<'a> {
    /// The shipped modules that could be read.
    pub parsed: &'a [KernelModule],
    /// The copy modprobe loads for each shipped name.
    pub winners: &'a BTreeMap<String, Winner>,
    /// The names built into the kernel (`modules.builtin`).
    pub builtin: &'a HashSet<String>,
    /// The modules directory, for `modules.alias` and `modules.builtin.modinfo`.
    pub dir: Option<&'a Path>,
    /// The error codes the module checks found on each module, by path.
    pub module_errors: &'a HashMap<String, Vec<&'static str>>,
}

/// One driver's alias (`of:N*T*C<compatible>`), and the module that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Claim {
    by: String,
    alias: String,
}

/// A shipped board DTB and the compatibles its nodes name.
struct Board {
    file: String,
    compatibles: Vec<String>,
}

/// What each entry is checked against, shared by every entry.
struct Cx<'a> {
    build: &'a str,
    rootfs: &'a Path,
    removed: &'a HashSet<String>,
    view: &'a ModuleView<'a>,
    builtin_aliases: &'a HashMap<String, Vec<String>>,
    modules_alias: &'a [(String, String)],
    /// Every alias of every shipped and built-in driver.
    claims: &'a [Claim],
    /// The overlays directory each boot tree's normal boot path reads.
    overlay_dirs: &'a [OverlayDir],
    boards: &'a [Board],
}

/// Checks every entry and the `dtoverlay=` lines of the shipped `config.txt`.
pub(super) fn check(sources: &Sources<'_>, view: &ModuleView<'_>, out: &mut Outcome) {
    let builtin_aliases: HashMap<String, Vec<String>> =
        view.dir.map(read_builtin_aliases).unwrap_or_default();
    let modules_alias: Vec<(String, String)> = view.dir.map(read_modules_alias).unwrap_or_default();
    let mut claims = Vec::new();
    for module in view.parsed {
        claims.extend(module.aliases.iter().map(|alias| Claim {
            by: module.name.clone(),
            alias: alias.clone(),
        }));
    }
    for (module, aliases) in &builtin_aliases {
        claims.extend(aliases.iter().map(|alias| Claim {
            by: module.clone(),
            alias: alias.clone(),
        }));
    }
    claims.extend(modules_alias.iter().map(|(alias, module)| Claim {
        by: module.clone(),
        alias: alias.clone(),
    }));
    // The boot trees: Buildroot's rpi-firmware, then each assembly tree. Each
    // one with a config.txt is read as the firmware reads it.
    let mut boot_trees = vec![sources.images_dir.join("rpi-firmware")];
    boot_trees.extend(sources.assembly_trees.iter().cloned());
    let overlay_dirs = boot::check(sources.build, &boot_trees, sources.pi_model, out);
    if sources.entries.is_empty() {
        return;
    }
    let boards = if sources
        .entries
        .iter()
        .any(|entry| !entry.compatible.is_empty())
    {
        let mut board_dirs = vec![sources.images_dir];
        board_dirs.extend(sources.assembly_trees.iter().map(PathBuf::as_path));
        let (boards, problems) = read_boards(&board_dirs);
        for problem in problems {
            out.error(
                "hardware_board_unreadable",
                None,
                format!("{}: {problem}", sources.build),
            );
        }
        boards
    } else {
        Vec::new()
    };
    let cx = Cx {
        build: sources.build,
        rootfs: sources.rootfs,
        removed: sources.removed,
        view,
        builtin_aliases: &builtin_aliases,
        modules_alias: &modules_alias,
        claims: &claims,
        overlay_dirs: &overlay_dirs,
        boards: &boards,
    };
    for entry in sources.entries {
        check_entry(entry, &cx, out);
    }
}

fn check_entry(entry: &ImageHardwareSpec, cx: &Cx<'_>, out: &mut Outcome) {
    let who = format!("{}: {}", cx.build, entry.id);
    let mut fragments: Vec<(String, Fragment)> = Vec::new();
    for overlay in &entry.overlays {
        let file = format!("{overlay}.dtbo");
        match find_overlay(cx.overlay_dirs, &file) {
            None => out.error(
                "hardware_overlay_missing",
                None,
                format!(
                    "{who}: overlay {file} is in no boot overlays dir ({})",
                    dirs_text(cx.overlay_dirs)
                ),
            ),
            Some(path) => match read_overlay(&path) {
                Ok(found) => fragments.extend(
                    found
                        .into_iter()
                        .map(|fragment| (overlay.clone(), fragment)),
                ),
                Err(error) => out.error(
                    "hardware_overlay_unreadable",
                    None,
                    format!("{who}: overlay {file} is not a readable device tree overlay: {error}"),
                ),
            },
        }
    }
    let named = check_modules(entry, &who, cx, out);
    for compatible in &entry.compatible {
        check_compatible(compatible, entry, &who, &named, &fragments, cx, out);
    }
    // The final rootfs: `target/` without the files of the excluded packages.
    for file in &entry.files {
        let rel = file.trim_start_matches('/');
        if !cx.rootfs.join(rel).exists() || cx.removed.contains(rel) {
            out.error(
                "hardware_file_missing",
                None,
                format!("{who}: file {file} is missing from the rootfs"),
            );
        }
    }
    for firmware in &entry.firmware {
        let present = ["", ".xz", ".zst", ".gz"].iter().any(|suffix| {
            let rel = format!("lib/firmware/{firmware}{suffix}");
            cx.rootfs.join(&rel).is_file() && !cx.removed.contains(&rel)
        });
        if !present {
            out.error(
                "hardware_firmware_missing",
                None,
                format!("{who}: firmware {firmware} is missing from lib/firmware"),
            );
        }
    }
}

/// Each named module must be built in, or be the copy modprobe loads and pass
/// the module checks. Returns the aliases the named modules carry.
fn check_modules(
    entry: &ImageHardwareSpec,
    who: &str,
    cx: &Cx<'_>,
    out: &mut Outcome,
) -> Vec<Claim> {
    let mut claims = Vec::new();
    for module in &entry.modules {
        let name = normalise_name(module);
        if cx.view.builtin.contains(&name) {
            claims.extend(
                cx.builtin_aliases
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .map(|alias| Claim {
                        by: name.clone(),
                        alias: alias.clone(),
                    }),
            );
            continue;
        }
        let Some(winner) = cx.view.winners.get(&name) else {
            out.error(
                "hardware_module_missing",
                None,
                format!("{who}: module {module} is neither shipped nor built in"),
            );
            continue;
        };
        if let Some(codes) = cx.view.module_errors.get(&winner.rel) {
            out.error(
                "hardware_module_faulty",
                Some(winner.rel.as_str()),
                format!(
                    "{who}: module {module} ({}) fails its kernel checks: {}",
                    winner.rel,
                    codes.join(", ")
                ),
            );
        }
        match cx
            .view
            .parsed
            .iter()
            .find(|parsed| parsed.rel_path == winner.rel)
        {
            Some(parsed) => {
                claims.extend(parsed.aliases.iter().map(|alias| Claim {
                    by: name.clone(),
                    alias: alias.clone(),
                }));
                claims.extend(
                    cx.modules_alias
                        .iter()
                        .filter(|(_, target)| *target == name)
                        .map(|(alias, _)| Claim {
                            by: name.clone(),
                            alias: alias.clone(),
                        }),
                );
            }
            None => out.error(
                "hardware_module_unreadable",
                Some(winner.rel.as_str()),
                format!(
                    "{who}: module {module} ({}) cannot be read (see module_invalid)",
                    winner.rel
                ),
            ),
        }
    }
    claims
}

fn check_compatible(
    compatible: &str,
    entry: &ImageHardwareSpec,
    who: &str,
    named: &[Claim],
    fragments: &[(String, Fragment)],
    cx: &Cx<'_>,
    out: &mut Outcome,
) {
    let anywhere: Vec<&Claim> = cx
        .claims
        .iter()
        .filter(|claim| alias_claims(&claim.alias, compatible))
        .collect();
    let unclaimed =
        format!("{who}: compatible {compatible} is claimed by no shipped or built-in driver");
    if entry.modules.is_empty() {
        if anywhere.is_empty() {
            out.error("hardware_compatible_unclaimed", None, unclaimed);
        }
    } else if !named
        .iter()
        .any(|claim| alias_claims(&claim.alias, compatible))
    {
        if anywhere.is_empty() {
            out.error("hardware_compatible_unclaimed", None, unclaimed);
        } else {
            let claimers: BTreeSet<&str> = anywhere.iter().map(|claim| claim.by.as_str()).collect();
            out.error(
                "hardware_compatible_unclaimed",
                None,
                format!(
                    "{who}: compatible {compatible} is not claimed by {} (the module modprobe loads); {} claim it",
                    entry.modules.join(", "),
                    claimers.into_iter().collect::<Vec<_>>().join(", ")
                ),
            );
        }
    }
    let mut named_by: Vec<String> = fragments
        .iter()
        .filter(|(_, fragment)| fragment.compatibles.iter().any(|c| c == compatible))
        .map(|(overlay, fragment)| format!("overlay {overlay}.dtbo, {}", fragment.name))
        .collect();
    named_by.extend(
        cx.boards
            .iter()
            .filter(|board| board.compatibles.iter().any(|c| c == compatible))
            .map(|board| format!("board {}", board.file)),
    );
    if named_by.is_empty() {
        out.error(
            "hardware_compatible_unnamed",
            None,
            format!(
                "{who}: compatible {compatible} is named by no overlay of this entry and no shipped board dtb"
            ),
        );
    }
}

/// Whether a modules alias matches a compatible: the device-table form
/// `of:N*T*C<compatible>`, with or without the trailing wildcard depmod writes.
fn alias_claims(alias: &str, compatible: &str) -> bool {
    let base = format!("of:N*T*C{compatible}");
    alias == base || alias == format!("{base}C*") || alias == format!("{base}*")
}

/// The first boot overlays directory (a tree's normal boot path) with the file.
fn find_overlay(dirs: &[OverlayDir], file: &str) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| boot::find_file(&dir.dir, file))
}

/// The overlays directories, for a message: `<tree>: <dir>`, or that there are none.
fn dirs_text(dirs: &[OverlayDir]) -> String {
    if dirs.is_empty() {
        return "there is no boot config.txt".into();
    }
    dirs.iter()
        .map(|dir| dir.label.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_overlay(path: &Path) -> Result<Vec<Fragment>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read: {error}"))?;
    Ok(fdt::overlay_fragments(&fdt::parse(&bytes)?))
}

/// The shipped board DTBs (the top-level `*.dtb` of each directory: Buildroot's
/// `images/`, and each assembly tree), and the ones that do not parse.
fn read_boards(dirs: &[&Path]) -> (Vec<Board>, Vec<String>) {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.is_file()
                            && path.extension().and_then(|ext| ext.to_str()) == Some("dtb")
                    })
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        files.extend(found);
    }
    let mut boards = Vec::new();
    let mut problems = Vec::new();
    for path in files {
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let parsed = std::fs::read(&path)
            .map_err(|error| format!("cannot read: {error}"))
            .and_then(|bytes| fdt::parse(&bytes));
        match parsed {
            Ok(root) => boards.push(Board {
                file,
                compatibles: root.compatibles(),
            }),
            Err(error) => problems.push(format!(
                "board {file} is not a readable device tree: {error}"
            )),
        }
    }
    (boards, problems)
}

/// The rootfs paths (relative to `target/`) that leaving the `excluded`
/// packages out of the rootfs removes: the paths `build/packages-file-list.txt`
/// lists for an excluded package, unless a kept package lists the same path.
/// This is the removal the Buildroot provider applies to `target/`.
pub(super) fn rootfs_exclusions(buildroot: &Path, excluded: &[String]) -> HashSet<String> {
    if excluded.is_empty() {
        return HashSet::new();
    }
    let Ok(text) = std::fs::read_to_string(buildroot.join("build/packages-file-list.txt")) else {
        return HashSet::new();
    };
    let mut from_excluded = HashSet::new();
    let mut kept = HashSet::new();
    for line in text.lines() {
        // `package,path`, the path relative to the root (`./usr/...`).
        let Some((package, path)) = line.split_once(',') else {
            continue;
        };
        let path = path.strip_prefix("./").unwrap_or(path).to_string();
        if excluded.iter().any(|name| name == package) {
            from_excluded.insert(path);
        } else {
            kept.insert(path);
        }
    }
    from_excluded.retain(|path| !kept.contains(path));
    from_excluded
}

/// The aliases a built-in module carries: `<name>.alias=<alias>` entries of
/// `modules.builtin.modinfo`, NUL-separated.
fn read_builtin_aliases(dir: &Path) -> HashMap<String, Vec<String>> {
    let bytes = std::fs::read(dir.join("modules.builtin.modinfo")).unwrap_or_default();
    let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
    for entry in bytes.split(|byte| *byte == 0) {
        let Ok(text) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        let Some((module, field)) = key.split_once('.') else {
            continue;
        };
        if field == "alias" {
            aliases
                .entry(normalise_name(module))
                .or_default()
                .push(value.to_string());
        }
    }
    aliases
}

/// `modules.alias` as (alias pattern, module name) pairs.
fn read_modules_alias(dir: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join("modules.alias"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next() != Some("alias") {
                return None;
            }
            let pattern = parts.next()?;
            let module = parts.next()?;
            Some((pattern.to_string(), normalise_name(module)))
        })
        .collect()
}
