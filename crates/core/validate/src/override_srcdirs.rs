//! `[image] override_srcdirs`: `[package, dir]` pairs a Buildroot image builds
//! from a directory. Each needs a package name Buildroot could define and a
//! directory, and names its package once.
use std::collections::HashSet;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_override_srcdirs(
    entries: &[(String, String)],
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut packages = HashSet::new();
    for (package, dir) in entries {
        let valid = !package.is_empty()
            && package
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
        let problem = if !valid {
            format!("override_srcdirs package '{package}' is not a Buildroot package name")
        } else if dir.trim().is_empty() {
            format!("override_srcdirs entry '{package}' needs a directory")
        } else if !packages.insert(package.as_str()) {
            format!("override_srcdirs names package '{package}' twice")
        } else {
            continue;
        };
        diagnostics.push(error(
            "buildroot_override_srcdir_invalid",
            problem,
            Some("image".into()),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(entries: &[(&str, &str)]) -> Vec<String> {
        let entries: Vec<(String, String)> = entries
            .iter()
            .map(|(package, dir)| (package.to_string(), dir.to_string()))
            .collect();
        let mut diagnostics = Vec::new();
        validate_override_srcdirs(&entries, &mut diagnostics);
        diagnostics.into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn names_dirs_and_duplicates_are_checked() {
        assert!(codes(&[("linux", "sdk/kernel"), ("uboot", "@sdk/u-boot")]).is_empty());
        let problems = codes(&[
            ("linux", "sdk/kernel"),
            ("linux", "other/kernel"),
            ("LINUX OVERRIDE", "x"),
            ("uboot", " "),
        ]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("twice"));
        assert!(problems[1].contains("not a Buildroot package name"));
        assert!(problems[2].contains("needs a directory"));
    }
}
