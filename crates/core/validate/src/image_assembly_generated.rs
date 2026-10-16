//! Validation of generated content: file entries with `content` or `json`,
//! listed binaries, and the `${assembly...}` tokens of generated text, JSON
//! and archive entries. A token that reads an assembly output no step writes
//! is an error, since nothing would produce it before the read.

use std::collections::HashSet;
use std::path::PathBuf;

use gaia_spec::{
    AssemblyTreeId, AssemblyValueToken, ImageAssemblySpec, ResolvedBuildSpec, assembly_step_paths,
    assembly_value_tokens,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// One place a generated text or JSON string is written, and the text.
pub(crate) struct TokenSite<'a> {
    /// How the site is named in messages.
    pub(crate) label: String,
    pub(crate) text: &'a str,
    /// The config location the site comes from.
    pub(crate) location: &'static str,
}

/// Every generated string that carries `${assembly...}` tokens: file content
/// and JSON, archive entries and archive JSON members.
pub(crate) fn token_sites(assembly: &ImageAssemblySpec) -> Vec<TokenSite<'_>> {
    let mut sites = Vec::new();
    for file in &assembly.files {
        let label = format!("file '{}:{}'", file.tree, file.dest);
        if let Some(content) = &file.content {
            sites.push(TokenSite {
                label: format!("{label} content"),
                text: content,
                location: "image.assembly.files.content",
            });
        }
        if let Some(json) = &file.json {
            for text in json.strings() {
                sites.push(TokenSite {
                    label: format!("{label} json"),
                    text,
                    location: "image.assembly.files.json",
                });
            }
        }
    }
    for archive in &assembly.archives {
        for member in &archive.members {
            let label = format!("archive '{}' member '{}'", archive.id, member.name);
            for (key, value) in member.entries.iter().flatten() {
                sites.push(TokenSite {
                    label: format!("{label} entry '{key}'"),
                    text: value,
                    location: "image.assembly.archives.generated.entries",
                });
            }
            if let Some(json) = &member.json {
                for text in json.strings() {
                    sites.push(TokenSite {
                        label: format!("{label} json"),
                        text,
                        location: "image.assembly.archives.generated.json",
                    });
                }
            }
        }
    }
    sites
}

/// A file entry with `content` or `json`: exactly one generated source, a
/// file (not a directory) as its dest, and well-formed tokens.
pub(crate) fn validate_generated_file(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    file: &gaia_spec::AssemblyFileSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = Some("image.assembly.files".to_string());
    let generated = usize::from(file.content.is_some()) + usize::from(file.json.is_some());
    if generated > 1 {
        diagnostics.push(error(
            "assembly_file_source_invalid",
            format!(
                "assembly file '{}:{}' must set exactly one of content or json",
                file.tree, file.dest
            ),
            location.clone(),
        ));
    }
    if file.src.is_some() || file.src_glob.is_some() {
        diagnostics.push(error(
            "assembly_file_source_invalid",
            format!(
                "assembly file '{}:{}' sets content or json, so it cannot also set src or src_glob",
                file.tree, file.dest
            ),
            location.clone(),
        ));
    }
    let dest = file.dest.trim();
    if dest.is_empty() || dest == "." || dest.ends_with('/') {
        diagnostics.push(error(
            "assembly_file_generated_dest_invalid",
            format!(
                "assembly file dest '{}' must name the file to write, not a directory",
                file.dest
            ),
            location,
        ));
    }
    if let Err(problem) = file.parsed_mode() {
        diagnostics.push(error(
            "assembly_file_mode_invalid",
            format!("invalid assembly file mode: {problem}"),
            Some("image.assembly.files".into()),
        ));
    }
    if let Some(content) = &file.content {
        validate_tokens(
            spec,
            tree_ids,
            content,
            "image.assembly.files.content",
            &format!("file '{}:{}' content", file.tree, file.dest),
            diagnostics,
        );
    }
    if let Some(json) = &file.json {
        for text in json.strings() {
            validate_tokens(
                spec,
                tree_ids,
                text,
                "image.assembly.files.json",
                &format!("file '{}:{}' json", file.tree, file.dest),
                diagnostics,
            );
        }
    }
}

/// Checks each token of `text`: its path template is valid, and it is one
/// the config can name (an unknown `${...}` is an error).
pub(crate) fn validate_tokens(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    text: &str,
    location: &str,
    label: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let rendered = assembly_value_tokens(text, |token| {
        validate_assembly_path_template(spec, tree_ids, token.path(), location, diagnostics);
        Ok(String::new())
    });
    if let Err(message) = rendered {
        diagnostics.push(error(
            "assembly_generated_token_invalid",
            format!("{label}: {message}"),
            Some(location.into()),
        ));
    }
}

/// Every listed binary: a known tree, a source and a destination in it, and
/// a valid mode.
pub(crate) fn validate_binaries(
    spec: &ResolvedBuildSpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = Some("image.assembly.binaries".to_string());
    for binary in &assembly.binaries {
        if !tree_ids.contains(&binary.tree) {
            diagnostics.push(error(
                "assembly_binary_tree_unknown",
                format!("assembly binary references unknown tree '{}'", binary.tree),
                location.clone(),
            ));
        }
        let dest = binary.dest.trim();
        if dest.is_empty()
            || dest.ends_with('/')
            || dest == "."
            || dest.starts_with('/')
            || dest.split('/').any(|part| part == "..")
        {
            diagnostics.push(error(
                "assembly_binary_dest_invalid",
                format!(
                    "assembly binary dest '{}' must be a file path inside its tree",
                    binary.dest
                ),
                location.clone(),
            ));
        }
        if binary.src.as_str().trim().is_empty() {
            diagnostics.push(error(
                "assembly_binary_src_empty",
                format!("assembly binary '{}' src cannot be empty", binary.dest),
                location.clone(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &binary.src,
                "image.assembly.binaries.src",
                diagnostics,
            );
        }
        if let Some(sysroot) = &binary.sysroot {
            validate_assembly_path_template(
                spec,
                tree_ids,
                sysroot,
                "image.assembly.binaries.sysroot",
                diagnostics,
            );
        }
        if let Err(problem) = binary.parsed_mode() {
            diagnostics.push(error(
                "assembly_binary_mode_invalid",
                format!("invalid assembly binary mode: {problem}"),
                location.clone(),
            ));
        }
    }
}

/// A token that reads an assembly work or out path must read something a
/// step writes: the write's path, as the config spells it, is a prefix of
/// the read (or is the read). Other paths name the provider's files, which
/// exist before the assembly runs.
pub(crate) fn validate_token_reads(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let paths = assembly_step_paths(
        assembly,
        &|template| Some(PathBuf::from(template.as_str())),
        &|tree| {
            assembly
                .trees
                .iter()
                .find(|candidate| candidate.id.as_str() == tree)
                .map(|candidate| PathBuf::from(candidate.path.as_str()))
        },
    );
    let writes: Vec<PathBuf> = paths
        .iter()
        .flat_map(|step| step.writes.iter().cloned())
        .collect();
    for site in token_sites(assembly) {
        let _ = assembly_value_tokens(site.text, |token| {
            check_read(token, &site, &writes, diagnostics);
            Ok(String::new())
        });
    }
}

fn check_read(
    token: &AssemblyValueToken,
    site: &TokenSite<'_>,
    writes: &[PathBuf],
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let text = token.path().as_str().trim();
    let produced_here = text.starts_with("$assembly.work") || text.starts_with("$assembly.out");
    if !produced_here {
        return;
    }
    let read = PathBuf::from(text);
    if writes.iter().any(|write| read.starts_with(write)) {
        return;
    }
    diagnostics.push(error(
        "assembly_token_path_not_produced",
        format!(
            "{} reads '{text}', which no assembly step writes; name a path a step produces, exactly as the step writes it",
            site.label
        ),
        Some(site.location.into()),
    ));
}
