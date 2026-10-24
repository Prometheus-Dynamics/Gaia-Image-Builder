//! The Raspberry Pi firmware's boot model, for every boot tree of the image
//! (Buildroot's `images/rpi-firmware` and each assembly tree with a top-level
//! `config.txt`). The firmware reads `config.txt` on a normal boot and
//! `tryboot.txt` on a trial boot; each path is read on its own. Reads only.
//!
//! What is read, and how (the same rules as Atlas's
//! `devices/raze/gaia/scripts/check-boot-overlays.sh`):
//! - `#` starts a comment; CRs and whitespace are dropped; file names compare
//!   without regard to case (a FAT volume's names).
//! - `include <file>` reads `<file>` relative to the tree root, as if its text
//!   were in place of the line (a section filter carries into it). A missing
//!   include and an include cycle are errors.
//! - Section filters (`[all]`, `[cm5]`, ...) are not evaluated by default: every
//!   `dtoverlay=` counts, whatever its section, which is stricter than the
//!   firmware and right for a must-exist check. `[image.checks] pi_model`
//!   turns evaluation on: `[all]` and `[none]` as written, `[tryboot]` only on
//!   the trial path, a model filter (`pi*`, `cm*`) only when it names that
//!   model. An unknown filter applies, with a note.
//! - `os_prefix=` is the last value on the path (it may be empty).
//!   `overlay_prefix=` (default `overlays/`) is read the same way.
//! - `dtoverlay=<name>[,params]` (the name ends at the first comma or colon;
//!   empty names are skipped) needs `<name>.dtbo` in the directory the firmware
//!   reads. That directory is `${os_prefix}${overlay_prefix}` when its README
//!   exists (any content), else `${overlay_prefix}` at the tree root.
//!
//! Errors on top of the overlay lookups:
//! - a `${os_prefix}${overlay_prefix}` directory that exists without a README
//!   (the firmware ignores it, so its overlays do not boot);
//! - on `config.txt`, when the slot directory is the one read, a `.dtbo` in it
//!   that the root `overlays/` (the fallback copy) lacks.
//!
//! Not handled: `autoboot.txt`, `boot_partition` and `upstream_kernel`.

use super::kernel_modules::Outcome;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const DEFAULT_OVERLAY_PREFIX: &str = "overlays/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootPath {
    /// `config.txt`, a normal boot.
    Normal,
    /// `tryboot.txt`, a trial boot (`reboot '0 tryboot'`).
    Trial,
}

impl BootPath {
    fn file(self) -> &'static str {
        match self {
            BootPath::Normal => "config.txt",
            BootPath::Trial => "tryboot.txt",
        }
    }
}

/// The overlays directory a tree's normal boot path reads.
#[derive(Debug, Clone)]
pub(super) struct OverlayDir {
    pub dir: PathBuf,
    /// `<tree>: <directory relative to the tree>`, for messages.
    pub label: String,
}

/// Checks every boot tree in `trees` (those with a top-level `config.txt`),
/// and returns the overlays directory each tree's normal boot path reads.
pub(super) fn check(
    build: &str,
    trees: &[PathBuf],
    pi_model: Option<&str>,
    out: &mut Outcome,
) -> Vec<OverlayDir> {
    let mut normal = Vec::new();
    for tree in trees {
        if !has_file(tree, BootPath::Normal.file()) {
            continue;
        }
        let label = tree_label(tree);
        for path in [BootPath::Normal, BootPath::Trial] {
            let file = path.file();
            if !has_file(tree, file) {
                continue;
            }
            let mut walk = Walk::new(tree, path, pi_model);
            walk.read(file);
            for (code, message) in &walk.problems {
                out.error(code, None, format!("{build}: {label}/{file}: {message}"));
            }
            for note in &walk.notes {
                out.warning(
                    "boot_filter_note",
                    None,
                    format!("{build}: {label}/{file}: {note}"),
                );
            }
            let os_prefix = walk.os_prefix.clone().unwrap_or_default();
            let overlay_prefix = walk
                .overlay_prefix
                .clone()
                .unwrap_or_else(|| DEFAULT_OVERLAY_PREFIX.to_string());
            let layout = Layout::new(tree, &os_prefix, &overlay_prefix);
            check_layout(build, &label, file, path, &layout, &walk.overlays, out);
            if path == BootPath::Normal {
                normal.push(OverlayDir {
                    dir: layout.read.clone(),
                    label: format!("{label}: {}", layout.read_rel),
                });
            }
        }
    }
    normal
}

/// Whether `name` (in `dir`, any case) is a file.
pub(super) fn has_file(dir: &Path, name: &str) -> bool {
    find_file(dir, name).is_some()
}

/// The file `name` in `dir`, compared without regard to case: the path as
/// written when it exists, else the directory entry that matches it.
pub(super) fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    locate(dir, name).filter(|path| path.is_file())
}

/// `dir/rel` with each component matched without regard to case, when the
/// path exists in some spelling; else `dir/rel` as written.
fn locate(dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for part in rel.split('/').filter(|part| !part.is_empty()) {
        let exact = current.join(part);
        if exact.exists() {
            current = exact;
            continue;
        }
        let wanted = part.to_ascii_lowercase();
        let entry = std::fs::read_dir(&current)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .find(|name| name.to_string_lossy().to_ascii_lowercase() == wanted)?;
        current = current.join(entry);
    }
    Some(current)
}

/// The tree's directory for a relative path, as the firmware resolves it.
fn dir_at(tree: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        return tree.to_path_buf();
    }
    locate(tree, rel).unwrap_or_else(|| tree.join(rel))
}

fn has_readme(dir: &Path) -> bool {
    has_file(dir, "README")
}

/// The last two components of a tree's path (`slots/fat`, `images/rpi-firmware`).
fn tree_label(tree: &Path) -> String {
    let mut parts: Vec<String> = tree
        .components()
        .rev()
        .take(2)
        .filter_map(|part| part.as_os_str().to_str().map(str::to_string))
        .collect();
    parts.reverse();
    parts.join("/")
}

/// The overlays directories of one boot path.
struct Layout {
    /// The directory the firmware reads.
    read: PathBuf,
    read_rel: String,
    /// `${os_prefix}${overlay_prefix}`, the slot's directory.
    slot: PathBuf,
    slot_rel: String,
    /// `${overlay_prefix}` at the tree root: the firmware's fallback.
    root: PathBuf,
    root_rel: String,
    slot_used: bool,
}

impl Layout {
    fn new(tree: &Path, os_prefix: &str, overlay_prefix: &str) -> Self {
        let slot_rel = format!("{os_prefix}{overlay_prefix}");
        let slot = dir_at(tree, &slot_rel);
        let root = dir_at(tree, overlay_prefix);
        let slot_used = has_readme(&slot);
        let (read, read_rel) = if slot_used {
            (slot.clone(), slot_rel.clone())
        } else {
            (root.clone(), overlay_prefix.to_string())
        };
        Self {
            read,
            read_rel,
            slot,
            slot_rel,
            root,
            root_rel: overlay_prefix.to_string(),
            slot_used,
        }
    }
}

fn check_layout(
    build: &str,
    label: &str,
    file: &str,
    path: BootPath,
    layout: &Layout,
    overlays: &[String],
    out: &mut Outcome,
) {
    let slot_is_root = layout.slot_rel == layout.root_rel;
    // The slot's overlays/ without its README: the firmware ignores it.
    if !layout.slot_used && !slot_is_root && layout.slot.is_dir() {
        out.error(
            "boot_overlays_without_readme",
            None,
            format!(
                "{build}: {label}/{file}: {} has no README, so the firmware ignores it and reads {}",
                layout.slot_rel, layout.root_rel
            ),
        );
    }
    let mut seen = BTreeSet::new();
    for name in overlays {
        if !seen.insert(name.as_str()) {
            continue;
        }
        let dtbo = format!("{name}.dtbo");
        if has_file(&layout.read, &dtbo) {
            continue;
        }
        let mut message = format!(
            "{build}: {label}/{file} enables overlay {name}, but {dtbo} is not in {}, the overlays directory the firmware reads",
            layout.read_rel
        );
        let mut hints = Vec::new();
        for (dir, rel) in [
            (&layout.slot, &layout.slot_rel),
            (&layout.root, &layout.root_rel),
        ] {
            if *dir == layout.read || !has_file(dir, &dtbo) {
                continue;
            }
            hints.push(if has_readme(dir) {
                format!(
                    "{rel} has {dtbo}, but the firmware reads {}",
                    layout.read_rel
                )
            } else {
                format!(
                    "{rel} has {dtbo} but no README, so the firmware reads {}",
                    layout.read_rel
                )
            });
        }
        if !hints.is_empty() {
            message.push_str(": ");
            message.push_str(&hints.join("; "));
        }
        out.error("boot_overlay_missing", None, message);
    }
    // The fallback copy: on config.txt, with the slot's overlays read, the
    // root overlays/ must hold every .dtbo the slot ships.
    if path == BootPath::Normal && layout.read_rel != layout.root_rel && layout.root.is_dir() {
        let mut lacking: Vec<String> = std::fs::read_dir(&layout.read)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".dtbo"))
            .filter(|name| !has_file(&layout.root, name))
            .collect();
        lacking.sort();
        if !lacking.is_empty() {
            out.error(
                "boot_fallback_overlay_missing",
                None,
                format!(
                    "{build}: {label}/{file}: {} lacks {}, which {} ships; the root copy is the firmware's fallback and must hold every overlay the slot does",
                    layout.root_rel,
                    lacking.join(", "),
                    layout.read_rel
                ),
            );
        }
    }
}

/// One boot path, read: its os_prefix, overlay_prefix and overlays, in file
/// order, and what was wrong with the reading.
struct Walk<'a> {
    tree: &'a Path,
    path: BootPath,
    pi_model: Option<&'a str>,
    /// Whether the lines being read apply (section filters, when evaluated).
    active: bool,
    /// The files being read, lower-cased, for the cycle guard.
    stack: Vec<String>,
    os_prefix: Option<String>,
    overlay_prefix: Option<String>,
    overlays: Vec<String>,
    problems: Vec<(&'static str, String)>,
    notes: Vec<String>,
}

impl<'a> Walk<'a> {
    fn new(tree: &'a Path, path: BootPath, pi_model: Option<&'a str>) -> Self {
        Self {
            tree,
            path,
            pi_model,
            active: true,
            stack: Vec::new(),
            os_prefix: None,
            overlay_prefix: None,
            overlays: Vec::new(),
            problems: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Reads `name` (relative to the tree root) into this walk.
    fn read(&mut self, name: &str) {
        let key = name.to_ascii_lowercase();
        if self.stack.contains(&key) {
            let chain: Vec<&str> = self.stack.iter().map(String::as_str).collect();
            self.problems.push((
                "boot_include_cycle",
                format!(
                    "includes {name}, which is already being read ({} -> {key})",
                    chain.join(" -> ")
                ),
            ));
            return;
        }
        let Some(file) = find_file(self.tree, name) else {
            let message = if self.stack.is_empty() {
                format!("{name} is missing")
            } else {
                format!("includes {name}, which is missing")
            };
            self.problems.push(("boot_include_missing", message));
            return;
        };
        let text = match std::fs::read(&file) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).replace('\r', ""),
            Err(error) => {
                self.problems.push((
                    "boot_include_missing",
                    format!("cannot read {name}: {error}"),
                ));
                return;
            }
        };
        self.stack.push(key);
        for raw in text.lines() {
            self.line(raw);
        }
        self.stack.pop();
    }

    fn line(&mut self, raw: &str) {
        let code = raw.split('#').next().unwrap_or_default().trim();
        if code.is_empty() {
            return;
        }
        if let Some(filter) = code
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            // Filters are evaluated only when a model is named.
            if self.pi_model.is_some() {
                self.active = self.section(filter.trim());
            }
            return;
        }
        if !self.active {
            return;
        }
        if let Some(target) = code
            .strip_prefix("include")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            let target = target.trim();
            if target.is_empty() {
                self.problems
                    .push(("boot_include_missing", "has an include with no file".into()));
            } else {
                self.read(target);
            }
            return;
        }
        let Some((key, value)) = code.split_once('=') else {
            return;
        };
        match key.trim() {
            "os_prefix" => self.os_prefix = Some(value.trim().to_string()),
            "overlay_prefix" => self.overlay_prefix = Some(value.trim().to_string()),
            "dtoverlay" => {
                let name = value.split([',', ':']).next().unwrap_or_default().trim();
                if !name.is_empty() {
                    self.overlays.push(name.to_string());
                }
            }
            _ => {}
        }
    }

    /// Whether the section `filter` applies on this path (only called when a
    /// model is named).
    fn section(&mut self, filter: &str) -> bool {
        let lower = filter.to_ascii_lowercase();
        match lower.as_str() {
            "all" => true,
            "none" => false,
            "tryboot" => self.path == BootPath::Trial,
            _ if lower.starts_with("pi") || lower.starts_with("cm") => self
                .pi_model
                .is_some_and(|model| model.eq_ignore_ascii_case(&lower)),
            _ => {
                let note =
                    format!("filter [{filter}] is not known to this check, so its section applies");
                if !self.notes.contains(&note) {
                    self.notes.push(note);
                }
                true
            }
        }
    }
}

#[cfg(test)]
#[path = "boot_tests.rs"]
mod boot_tests;
