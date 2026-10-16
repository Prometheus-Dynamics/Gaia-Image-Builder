//! Validation of the `include` and `strip` options of assembly files, and of
//! the assembly's `strip_tool`.

use std::collections::HashSet;

use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// `include` lists file names of a `src_glob`; each entry must be a bare,
/// non-empty name.
pub(crate) fn validate_include(
    file: &gaia_spec::AssemblyFileSpec,
    index: usize,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if file.include.is_empty() {
        return;
    }
    if file.src_glob.is_none() {
        diagnostics.push(error(
            "assembly_include_not_allowed",
            format!("files[{index}].include only applies to src_glob, not to src, content or json"),
            Some("image.assembly.files".into()),
        ));
    }
    for name in &file.include {
        if name.trim().is_empty() || name.contains('/') || name.contains('\0') {
            diagnostics.push(error(
                "assembly_include_invalid",
                format!("files[{index}].include entry '{name}' must be a file name without '/'"),
                Some("image.assembly.files".into()),
            ));
        }
    }
}

/// `strip` strips copied ELF files; generated content is never an ELF copy.
pub(crate) fn validate_strip_file(
    file: &gaia_spec::AssemblyFileSpec,
    index: usize,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if file.strip && (file.content.is_some() || file.json.is_some()) {
        diagnostics.push(error(
            "assembly_strip_not_allowed",
            format!(
                "files[{index}].strip only applies to a copied file (src or src_glob), not generated content"
            ),
            Some("image.assembly.files".into()),
        ));
    }
}

/// `strip_tool` is a program name, or a path that may use provider tokens.
pub(crate) fn validate_strip_tool(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<gaia_spec::AssemblyTreeId>,
    tool: &gaia_spec::AssemblyPathTemplate,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if tool.as_str().trim().is_empty() {
        diagnostics.push(error(
            "assembly_strip_tool_empty",
            "assembly strip_tool cannot be empty".into(),
            Some("image.assembly.strip_tool".into()),
        ));
        return;
    }
    validate_assembly_path_template(
        spec,
        tree_ids,
        tool,
        "image.assembly.strip_tool",
        diagnostics,
    );
}
