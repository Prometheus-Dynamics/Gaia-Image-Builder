//! Generated content: the `${assembly...}` tokens of archive entries, text
//! files and JSON documents are rendered here, and a tree file generated
//! from text or JSON is written here.

use super::files::assembly_dest_is_directory;
use super::*;
use gaia_spec::{AssemblyJsonValue, AssemblyPathTemplate, AssemblyValueToken};

/// The value of one `${assembly...}` token: a file's sha256, a field of a
/// verity transform's output, or a file's size in bytes. A tree digest is
/// a JSON object, so it is rendered by [`tree_digest_members`] instead.
pub(super) fn token_value(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    verity: &VerityOutputs,
    token: &AssemblyValueToken,
) -> Result<String, String> {
    match token {
        AssemblyValueToken::Digest(token) => {
            let path = roots.resolve_path(spec, &token.path)?;
            if !path.is_file() {
                return Err(format!(
                    "'${{assembly.sha256:{}}}' file '{}' does not exist",
                    token.path,
                    path.display()
                ));
            }
            file_sha256(&path)
        }
        AssemblyValueToken::Verity(token) => {
            let path = roots.resolve_path(spec, &token.path)?;
            let output = verity.get(&path).ok_or_else(|| {
                format!(
                    "'${{assembly.verity:{}:{}}}' names no verity transform output at '{}'",
                    token.path,
                    token.field.as_str(),
                    path.display()
                )
            })?;
            Ok(output.field(token.field))
        }
        AssemblyValueToken::Size(token) => {
            let path = roots.resolve_path(spec, &token.path)?;
            if !path.is_file() {
                return Err(format!(
                    "'${{assembly.size:{}}}' file '{}' does not exist",
                    token.path,
                    path.display()
                ));
            }
            Ok(file_len(&path)?.to_string())
        }
        AssemblyValueToken::TreeDigest(token) => Err(format!(
            "'${{assembly.sha256_tree:{}}}' is a whole JSON value; it cannot be rendered as text",
            token.path
        )),
    }
}

/// The per-file digest map of a tree (a tree id, or a path template): each
/// regular file under it, by its path with `/` separators, with its sha256
/// hex, sorted by path. Symlinks and directories are not listed; a symlink
/// is not followed.
pub(super) fn tree_digest_members(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    template: &AssemblyPathTemplate,
) -> Result<Vec<(String, String)>, String> {
    let root = roots.tree_or_path(spec, template.as_str())?;
    if !root.is_dir() {
        return Err(format!(
            "'${{assembly.sha256_tree:{}}}' names '{}', which is not a directory",
            template,
            root.display()
        ));
    }
    gaia_plan::tree_digest_members(&root)
}

/// Renders the tokens of a text value.
pub(super) fn render_text(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    verity: &VerityOutputs,
    text: &str,
) -> Result<String, String> {
    gaia_spec::assembly_value_tokens(text, |token| token_value(spec, roots, verity, token))
}

/// The text of a JSON document, with its tokens rendered. A tree digest is
/// read after the steps that fill its tree (see `assembly_step_paths`).
pub(super) fn render_json(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    verity: &VerityOutputs,
    json: &AssemblyJsonValue,
) -> Result<String, String> {
    json.render_with_trees(
        &mut |token| token_value(spec, roots, verity, token),
        &mut |template| tree_digest_members(spec, roots, template),
    )
}

/// A tree file generated from `content` or `json`, as written.
pub(super) struct GeneratedFile {
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

/// The mode a generated file gets when the config does not set one. Fixed,
/// so the tree does not depend on the host's umask.
const DEFAULT_GENERATED_MODE: &str = "0644";

/// Writes a file entry's generated content into the tree.
pub(super) fn write_generated_file(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    verity: &VerityOutputs,
    tree_path: &Path,
    file: &gaia_spec::AssemblyFileSpec,
) -> Result<GeneratedFile, String> {
    if assembly_dest_is_directory(&file.dest) {
        return Err(format!(
            "assembly file for tree '{}' generates content for dest '{}', which names a directory; name the file",
            file.tree, file.dest
        ));
    }
    let text = match (&file.content, &file.json) {
        (Some(content), None) => render_text(spec, roots, verity, content)?,
        (None, Some(json)) => render_json(spec, roots, verity, json)?,
        _ => {
            return Err(format!(
                "assembly file for tree '{}' must set exactly one of src, src_glob, content or json",
                file.tree
            ));
        }
    };
    // Text written to a .json file must be JSON: a TOML literal string
    // ('...\n') keeps a backslash-n and once shipped an invalid state.json.
    if file.content.is_some() && file.dest.to_ascii_lowercase().ends_with(".json") {
        serde_json::from_str::<serde_json::Value>(&text).map_err(|error| {
            format!(
                "assembly file for tree '{}' writes '{}', which is not valid JSON after its tokens are filled in ({error}); in TOML a '...' literal string keeps a backslash-n as two characters: use a \"...\" string, a ''' multi-line string, or `json = {{ ... }}`",
                file.tree, file.dest
            )
        })?;
    }
    let dest = assembly_tree_dest(tree_path, &file.dest)?;
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create generated file dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    std_fs::write(&dest, text.as_bytes()).map_err(|error| {
        format!(
            "failed to write generated file '{}': {error}",
            dest.display()
        )
    })?;
    let mode = match file.parsed_mode().map_err(|error| error.to_string())? {
        Some(mode) => Some(mode),
        None => Some(
            DEFAULT_GENERATED_MODE
                .parse()
                .map_err(|error: gaia_spec::FileModeParseError| error.to_string())?,
        ),
    };
    apply_mode(&dest, mode)?;
    Ok(GeneratedFile {
        bytes: file_len(&dest)?,
        sha256: file_sha256(&dest)?,
        dest,
    })
}
