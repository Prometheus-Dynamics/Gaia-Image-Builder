//! Exact version pins in a node's Cargo.toml.
//!
//! A node that links a pure-Rust library with no version the library reports
//! at run time pins it: `name = "=x.y.z"`. The node's step key names the same
//! version ([`KeyBuilder::lib`](crate::KeyBuilder::lib)), and the node's test
//! calls [`check_pins`], which reads the node's own Cargo.toml. So the
//! version moves only in a commit that moves the key too, and a fresh resolve
//! cannot move it.

use std::path::Path;

/// The exact versions the manifest text pins `name` to: the `x.y.z` of each
/// `name = "=x.y.z"` and `name = { version = "=x.y.z", .. }` line. A version
/// without the `=` is not a pin.
pub fn pinned_versions(manifest: &str, name: &str) -> Vec<String> {
    manifest
        .lines()
        .filter_map(|line| pin_on_line(line, name))
        .collect()
}

fn pin_on_line(line: &str, name: &str) -> Option<String> {
    let rest = line
        .trim_start()
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let rest = match rest.strip_prefix('{') {
        Some(table) => table
            .split_once("version")?
            .1
            .trim_start()
            .strip_prefix('=')?
            .trim_start(),
        None => rest,
    };
    let value = rest.strip_prefix('"')?.split('"').next()?;
    value.strip_prefix('=').map(str::to_string)
}

/// Checks that each `(name, version)` is pinned to `version` in the Cargo.toml
/// beside the node crate. `manifest_dir` is the node's `CARGO_MANIFEST_DIR`.
/// Returns every mismatch in one error.
pub fn check_pins(manifest_dir: &str, libs: &[(&str, &str)]) -> Result<(), String> {
    let path = Path::new(manifest_dir).join("Cargo.toml");
    let manifest =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut problems = Vec::new();
    for (name, version) in libs {
        let pins = pinned_versions(&manifest, name);
        if pins.is_empty() {
            problems.push(format!("{name}: not pinned in Cargo.toml (={version})"));
        }
        for pin in pins {
            if pin != *version {
                problems.push(format!(
                    "{name}: the step key names {version}, Cargo.toml pins ={pin}"
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}
