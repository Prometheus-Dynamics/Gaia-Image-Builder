//! `${source.<id>.commit}`: the exact commit a source builds from, so an
//! image can record it (for example in a stage env set that becomes an
//! `/etc` file, or a build label).
//!
//! Resolved after compilation, once the lockfile is applied:
//! - git source: its lockfile commit, else a full-sha `rev`;
//! - import source: the commit its checkout was made at;
//! - path source (including `--set sources.<id>.path` overrides): the
//!   directory's `git rev-parse HEAD`, suffixed `-dirty` when tracked files
//!   have uncommitted changes.
//!
//! A git source tracking a branch or tag without a lock has no exact commit
//! before it is fetched; its token stays unresolved and validation reports it
//! (pin it with `rev` or `gaia lock`).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use gaia_spec::{ResolvedBuildSpec, SourceDefinition};

const PREFIX: &str = "source.";
const SUFFIX: &str = ".commit";

/// Substitutes `${source.<id>.commit}` in stage env set values and build
/// labels, and drops the tokens it resolved from the unresolved list.
pub(crate) fn substitute_source_commits(spec: &mut ResolvedBuildSpec) {
    let commits = source_commits(spec);
    if commits.is_empty() {
        return;
    }
    for env_set in &mut spec.stage.env_sets {
        for (_, value) in &mut env_set.entries {
            *value = substitute(value, &commits);
        }
    }
    for (_, value) in &mut spec.metadata.labels {
        *value = substitute(value, &commits);
    }
    spec.policy.interpolation.unresolved.retain(|unresolved| {
        source_token_id(&unresolved.token).is_none_or(|id| !commits.contains_key(id))
    });
}

fn source_token_id(token: &str) -> Option<&str> {
    token.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)
}

fn substitute(value: &str, commits: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let token = &remainder[..end];
        match source_token_id(token).and_then(|id| commits.get(id)) {
            Some(commit) => output.push_str(commit),
            None => output.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &remainder[end + 1..];
    }
    output.push_str(rest);
    output
}

fn source_commits(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    let mut commits = BTreeMap::new();
    for import in &spec.selection.import_sources {
        if let Some(commit) = import
            .identity
            .strip_prefix("git:")
            .and_then(|identity| identity.rsplit_once('@'))
            .map(|(_, commit)| commit.to_string())
        {
            commits.insert(import.id.clone(), commit);
        }
    }
    for source in &spec.sources {
        let commit = match &source.definition {
            SourceDefinition::Git(git) => git
                .locked_commit
                .clone()
                .or_else(|| git.rev.clone().filter(|rev| is_full_sha(rev))),
            SourceDefinition::Path(path) => {
                gaia_spec::resolve_workspace_path(&spec.workspace, &path.path)
                    .ok()
                    .and_then(|dir| git_head(&dir))
            }
            _ => None,
        };
        if let Some(commit) = commit {
            commits.insert(source.id.as_str().to_string(), commit);
        }
    }
    commits
}

fn is_full_sha(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn git_head(dir: &Path) -> Option<String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let head = git(&["rev-parse", "HEAD"]).filter(|head| is_full_sha(head))?;
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|status| !status.is_empty());
    Some(if dirty { format!("{head}-dirty") } else { head })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_known_source_commits_only() {
        let commits = BTreeMap::from([("atlas".to_string(), "a".repeat(40))]);
        assert_eq!(
            substitute(
                "device_package.commit=${source.atlas.commit} other=${source.orion.commit} v=${x}",
                &commits
            ),
            format!(
                "device_package.commit={} other=${{source.orion.commit}} v=${{x}}",
                "a".repeat(40)
            )
        );
    }

    #[test]
    fn full_shas_only() {
        assert!(is_full_sha(&"0123456789abcdef".repeat(2).repeat(2)[..40]));
        assert!(!is_full_sha("main"));
        assert!(!is_full_sha("abc123"));
    }
}
