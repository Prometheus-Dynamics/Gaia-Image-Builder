//! A `cdylib`'s declared surface (`surface = { exports = "<file>" }`): after
//! the build, the library's dynamic exports must be exactly the declared
//! list. The library's own build keeps everything else local (a linker
//! version script); this only checks the result, so an export that leaks or
//! one that went missing fails the artifact instead of reaching a board.

use std::collections::BTreeSet;
use std::path::Path;

use gaia_artifact_providers::{
    ArtifactExecutionContract, ArtifactProviderError, ArtifactProviderErrorKind,
    ArtifactProviderValidationIssue, artifact_output_path, dynamic_symbols,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ArtifactVariantSpec};

/// How many names of each kind a failure lists before it counts the rest.
const NAMED: usize = 8;

/// A surface needs one shared library file to check.
pub(crate) fn validate(artifact: &ArtifactSpec, issues: &mut Vec<ArtifactProviderValidationIssue>) {
    let ArtifactDefinition::Rust(rust) = &artifact.definition else {
        return;
    };
    let Some(surface) = &rust.surface else {
        return;
    };
    if rust.variant == ArtifactVariantSpec::Directory || rust.is_std_library() {
        issues.push(ArtifactProviderValidationIssue {
            code: "rust_surface_not_a_library",
            message: "surface needs the artifact to be one shared library file, not a directory or the standard library".into(),
        });
    }
    if surface.exports.trim().is_empty() {
        issues.push(ArtifactProviderValidationIssue {
            code: "rust_surface_exports_empty",
            message: "surface exports must name the file that lists the exports".into(),
        });
    }
}

/// Checks the built library against its declared surface. `None` when the
/// artifact declares none; otherwise what was checked.
pub(crate) fn check(
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
) -> Result<Option<String>, ArtifactProviderError> {
    let ArtifactDefinition::Rust(rust) = &artifact.definition else {
        return Ok(None);
    };
    let Some(surface) = &rust.surface else {
        return Ok(None);
    };
    let source_dir = contract.source_dir.as_deref().unwrap_or(".");
    let list_path = Path::new(source_dir).join(&surface.exports);
    let list = std::fs::read_to_string(&list_path).map_err(|error| {
        failure(format!(
            "artifact '{}' surface exports '{}': {error}",
            artifact.id.as_str(),
            list_path.display()
        ))
    })?;
    let declared = declared_exports(&list).map_err(|message| {
        failure(format!(
            "artifact '{}' surface exports '{}': {message}",
            artifact.id.as_str(),
            list_path.display()
        ))
    })?;
    let library = artifact_output_path(contract, source_dir);
    let bytes = std::fs::read(&library).map_err(|error| {
        failure(format!(
            "artifact '{}' library '{}': {error}",
            artifact.id.as_str(),
            library.display()
        ))
    })?;
    let exported = dynamic_symbols(&bytes)
        .map_err(|message| {
            failure(format!(
                "artifact '{}' library '{}': {message}",
                artifact.id.as_str(),
                library.display()
            ))
        })?
        .exports
        .into_iter()
        .filter(|name| !ignored(name))
        .collect::<BTreeSet<_>>();
    let undeclared = exported.difference(&declared).collect::<Vec<_>>();
    let missing = declared.difference(&exported).collect::<Vec<_>>();
    if undeclared.is_empty() && missing.is_empty() {
        return Ok(Some(format!(
            "rust artifact '{}' exports its declared surface: {} symbol(s) from {}",
            artifact.id.as_str(),
            declared.len(),
            surface.exports
        )));
    }
    let mut parts = Vec::new();
    if !undeclared.is_empty() {
        parts.push(format!(
            "{} undeclared export(s): {}",
            undeclared.len(),
            named(&undeclared)
        ));
    }
    if !missing.is_empty() {
        parts.push(format!(
            "{} declared but not exported: {}",
            missing.len(),
            named(&missing)
        ));
    }
    Err(failure(format!(
        "rust artifact '{}' does not export its declared surface ({}): {}; the library's build sets what it exports (a version script), and {} lists the surface",
        artifact.id.as_str(),
        library.display(),
        parts.join("; "),
        surface.exports
    )))
}

/// Symbols rustc exports from every `cdylib`/`dylib` that are no ABI.
fn ignored(name: &str) -> bool {
    name.starts_with("rust_metadata_")
}

fn named(names: &[&String]) -> String {
    let mut text = names
        .iter()
        .take(NAMED)
        .map(|name| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > NAMED {
        text.push_str(&format!(" and {} more", names.len() - NAMED));
    }
    text
}

fn failure(message: String) -> ArtifactProviderError {
    ArtifactProviderError::new(ArtifactProviderErrorKind::RuntimeState, message)
}

/// The names a surface list declares. A linker version script (it has a
/// `{`) declares the names of its `global:` blocks; any other file has one
/// name per line, `#` starting a comment. Wildcards are refused: a surface
/// names each symbol.
pub(crate) fn declared_exports(text: &str) -> Result<BTreeSet<String>, String> {
    let names = if text.contains('{') {
        version_script_globals(text)?
    } else {
        text.lines()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    let mut declared = BTreeSet::new();
    for name in names {
        if name.contains(['*', '?', '[', ' ', '\t']) {
            return Err(format!("'{name}' is not one symbol name"));
        }
        if !declared.insert(name.clone()) {
            return Err(format!("'{name}' is listed twice"));
        }
    }
    if declared.is_empty() {
        return Err("it declares no symbols".into());
    }
    Ok(declared)
}

/// The `global:` names of a version script, across its version nodes.
fn version_script_globals(text: &str) -> Result<Vec<String>, String> {
    // Comments: `/* */`, `#` and `//` to the end of the line.
    let mut clean = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        clean.push_str(&rest[..start]);
        let end = rest[start..]
            .find("*/")
            .ok_or_else(|| "a /* comment is not closed".to_string())?;
        rest = &rest[start + end + 2..];
    }
    clean.push_str(rest);
    let clean = clean
        .lines()
        .map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            line.split("//").next().unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut names = Vec::new();
    let mut global = false;
    // Tokens split at `;`, `{` and `}`; a `global:`/`local:` label may share
    // a token with the first name after it.
    for token in clean.split([';', '{', '}']) {
        let mut token = token.trim();
        loop {
            if let Some(after) = token.strip_prefix("global:") {
                global = true;
                token = after.trim();
            } else if let Some(after) = token.strip_prefix("local:") {
                global = false;
                token = after.trim();
            } else {
                break;
            }
        }
        if token.is_empty() {
            continue;
        }
        // A closing `} NODE;` or `} PARENT;` names a version node, not a
        // symbol; it follows a `}` and closes the node's globals.
        if global {
            names.push(token.to_string());
        }
    }
    // Node names: the word before each `{` and after each `}`.
    let nodes = node_names(&clean);
    names.retain(|name| !nodes.contains(name));
    Ok(names)
}

/// The version node names of a script: the word before each `{` and the
/// parent names after each `}`.
fn node_names(text: &str) -> BTreeSet<String> {
    let mut nodes = BTreeSet::new();
    for (index, _) in text.match_indices('{') {
        if let Some(word) = text[..index].split_whitespace().next_back() {
            let word = word.trim_end_matches(';').trim_start_matches([';', '}']);
            if !word.is_empty() {
                nodes.insert(word.to_string());
            }
        }
    }
    for (index, _) in text.match_indices('}') {
        let after = text[index + 1..].trim_start();
        let word = after.split(';').next().unwrap_or_default().trim();
        if !word.is_empty() && !word.contains(['{', '}']) {
            nodes.insert(word.to_string());
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_list_takes_one_name_per_line_and_comments() {
        let declared =
            declared_exports("# the surface\nsurface_open\n\nsurface_tick  # per frame\n")
                .expect("list");
        assert_eq!(
            declared.into_iter().collect::<Vec<_>>(),
            ["surface_open", "surface_tick"]
        );
    }

    #[test]
    fn a_version_script_declares_its_global_names_not_its_nodes() {
        let script = "/* daedalus */\nDAEDALUS_1 {\n  global:\n    surface_open;\n    surface_tick; # per frame\n  local: *;\n};\nDAEDALUS_2 {\n  global: surface_close;\n} DAEDALUS_1;\n";
        let declared = declared_exports(script).expect("script");
        assert_eq!(
            declared.into_iter().collect::<Vec<_>>(),
            ["surface_close", "surface_open", "surface_tick"]
        );
    }

    #[test]
    fn wildcards_duplicates_and_empty_lists_are_refused() {
        assert!(declared_exports("{ global: surface_*; local: *; };").is_err());
        assert!(declared_exports("a\na\n").is_err());
        assert!(declared_exports("# nothing\n").is_err());
        assert!(declared_exports("{ global: a; /* open").is_err());
    }
}
