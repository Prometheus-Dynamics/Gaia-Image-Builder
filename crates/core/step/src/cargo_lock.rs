//! Reads the versions Cargo resolved, from Cargo.lock.
//!
//! A node that links a format or encoder library puts that library's
//! resolved version in its step key ([`KeyBuilder::lib`](crate::KeyBuilder::lib)).
//! The node's test calls [`check_libs`], so the test fails when Cargo's
//! resolved version differs from the one the key names.

use std::path::Path;

/// Every version of the package `name` in Cargo.lock text. A package that
/// is in the lock once gives one; a crate that is not there gives none.
pub fn resolved_versions(lock: &str, name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current: Option<&str> = None;
    for line in lock.lines() {
        if line == "[[package]]" {
            current = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            current = Some(value.trim_matches('"'));
        } else if let Some(value) = line
            .strip_prefix("version = ")
            .filter(|_| current == Some(name))
        {
            found.push(value.trim_matches('"').to_string());
        }
    }
    found
}

/// Checks each `(name, version)` against the Cargo.lock beside the
/// workspace root. `manifest_dir` is the node crate's `CARGO_MANIFEST_DIR`
/// (`crates/<folder>/<name>`). Returns every mismatch in one error.
pub fn check_libs(manifest_dir: &str, libs: &[(&str, &str)]) -> Result<(), String> {
    let path = Path::new(manifest_dir).join("../../../Cargo.lock");
    let lock =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut problems = Vec::new();
    for (name, version) in libs {
        match resolved_versions(&lock, name).as_slice() {
            [resolved] if resolved == version => {}
            [resolved] => problems.push(format!(
                "{name}: the step key names {version}, Cargo.lock resolves {resolved}"
            )),
            [] => problems.push(format!("{name}: not in Cargo.lock")),
            many => problems.push(format!(
                "{name}: {} versions in Cargo.lock, the key names one",
                many.len()
            )),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}
