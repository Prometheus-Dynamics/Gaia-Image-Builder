//! `[[image.rootfs.out_of_tree_modules]]`: Kbuild modules a Buildroot image
//! builds against its kernel. Each needs a name that makes a package name
//! (`gaia-kmod-<name>`), listed once, and a directory.
use std::collections::HashSet;

use gaia_spec::OutOfTreeModuleSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_out_of_tree_modules(
    modules: &[OutOfTreeModuleSpec],
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut names = HashSet::new();
    for module in modules {
        let name = module.name.as_str();
        let valid = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c));
        let problem = if !valid {
            format!(
                "out_of_tree_modules name '{name}' must be lowercase letters, digits, '-' or '_' (it names the package gaia-kmod-<name>)"
            )
        } else if module.path.is_empty() {
            format!(
                "out_of_tree_modules entry '{name}' needs a path (the module's Kbuild directory)"
            )
        } else if !names.insert(name) {
            format!("out_of_tree_modules names '{name}' twice")
        } else {
            continue;
        };
        diagnostics.push(error(
            "buildroot_out_of_tree_module_invalid",
            problem,
            Some("image.rootfs.out_of_tree_modules".into()),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problems(modules: &[(&str, &str)]) -> Vec<String> {
        let modules: Vec<OutOfTreeModuleSpec> = modules
            .iter()
            .map(|(name, path)| OutOfTreeModuleSpec {
                name: name.to_string(),
                path: path.to_string(),
            })
            .collect();
        let mut diagnostics = Vec::new();
        validate_out_of_tree_modules(&modules, &mut diagnostics);
        diagnostics.into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn names_paths_and_duplicates_are_checked() {
        assert!(
            problems(&[
                ("lemnos", "lemnos/kernel-modules/lemnos"),
                ("wifi_8821", "w")
            ])
            .is_empty()
        );
        let found = problems(&[
            ("lemnos", "a"),
            ("lemnos", "b"),
            ("Lemnos", "c"),
            ("-x", "d"),
            ("ok", ""),
        ]);
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found[0].contains("twice"));
        assert!(found[1].contains("lowercase"));
        assert!(found[2].contains("lowercase"));
        assert!(found[3].contains("needs a path"));
    }
}
