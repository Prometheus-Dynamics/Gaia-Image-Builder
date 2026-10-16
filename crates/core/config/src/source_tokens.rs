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
//!
//! `${source.<id>.commit_short}`: the first 8 characters of that commit (a
//! `-dirty` suffix, if any, is kept).
//!
//! `${source.<id>.path}`: the directory holding the source's files, so a
//! Buildroot setting can name a file inside a source (for example
//! `BR2_ROOTFS_USERS_TABLES`). See [`gaia_spec::source_checkout_dir`]: an
//! import source's checkout, a path source's directory, or
//! `<build_dir>/sources/<id>` for sources Gaia materializes. The planner
//! makes the image depend on a source whose directory an override names.
//!
//! Both tokens are substituted in stage env set values, build labels,
//! Buildroot `config_overrides` values and Java artifacts' build target,
//! arguments, command and environment. With build variants, a source a
//! variant checks out for itself resolves to that variant's checkout
//! (`<build_dir>/sources/variants/<variant>/<id>`); a source every variant
//! shares resolves to the shared one.
//!
//! An artifact whose build fields name another source this way depends on
//! that source (its `extra_sources`), so the artifact rebuilds when the
//! source's content changes even though the substituted path stays the same.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

use gaia_spec::{
    ArtifactDefinition, ImageDefinition, ResolvedBuildSpec, SourceDefinition, SourceId,
};

const PREFIX: &str = "source.";
const COMMIT_SUFFIX: &str = ".commit";
const COMMIT_SHORT_SUFFIX: &str = ".commit_short";
const PATH_SUFFIX: &str = ".path";

/// The first 8 characters of a commit, keeping a `-dirty` suffix.
pub(crate) fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
}

/// Substitutes `${source.<id>.commit}` and `${source.<id>.path}`, and drops
/// the tokens it resolved from the unresolved list.
pub(crate) fn substitute_source_tokens(spec: &mut ResolvedBuildSpec) {
    let values = source_token_values(spec);
    if values.is_empty() {
        return;
    }
    for env_set in &mut spec.stage.env_sets {
        for (_, value) in &mut env_set.entries {
            *value = substitute(value, &values, &mut BTreeSet::new());
        }
    }
    for (_, value) in &mut spec.metadata.labels {
        *value = substitute(value, &values, &mut BTreeSet::new());
    }
    if let ImageDefinition::Buildroot(buildroot) = &mut spec.image.definition {
        for (_, value) in &mut buildroot.config_overrides {
            *value = substitute(value, &values, &mut BTreeSet::new());
        }
    }
    let declared = spec
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<BTreeSet<SourceId>>();
    for artifact in &mut spec.artifacts {
        let mut referenced = BTreeSet::new();
        if let ArtifactDefinition::Java(java) = &mut artifact.definition {
            java.build_target = substitute(&java.build_target, &values, &mut referenced);
            for value in java.build_args.iter_mut().chain(&mut java.build_command) {
                *value = substitute(value, &values, &mut referenced);
            }
            for (_, value) in &mut java.build_env {
                *value = substitute(value, &values, &mut referenced);
            }
        }
        add_extra_sources(artifact, referenced, &declared);
    }
    spec.policy
        .interpolation
        .unresolved
        .retain(|unresolved| !values.contains_key(&unresolved.token));
}

/// The source id of a `source.<id>.commit` or `source.<id>.path` token.
fn source_token_id(token: &str) -> Option<&str> {
    let rest = token.strip_prefix(PREFIX)?;
    rest.strip_suffix(COMMIT_SUFFIX)
        .or_else(|| rest.strip_suffix(COMMIT_SHORT_SUFFIX))
        .or_else(|| rest.strip_suffix(PATH_SUFFIX))
}

/// Records the artifact's extra sources: the declared sources its build
/// fields name, other than its own `source`. Sorted, without duplicates.
fn add_extra_sources(
    artifact: &mut gaia_spec::ArtifactSpec,
    referenced: BTreeSet<SourceId>,
    declared: &BTreeSet<SourceId>,
) {
    let own = artifact.source.as_ref().map(|source| &source.id);
    let mut extra = artifact
        .extra_sources
        .iter()
        .cloned()
        .chain(
            referenced
                .into_iter()
                .filter(|id| declared.contains(id) && Some(id) != own),
        )
        .collect::<BTreeSet<_>>();
    extra.retain(|id| Some(id) != own);
    artifact.extra_sources = extra.into_iter().collect();
}

/// Replaces every `${token}` with a known value; anything else is kept.
/// Every source id named by a source token (resolved or not) is added to
/// `referenced`.
fn substitute(
    value: &str,
    values: &BTreeMap<String, String>,
    referenced: &mut BTreeSet<SourceId>,
) -> String {
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
        if let Some(id) = source_token_id(token) {
            referenced.insert(SourceId::new(id));
        }
        match values.get(token).filter(|_| is_source_token(token)) {
            Some(resolved) => output.push_str(resolved),
            None => output.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &remainder[end + 1..];
    }
    output.push_str(rest);
    output
}

fn is_source_token(token: &str) -> bool {
    source_token_id(token).is_some()
}

/// Every resolvable token, keyed without `${}` (`source.<id>.commit`).
fn source_token_values(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (id, commit) in source_commits(spec) {
        values.insert(
            format!("{PREFIX}{id}{COMMIT_SHORT_SUFFIX}"),
            short_commit(&commit),
        );
        values.insert(format!("{PREFIX}{id}{COMMIT_SUFFIX}"), commit);
    }
    for import in &spec.selection.import_sources {
        values.insert(
            format!("{PREFIX}{}{PATH_SUFFIX}", import.id),
            import.root.clone(),
        );
    }
    for source in &spec.sources {
        values.insert(
            format!("{PREFIX}{}{PATH_SUFFIX}", source.id.as_str()),
            gaia_spec::source_checkout_dir(spec, source)
                .display()
                .to_string(),
        );
    }
    values
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
    fn substitutes_known_source_tokens_only() {
        let values = BTreeMap::from([
            ("source.atlas.commit".to_string(), "a".repeat(40)),
            ("source.atlas.path".to_string(), "/work/atlas".to_string()),
            ("x".to_string(), "not a source token".to_string()),
        ]);
        let mut referenced = BTreeSet::new();
        assert_eq!(
            substitute(
                "device_package.commit=${source.atlas.commit} other=${source.orion.commit} v=${x}",
                &values,
                &mut referenced
            ),
            format!(
                "device_package.commit={} other=${{source.orion.commit}} v=${{x}}",
                "a".repeat(40)
            )
        );
        assert_eq!(
            substitute(
                "\"${source.atlas.path}/users.table ${source.orion.path}/x\"",
                &values,
                &mut referenced
            ),
            "\"/work/atlas/users.table ${source.orion.path}/x\""
        );
        // Named by a source token, resolved or not; `${x}` is not a source.
        assert_eq!(
            referenced,
            BTreeSet::from([SourceId::new("atlas"), SourceId::new("orion")])
        );
    }

    #[test]
    fn extra_sources_are_declared_sources_other_than_the_artifact_source() {
        use gaia_spec::{
            ArtifactDefinition, ArtifactOutputSpec, ArtifactSpec, JavaArtifactSpec, SourceRef,
        };
        let mut artifact = ArtifactSpec::new(
            "app",
            ArtifactDefinition::Java(JavaArtifactSpec {
                build_target: String::new(),
                build_args: Vec::new(),
                build_command: Vec::new(),
                build_env: Vec::new(),
            }),
            Some(SourceRef::new("atlas")),
            ArtifactOutputSpec {
                path: "app.jar".into(),
            },
        );
        artifact.extra_sources = vec![SourceId::new("zeta")];
        let declared = BTreeSet::from([
            SourceId::new("atlas"),
            SourceId::new("eidos"),
            SourceId::new("zeta"),
        ]);
        let referenced = BTreeSet::from([
            SourceId::new("atlas"),
            SourceId::new("eidos"),
            SourceId::new("orion"),
        ]);
        add_extra_sources(&mut artifact, referenced, &declared);
        assert_eq!(
            artifact.extra_sources,
            vec![SourceId::new("eidos"), SourceId::new("zeta")]
        );
    }

    #[test]
    fn full_shas_only() {
        assert!(is_full_sha(&"0123456789abcdef".repeat(2).repeat(2)[..40]));
        assert!(!is_full_sha("main"));
        assert!(!is_full_sha("abc123"));
    }

    #[test]
    fn commit_short_is_eight_characters_and_keeps_the_dirty_marker() {
        assert_eq!(short_commit(&"0123456789abcdef".repeat(2)), "01234567");
        assert_eq!(short_commit("0123456789abcdef-dirty"), "01234567");
        assert_eq!(source_token_id("source.atlas.commit_short"), Some("atlas"));
        assert_eq!(source_token_id("source.atlas.commit"), Some("atlas"));
        let values = BTreeMap::from([(
            "source.atlas.commit_short".to_string(),
            "01234567".to_string(),
        )]);
        let mut referenced = BTreeSet::new();
        assert_eq!(
            substitute("rev=${source.atlas.commit_short}", &values, &mut referenced),
            "rev=01234567"
        );
        assert!(referenced.contains(&SourceId::new("atlas")));
    }
}
