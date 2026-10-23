use crate::reuse::path_state_signature;
use gaia_spec::{
    AssemblyArchiveMemberSourceSpec, AssemblyRoots, AssemblyValueToken, ResolvedBuildSpec,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Fingerprint entries for archive inputs, as `(name, part)` pairs. The
/// archive config itself is covered by the hashed assembly spec; this adds the
/// state of every file a member or `${assembly...}` token reads. Files
/// produced earlier in the same assembly are recorded by path only, since
/// their own inputs are already fingerprinted.
pub(crate) fn archive_input_entries(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    assembly: &gaia_spec::ImageAssemblySpec,
    generated_outputs: &BTreeSet<PathBuf>,
) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    for archive in &assembly.archives {
        for member in &archive.members {
            // The name reads `archive id/member`; the fingerprint parts keep
            // their long-standing `id:member` form, so recorded fingerprints hold.
            let owner = format!("archive {}/{}", archive.id, member.name);
            let part_owner = format!("{}:{}", archive.id, member.name);
            match member.source() {
                Some(AssemblyArchiveMemberSourceSpec::File(src)) => {
                    parts.push(input_part(
                        "archive-member",
                        &owner,
                        &part_owner,
                        resolve(spec, roots, src),
                        generated_outputs,
                    ));
                }
                Some(AssemblyArchiveMemberSourceSpec::Generated(entries)) => {
                    for (_, value) in entries {
                        token_input_entries(
                            spec,
                            roots,
                            "archive-digest",
                            (&owner, &part_owner),
                            value,
                            generated_outputs,
                            &mut parts,
                        );
                    }
                }
                Some(AssemblyArchiveMemberSourceSpec::Json(json)) => {
                    for text in json.strings() {
                        json_token_input_entries(
                            spec,
                            roots,
                            "archive-digest",
                            (&owner, &part_owner),
                            text,
                            generated_outputs,
                            &mut parts,
                        );
                    }
                }
                Some(AssemblyArchiveMemberSourceSpec::Tree(tree)) => {
                    // The tree's root state covers every entry under it.
                    if let Ok(root) = roots.tree_path(tree.as_str()) {
                        parts.push(input_part(
                            "archive-member",
                            &owner,
                            &part_owner,
                            root.to_path_buf(),
                            generated_outputs,
                        ));
                    }
                }
                Some(AssemblyArchiveMemberSourceSpec::Dir(dir)) => {
                    parts.push(input_part(
                        "archive-member",
                        &owner,
                        &part_owner,
                        resolve(spec, roots, dir),
                        generated_outputs,
                    ));
                }
                Some(AssemblyArchiveMemberSourceSpec::Glob(pattern)) => {
                    // The matched files, and the set they form, are the inputs.
                    let matches = gaia_spec::expand_simple_glob(spec, roots, pattern.as_str())
                        .map(|paths| gaia_spec::exclude_names(paths, &member.exclude))
                        .unwrap_or_default();
                    let names = matches
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    parts.push((
                        format!("archive {owner} matches"),
                        format!(
                            "archive-glob:{part_owner}:{}",
                            crate::reuse_assembly::text_digest(&names)
                        ),
                    ));
                    for path in matches {
                        parts.push(input_part(
                            "archive-member",
                            &owner,
                            &part_owner,
                            path,
                            generated_outputs,
                        ));
                    }
                }
                None => {}
            }
        }
    }
    parts
}

/// Fingerprint entries for OCI image inputs: each layer tar's state, and
/// each tree layer's content digest. The OCI config itself is covered by
/// the hashed assembly spec.
pub(crate) fn oci_input_entries(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    assembly: &gaia_spec::ImageAssemblySpec,
    generated_outputs: &BTreeSet<PathBuf>,
) -> Vec<(String, String)> {
    use gaia_spec::AssemblyOciLayerSource;
    let mut parts = Vec::new();
    for oci in &assembly.ocis {
        for (index, layer) in oci.layers.iter().enumerate() {
            let owner = format!("oci {}/layer {}", oci.id, index + 1);
            let part_owner = format!("{}:layer{}", oci.id, index + 1);
            match layer.source() {
                Some(AssemblyOciLayerSource::Tar(tar)) => {
                    parts.push(input_part(
                        "oci-layer",
                        &owner,
                        &part_owner,
                        resolve(spec, roots, tar),
                        generated_outputs,
                    ));
                }
                Some(AssemblyOciLayerSource::Tree(tree)) => {
                    if let Ok(root) = roots.tree_path(tree.as_str()) {
                        parts.push((
                            format!("{owner} {}", root.display()),
                            format!(
                                "oci-tree:{part_owner}:{}:{}",
                                root.display(),
                                crate::tree_digest::tree_digest_signature(root)
                            ),
                        ));
                    }
                }
                None => {}
            }
        }
    }
    parts
}

/// Fingerprint entries for the files a text value's `${assembly...}` tokens
/// read, named after `owner` (for example `archive a/m` or `file tree:dest`)
/// and tagged with `kind`.
pub(crate) fn token_input_entries(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    kind: &str,
    // The name of the owner, and the owner as the fingerprint parts spell it.
    (owner, part_owner): (&str, &str),
    value: &str,
    generated_outputs: &BTreeSet<PathBuf>,
    parts: &mut Vec<(String, String)>,
) {
    let _ = gaia_spec::assembly_value_tokens(value, |token| {
        parts.push(input_part(
            kind,
            owner,
            part_owner,
            resolve(spec, roots, token.path()),
            generated_outputs,
        ));
        Ok(String::new())
    });
}

/// `token_input_entries` for a JSON string. A tree digest is covered by its
/// tree's content signature, so the owner rebuilds when a file in the tree
/// changes, whatever the tree's mtimes.
pub(crate) fn json_token_input_entries(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    kind: &str,
    (owner, part_owner): (&str, &str),
    value: &str,
    generated_outputs: &BTreeSet<PathBuf>,
    parts: &mut Vec<(String, String)>,
) {
    let _ = gaia_spec::assembly_json_string_tokens(value, |token| {
        match token {
            AssemblyValueToken::TreeDigest(digest) => {
                let root = roots
                    .tree_or_path(spec, digest.path.as_str())
                    .unwrap_or_else(|_| PathBuf::from(digest.path.as_str()));
                parts.push((
                    format!("{owner} {}", root.display()),
                    format!(
                        "{kind}-tree:{part_owner}:{}:{}",
                        root.display(),
                        crate::tree_digest::tree_digest_signature(&root)
                    ),
                ));
            }
            other => parts.push(input_part(
                kind,
                owner,
                part_owner,
                resolve(spec, roots, other.path()),
                generated_outputs,
            )),
        }
        Ok(String::new())
    });
}

fn resolve(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    template: &gaia_spec::AssemblyPathTemplate,
) -> PathBuf {
    roots
        .resolve_path(spec, template)
        .unwrap_or_else(|_| PathBuf::from(template.as_str()))
}

fn input_part(
    kind: &str,
    owner: &str,
    part_owner: &str,
    path: PathBuf,
    generated_outputs: &BTreeSet<PathBuf>,
) -> (String, String) {
    let name = format!("{owner} {}", path.display());
    if generated_outputs.contains(&path) {
        return (
            name,
            format!("{kind}-generated:{part_owner}:{}", path.display()),
        );
    }
    (
        name,
        format!(
            "{kind}:{part_owner}:{}:{}",
            path.display(),
            path_state_signature(Path::new(&path))
        ),
    )
}
