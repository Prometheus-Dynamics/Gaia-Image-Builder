//! `[[image.assembly.checks]]` validation: unique ids, a command, and trees
//! the command's `$assembly.tree.<id>` tokens name.

use std::collections::HashSet;

use gaia_spec::{AssemblyTreeId, ImageAssemblySpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

const TREE_TOKEN: &str = "$assembly.tree.";

pub(crate) fn validate_checks(
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.checks".to_string());
    let mut ids = HashSet::new();
    for check in &assembly.checks {
        if check.id.trim().is_empty() || !ids.insert(check.id.as_str()) {
            diagnostics.push(error(
                "assembly_check_id_invalid",
                format!(
                    "image.assembly.checks id '{}' is empty or used twice",
                    check.id
                ),
                location(),
            ));
        }
        if check
            .command
            .first()
            .is_none_or(|program| program.trim().is_empty())
        {
            diagnostics.push(error(
                "assembly_check_command_missing",
                format!(
                    "image.assembly.checks '{}' has no command; set command = [\"program\", \"arg\", ...]",
                    check.id
                ),
                location(),
            ));
        }
        for arg in &check.command {
            for tree in named_trees(arg) {
                if !tree_ids.iter().any(|id| id.as_str() == tree) {
                    diagnostics.push(error(
                        "assembly_check_tree_unknown",
                        format!(
                            "image.assembly.checks '{}' names $assembly.tree.{tree}, which no [[image.assembly.trees]] entry defines",
                            check.id
                        ),
                        location(),
                    ));
                }
            }
        }
    }
}

/// The tree ids `$assembly.tree.<id>` tokens in `arg` name.
fn named_trees(arg: &str) -> Vec<&str> {
    let mut trees = Vec::new();
    let mut rest = arg;
    while let Some(index) = rest.find(TREE_TOKEN) {
        let after = &rest[index + TREE_TOKEN.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(after.len());
        trees.push(&after[..end]);
        rest = &after[end..];
    }
    trees
}
