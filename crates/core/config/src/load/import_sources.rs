//! Git sources used by `imports = [{ source = "<id>", path = "..." }]`.
//!
//! Import sources are resolved while config files are loaded, before any
//! build runs:
//! - The source must be a `kind = "git"` entry of `[[sources]]` declared in
//!   a local config file (the entrypoint, its `extends` chain or a layer it
//!   imports by plain path). Files read from a source cannot declare import
//!   sources themselves.
//! - Its commit comes from `rev`, or from the build's lockfile.
//! - The checkout lives in `<workspace>/.gaia/cache/import-sources/<id>-<rev>`
//!   and is never fetched again once present.
//! - `--set sources.<id>.path=<dir>` reads the files from `<dir>` instead.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use gaia_spec::{
    DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS, GitSourceSpec, ImportSourceSpec, SourcePinPolicySpec,
    SourceRefreshPolicySpec,
};

use crate::ConfigError;
use crate::lockfile::{GitLockStatus, GitLockfile};

pub(super) struct ImportSources {
    workspace_root: PathBuf,
    lockfile_path: PathBuf,
    lockfile: Option<GitLockfile>,
    declared: BTreeMap<String, Declaration>,
    path_overrides: BTreeMap<String, PathBuf>,
    resolve_unpinned: bool,
    resolved: BTreeMap<String, Resolved>,
}

struct Declaration {
    git: GitSourceSpec,
    file: PathBuf,
    conflict: Option<PathBuf>,
}

struct Resolved {
    root: PathBuf,
    identity: String,
    path_override: bool,
    contributes: BTreeSet<String>,
    config_digest: DefaultHasher,
}

impl ImportSources {
    pub(super) fn new(
        workspace_root: PathBuf,
        entrypoint: &Path,
        path_overrides: BTreeMap<String, PathBuf>,
        resolve_unpinned: bool,
    ) -> Self {
        let lockfile_path = crate::lockfile::lockfile_path_for_build_file(entrypoint);
        let lockfile = match GitLockfile::load(&lockfile_path) {
            Ok(lock) => lock,
            Err(error) => {
                tracing::warn!(%error, "ignoring unreadable lockfile for import sources");
                None
            }
        };
        Self {
            workspace_root,
            lockfile_path,
            lockfile,
            declared: BTreeMap::new(),
            path_overrides,
            resolve_unpinned,
            resolved: BTreeMap::new(),
        }
    }

    /// Registers the git `[[sources]]` of a local config file.
    pub(super) fn declare_from(&mut self, file: &Path, value: &toml::Value) {
        let Some(sources) = value.get("sources").and_then(toml::Value::as_array) else {
            return;
        };
        for source in sources {
            let field = |name: &str| {
                source
                    .get(name)
                    .and_then(toml::Value::as_str)
                    .map(ToString::to_string)
            };
            if field("kind").as_deref() != Some("git") {
                continue;
            }
            let (Some(id), Some(repo)) = (field("id"), field("repo")) else {
                continue;
            };
            let git = GitSourceSpec {
                repo,
                branch: field("branch"),
                tag: field("tag"),
                rev: field("rev"),
                subdir: field("subdir"),
                update: false,
                refresh_policy: SourceRefreshPolicySpec::Auto,
                pin_policy: SourcePinPolicySpec::Locked,
                locked_commit: None,
            };
            match self.declared.get_mut(&id) {
                Some(existing) if existing.git != git && existing.conflict.is_none() => {
                    existing.conflict = Some(file.to_path_buf());
                }
                Some(_) => {}
                None => {
                    self.declared.insert(
                        id,
                        Declaration {
                            git,
                            file: file.to_path_buf(),
                            conflict: None,
                        },
                    );
                }
            }
        }
    }

    /// Directory the files of source `id` are read from, checking it out
    /// on first use.
    pub(super) fn root(&mut self, id: &str, referenced_by: &Path) -> Result<PathBuf, ConfigError> {
        if let Some(resolved) = self.resolved.get(id) {
            return Ok(resolved.root.clone());
        }
        let resolved = match self.path_overrides.get(id) {
            Some(dir) => {
                let root = fs::canonicalize(dir).map_err(|error| {
                    import_error(
                        id,
                        referenced_by,
                        format!(
                            "local override 'sources.{id}.path' = '{}' is not readable: {error}",
                            dir.display()
                        ),
                    )
                })?;
                Resolved::new(format!("path:{}", root.display()), root, true)
            }
            None => self.checkout(id, referenced_by)?,
        };
        let root = resolved.root.clone();
        tracing::debug!(source = id, root = %root.display(), "resolved import source");
        self.resolved.insert(id.to_string(), resolved);
        Ok(root)
    }

    fn checkout(&self, id: &str, referenced_by: &Path) -> Result<Resolved, ConfigError> {
        let declaration = self.declared.get(id).ok_or_else(|| {
            import_error(
                id,
                referenced_by,
                format!(
                    "no git source '{id}' is declared in a local config file; declare it with \
                     `[[sources]] id = \"{id}\" kind = \"git\"` in the build entrypoint or a \
                     layer it imports by path"
                ),
            )
        })?;
        if let Some(other) = &declaration.conflict {
            return Err(import_error(
                id,
                referenced_by,
                format!(
                    "source '{id}' is declared differently in '{}' and '{}'",
                    declaration.file.display(),
                    other.display()
                ),
            ));
        }
        let git = &declaration.git;
        let timeout = Duration::from_secs(DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS);
        let rev = self.pinned_rev(id, git, referenced_by, timeout)?;
        let dest = self
            .workspace_root
            .join(gaia_source_providers::IMPORT_SOURCE_CACHE_DIR)
            .join(format!("{id}-{}", sanitize(&rev)));
        let commit = gaia_source_providers::ensure_git_revision_checkout(
            &self.workspace_root,
            &git.repo,
            &rev,
            &dest,
            timeout,
        )
        .map_err(|error| {
            import_error(
                id,
                referenced_by,
                format!("failed to check out '{}' at '{rev}': {error}", git.repo),
            )
        })?;
        let root = fs::canonicalize(&dest).map_err(|error| {
            import_error(
                id,
                referenced_by,
                format!("checkout '{}' is not readable: {error}", dest.display()),
            )
        })?;
        Ok(Resolved::new(
            format!("git:{}@{commit}", git.repo),
            root,
            false,
        ))
    }

    fn pinned_rev(
        &self,
        id: &str,
        git: &GitSourceSpec,
        referenced_by: &Path,
        timeout: Duration,
    ) -> Result<String, ConfigError> {
        if let Some(rev) = &git.rev {
            return Ok(rev.clone());
        }
        let stale = match self.lockfile.as_ref().map(|lock| lock.status(id, git)) {
            Some(GitLockStatus::Locked(entry)) => return Ok(entry.commit.clone()),
            Some(GitLockStatus::Stale(_)) => " (its entry is stale)",
            _ => "",
        };
        if self.resolve_unpinned {
            return gaia_source_providers::resolve_git_source_commit(git, timeout).map_err(
                |error| import_error(id, referenced_by, format!("failed to resolve: {error}")),
            );
        }
        Err(import_error(
            id,
            referenced_by,
            format!(
                "import sources must be pinned, but '{id}' has no `rev` and no entry in \
                 lockfile '{}'{stale}; add `rev = \"<commit>\"` to the source or run \
                 `gaia lock <build>`",
                self.lockfile_path.display()
            ),
        ))
    }

    /// Resolves the `path` of a `{ source = .., path = .. }` import.
    pub(super) fn import_path(
        &mut self,
        id: &str,
        path: &str,
        referenced_by: &Path,
    ) -> Result<PathBuf, ConfigError> {
        let root = self.root(id, referenced_by)?;
        if Path::new(path).is_absolute() {
            return Err(import_error(
                id,
                referenced_by,
                format!("import path '{path}' must be relative to the source checkout"),
            ));
        }
        let resolved = root.join(path);
        self.ensure_inside(id, &resolved, referenced_by)?;
        Ok(resolved)
    }

    /// Rejects paths that leave the checkout of `id` (via `..` or symlinks).
    pub(super) fn ensure_inside(
        &mut self,
        id: &str,
        path: &Path,
        referenced_by: &Path,
    ) -> Result<(), ConfigError> {
        let root = self.root(id, referenced_by)?;
        let escapes = || {
            import_error(
                id,
                referenced_by,
                format!(
                    "'{}' is outside the checkout '{}'; files imported from a source may only \
                     import files from the same checkout",
                    path.display(),
                    root.display()
                ),
            )
        };
        if !lexically_normalized(path).starts_with(&root) {
            return Err(escapes());
        }
        if let Ok(canonical) = fs::canonicalize(path)
            && !canonical.starts_with(&root)
        {
            return Err(escapes());
        }
        Ok(())
    }

    /// Records what a file read from source `id` contributes to the build.
    pub(super) fn record_file(&mut self, id: &str, value: &toml::Value, contents: &str) {
        let Some(resolved) = self.resolved.get_mut(id) else {
            return;
        };
        contents.hash(&mut resolved.config_digest);
        let ids = |table: Option<&toml::Value>| {
            table
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("id").and_then(toml::Value::as_str))
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        let stage = value.get("stage");
        for (kind, items) in [
            ("source", ids(value.get("sources"))),
            ("artifact", ids(value.get("artifacts"))),
            ("install", ids(value.get("install"))),
            (
                "stage-file",
                ids(stage.and_then(|stage| stage.get("files"))),
            ),
            (
                "stage-env-set",
                ids(stage.and_then(|stage| stage.get("env_sets"))),
            ),
            (
                "stage-service",
                ids(stage.and_then(|stage| stage.get("services"))),
            ),
        ] {
            for item in items {
                resolved.contributes.insert(format!("{kind}:{item}"));
            }
        }
        if value.get("image").is_some() {
            resolved.contributes.insert("image".into());
        }
    }

    pub(super) fn into_specs(self) -> Vec<ImportSourceSpec> {
        self.resolved
            .into_iter()
            .map(|(id, resolved)| ImportSourceSpec {
                id,
                root: resolved.root.display().to_string(),
                identity: if resolved.path_override {
                    format!(
                        "{}#{:016x}",
                        resolved.identity,
                        resolved.config_digest.finish()
                    )
                } else {
                    resolved.identity
                },
                contributes: resolved.contributes.into_iter().collect(),
            })
            .collect()
    }
}

impl Resolved {
    fn new(identity: String, root: PathBuf, path_override: bool) -> Self {
        Self {
            root,
            identity,
            path_override,
            contributes: BTreeSet::new(),
            config_digest: DefaultHasher::new(),
        }
    }
}

fn import_error(id: &str, referenced_by: &Path, message: String) -> ConfigError {
    ConfigError::ImportSource {
        source_id: id.to_string(),
        referenced_by: referenced_by.display().to_string(),
        message,
    }
}

fn sanitize(rev: &str) -> String {
    rev.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
