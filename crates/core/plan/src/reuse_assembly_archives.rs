use crate::reuse::path_state_signature;
use gaia_spec::{AssemblyArchiveMemberSourceSpec, AssemblyRoots, ResolvedBuildSpec};
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
                        token_input_entries(
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
                None => {}
            }
        }
    }
    parts
}

/// Fingerprint entries for the files a value's `${assembly...}` tokens read,
/// named after `owner` (for example `archive a/m` or `file tree:dest`) and
/// tagged with `kind`.
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
