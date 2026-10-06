use crate::reuse::path_state_signature;
use gaia_spec::{AssemblyArchiveMemberSourceSpec, AssemblyRoots, ResolvedBuildSpec};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Fingerprint parts for archive inputs. The archive config itself is
/// covered by the hashed assembly spec; this adds the state of every file a
/// member or `${assembly.sha256:...}` token reads. Files produced earlier in
/// the same assembly are recorded by path only, since their own inputs are
/// already fingerprinted.
pub(crate) fn archive_input_parts(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    assembly: &gaia_spec::ImageAssemblySpec,
    generated_outputs: &BTreeSet<PathBuf>,
) -> Vec<String> {
    let mut parts = Vec::new();
    for archive in &assembly.archives {
        for member in &archive.members {
            match member.source() {
                Some(AssemblyArchiveMemberSourceSpec::File(src)) => {
                    parts.push(input_part(
                        "archive-member",
                        &archive.id,
                        &member.name,
                        resolve(spec, roots, src),
                        generated_outputs,
                    ));
                }
                Some(AssemblyArchiveMemberSourceSpec::Generated(entries)) => {
                    for (_, value) in entries {
                        let _ = gaia_spec::assembly_digest_tokens(value, |token| {
                            parts.push(input_part(
                                "archive-digest",
                                &archive.id,
                                &member.name,
                                resolve(spec, roots, &token.path),
                                generated_outputs,
                            ));
                            Ok(String::new())
                        });
                    }
                }
                None => {}
            }
        }
    }
    parts
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
    archive_id: &str,
    member: &str,
    path: PathBuf,
    generated_outputs: &BTreeSet<PathBuf>,
) -> String {
    if generated_outputs.contains(&path) {
        return format!("{kind}-generated:{archive_id}:{member}:{}", path.display());
    }
    format!(
        "{kind}:{archive_id}:{member}:{}:{}",
        path.display(),
        path_state_signature(Path::new(&path))
    )
}
