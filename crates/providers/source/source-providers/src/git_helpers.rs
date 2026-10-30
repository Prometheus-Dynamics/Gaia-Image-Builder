use super::*;
pub(crate) use gaia_process::git_command;

pub(crate) fn is_local_git_repo(repo: &str) -> bool {
    repo.starts_with("file://") || Path::new(repo).exists()
}

/// Origin URL a checkout records: the repository as configured, with any
/// credentials redacted. A local repository is named by its path.
pub(crate) fn git_checkout_origin(git: &GitSourceSpec) -> String {
    let repo = git.repo.strip_prefix("file://").unwrap_or(&git.repo);
    redact_url_userinfo(repo)
}

/// The revision a local git source checks out, resolved in the repository
/// itself. The lock wins, then `rev`, `tag` and `branch`, as before.
pub(crate) fn local_git_target(git: &GitSourceSpec) -> String {
    if let Some(locked) = &git.locked_commit {
        locked.clone()
    } else if let Some(rev) = &git.rev {
        rev.clone()
    } else if let Some(tag) = &git.tag {
        format!("refs/tags/{tag}")
    } else if let Some(branch) = &git.branch {
        format!("refs/heads/{branch}")
    } else {
        "HEAD".into()
    }
}

/// A full commit id names one commit forever, so a repository that already
/// holds it needs no fetch to check it out.
pub(crate) fn is_full_commit_id(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The commit `target` (a commit id or a ref name) peels to in `repo`.
pub(crate) fn resolve_commit_in(repo: &Path, target: &str) -> Option<String> {
    let output = git_command()
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{target}^{{commit}}"))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !commit.is_empty()).then_some(commit)
}

/// Whether `repo` holds the commit object `commit`, directly or through its
/// alternates (a checkout of a mirror reads the mirror's objects that way).
fn holds_commit(repo: &Path, commit: &str) -> bool {
    git_command()
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e"])
        .arg(format!("{commit}^{{commit}}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Where and how the git commands of one source run.
pub(crate) struct GitRun<'a> {
    pub(crate) execution: &'a SourceExecutionContext,
    pub(crate) policy: SourceCommandPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

impl GitRun<'_> {
    /// Runs a git command that must succeed, under the source's retry policy.
    pub(crate) fn run(&self, command: Command, label: &str) -> Result<(), SourceProviderError> {
        run_command_with_policy(
            command,
            self.execution,
            label,
            self.policy,
            self.log_sink.clone(),
            self.cancel_check.clone(),
        )
    }

    /// Runs a git command and returns its output, for `ls-remote`.
    pub(crate) fn output(
        &self,
        command: &mut Command,
        label: &str,
    ) -> Result<Output, SourceProviderError> {
        command_output_with_timeout(
            command,
            self.execution,
            Duration::from_secs(self.policy.timeout_seconds.max(1)),
            label,
            self.policy.output_retention,
            self.log_sink.clone(),
            self.cancel_check.clone(),
        )
    }
}

/// When a remote git source fetches its mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MirrorRefresh<'a> {
    /// Fetch unless the mirror already holds this full commit id. A failed
    /// fetch is only a warning: the checkout then fails if the commit is
    /// still missing.
    IfMissing(&'a str),
    /// Fetch every time, because the wanted ref may have moved. A failed
    /// fetch is an error, so nothing is resolved from a stale mirror.
    Always,
    /// Never fetch (offline): the mirror as it is, and an error when there is
    /// none.
    Never,
}

/// Runs `fetch` for an existing mirror as `refresh` asks. It returns without
/// fetching when the mirror already holds the wanted commit.
pub(crate) fn refresh_mirror_with<F>(
    mirror: &Path,
    refresh: MirrorRefresh,
    fetch: F,
) -> Result<(), SourceProviderError>
where
    F: FnOnce() -> Result<(), SourceProviderError>,
{
    match refresh {
        MirrorRefresh::IfMissing(commit) => {
            if holds_commit(mirror, commit) {
                return Ok(());
            }
            if let Err(error) = fetch() {
                tracing::warn!(
                    mirror = %mirror.display(),
                    error = %error.message,
                    "git cache refresh failed"
                );
            }
            Ok(())
        }
        MirrorRefresh::Always => fetch(),
        MirrorRefresh::Never => Ok(()),
    }
}

/// The mirror of a remote repository under `.gaia/cache/git`: created on
/// first use, and refreshed as `refresh` asks.
pub(crate) fn ensure_remote_git_cache(
    git: &GitSourceSpec,
    run: &GitRun,
    refresh: MirrorRefresh,
) -> Result<PathBuf, SourceProviderError> {
    let cache_dir = run
        .execution
        .workspace_root
        .join(".gaia")
        .join("cache")
        .join("git");
    fs::create_dir_all(&cache_dir).map_err(|error| {
        SourceProviderError::runtime_state(format!(
            "failed to create git source cache dir '{}': {error}",
            cache_dir.display()
        ))
    })?;
    let mirror_dir = remote_git_mirror_dir(&cache_dir, &git.repo);
    if mirror_dir.join("HEAD").is_file() {
        // Refreshing keeps the mirror supplying objects for new commits.
        refresh_mirror_with(&mirror_dir, refresh, || {
            let mut fetch = git_command();
            fetch
                .arg("-C")
                .arg(&mirror_dir)
                .arg("fetch")
                .arg("--prune")
                .arg("origin");
            run.run(fetch, "refresh remote git source cache")
        })?;
        return Ok(mirror_dir);
    }
    if refresh == MirrorRefresh::Never {
        return Err(SourceProviderError::backend_command(format!(
            "offline: no git mirror of '{}' in '{}'; run 'gaia prefetch' (or 'gaia run --prefetch') online first",
            redact_url_userinfo(&git.repo),
            mirror_dir.display()
        )));
    }
    let mut clone = git_command();
    clone
        .arg("clone")
        .arg("--mirror")
        .arg(&git.repo)
        .arg(&mirror_dir);
    run.run(clone, "clone remote git source cache")?;
    Ok(mirror_dir)
}

/// The repository a checkout takes its commits from.
pub(crate) enum CheckoutSource {
    /// A remote repository's mirror. A new checkout shares the mirror's
    /// objects instead of copying them.
    Mirror(PathBuf),
    /// A local repository, which a new checkout clones by hard links.
    Local(PathBuf),
}

impl CheckoutSource {
    fn path(&self) -> &Path {
        match self {
            Self::Mirror(path) | Self::Local(path) => path,
        }
    }
}

/// Makes `output_dir` a detached checkout of `target`, resolved in `source`.
/// An existing checkout of the same repository is updated in place. Any
/// failure there clones the checkout again.
pub(crate) fn checkout_git_commit(
    git: &GitSourceSpec,
    output_dir: &Path,
    source: &CheckoutSource,
    target: &str,
    run: &GitRun,
) -> Result<(), SourceProviderError> {
    let commit = resolve_commit_in(source.path(), target).ok_or_else(|| {
        SourceProviderError::backend_command(format!(
            "git source '{}' has no commit for '{target}' in '{}'",
            git.repo,
            source.path().display()
        ))
    })?;
    let origin = git_checkout_origin(git);
    if let Err(error) = update_checkout_in_place(output_dir, source.path(), &origin, &commit, run) {
        tracing::debug!(
            checkout = %output_dir.display(),
            error = %error.message,
            "git checkout not updated in place; cloning it again"
        );
        clone_checkout(
            matches!(source, CheckoutSource::Mirror(_)),
            output_dir,
            source.path(),
            &origin,
            &commit,
            run,
        )?;
    }
    let head = git_head_commit(output_dir).unwrap_or_default();
    if head != commit {
        return Err(SourceProviderError::backend_command(format!(
            "git source '{}' checked out '{head}' instead of '{commit}'",
            git.repo
        )));
    }
    write_selected_subdir_marker(git, output_dir)
}

/// Moves an existing checkout of the same origin to `commit`: fetches the
/// commit's objects when the checkout lacks them, forces the tree to it and
/// removes untracked files. Gaia's `.gaia` state is moved aside by the caller
/// and is excluded from the removal anyway.
fn update_checkout_in_place(
    output_dir: &Path,
    source: &Path,
    origin: &str,
    commit: &str,
    run: &GitRun,
) -> Result<(), SourceProviderError> {
    if !output_dir.join(".git").exists() {
        return Err(SourceProviderError::runtime_state(format!(
            "'{}' has no git checkout to update",
            output_dir.display()
        )));
    }
    let recorded = git_command()
        .arg("-C")
        .arg(output_dir)
        .args(["config", "--get", "remote.origin.url"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    if recorded.as_deref() != Some(origin) {
        return Err(SourceProviderError::runtime_state(format!(
            "'{}' is a checkout of another repository",
            output_dir.display()
        )));
    }
    if git_head_commit(output_dir).as_deref() == Some(commit)
        && git_status_ignored(output_dir).is_some_and(|status| {
            status.lines().all(|line| {
                line.strip_prefix("?? ")
                    .is_some_and(is_checkout_marker_path)
            })
        })
    {
        // Already at the commit with nothing but Gaia's markers around, so
        // there is nothing to check out or clean. Markers that are only
        // written sometimes are removed; the caller writes them again if
        // they still apply.
        for name in CHECKOUT_OPTIONAL_MARKERS {
            let _ = fs::remove_file(output_dir.join(name));
        }
        return Ok(());
    }
    if !holds_commit(output_dir, commit) {
        // A checkout of a mirror reads the mirror's objects through its
        // alternates, so a commit missing here came in with a later fetch of
        // the source: fetch the source's refs to bring it in.
        let mut fetch = git_command();
        fetch
            .arg("-C")
            .arg(output_dir)
            .args(["fetch", "--quiet", "--no-tags", "--prune"])
            .arg(source)
            .arg("+refs/*:refs/gaia/source/*");
        run.run(fetch, "fetch git source objects")?;
    }
    let mut checkout = git_command();
    checkout
        .arg("-C")
        .arg(output_dir)
        .args(["checkout", "--quiet", "--force", "--detach"])
        .arg(commit);
    run.run(checkout, "checkout git commit")?;
    let mut clean = git_command();
    clean
        .arg("-C")
        .arg(output_dir)
        .args(["clean", "--quiet", "-ffdx", "-e", ".gaia"]);
    run.run(clean, "clean git checkout")?;
    let clean_tree = git_status_porcelain(output_dir).is_some_and(|status| status.is_empty());
    if git_head_commit(output_dir).as_deref() != Some(commit) || !clean_tree {
        return Err(SourceProviderError::runtime_state(format!(
            "'{}' did not check out '{commit}' cleanly",
            output_dir.display()
        )));
    }
    Ok(())
}

/// Clones the checkout afresh, emptying `output_dir` first. With `shared` the
/// checkout reads the mirror's objects instead of copying them; a local
/// repository is cloned by hard links, which is as cheap and independent of
/// the repository's later history.
fn clone_checkout(
    shared: bool,
    output_dir: &Path,
    source: &Path,
    origin: &str,
    commit: &str,
    run: &GitRun,
) -> Result<(), SourceProviderError> {
    prepare_materialized_dir(output_dir)?;
    let mut clone = git_command();
    clone.arg("clone").arg("--quiet").arg("--no-checkout");
    if shared {
        clone.arg("--shared");
    }
    clone.arg(source).arg(output_dir);
    run.run(clone, "clone git source")?;
    set_checkout_origin(output_dir, origin)?;
    let mut checkout = git_command();
    checkout
        .arg("-C")
        .arg(output_dir)
        .args(["checkout", "--quiet", "--force", "--detach"])
        .arg(commit);
    run.run(checkout, "checkout git commit")
}

/// Records the origin of a checkout as the configured repository with its
/// credentials redacted. The checkout is never fetched from by that URL.
fn set_checkout_origin(output_dir: &Path, origin: &str) -> Result<(), SourceProviderError> {
    let output = git_command()
        .arg("-C")
        .arg(output_dir)
        .args(["remote", "set-url", "origin"])
        .arg(origin)
        .output()
        .map_err(|error| {
            SourceProviderError::runtime_state(format!(
                "failed to run git remote set-url in '{}': {error}",
                output_dir.display()
            ))
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SourceProviderError::runtime_state(redact_url_userinfo(
            &format!(
                "failed to set the origin URL of '{}': {}",
                output_dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )))
    }
}

/// Files Gaia writes into a git checkout. Untracked, they are not content.
pub(crate) const CHECKOUT_MARKER_FILES: &[&str] = &[
    "source.txt",
    ".gaia-source-state.txt",
    "refs.txt",
    "selected-subdir.txt",
    "remote-resolution-error.txt",
];

/// The markers that are written only for some sources, so a checkout that
/// keeps its files drops them and lets the caller write them again.
pub(crate) const CHECKOUT_OPTIONAL_MARKERS: &[&str] = &[
    "refs.txt",
    "selected-subdir.txt",
    "remote-resolution-error.txt",
];

/// Whether an untracked path is Gaia's: anything under `.gaia`, or one of the
/// marker files at the top of the checkout.
pub(crate) fn is_checkout_marker_path(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or(path);
    first == ".gaia" || (!path.contains('/') && CHECKOUT_MARKER_FILES.contains(&path))
}

/// `git status --porcelain` of `dir`, listing untracked files one by one and
/// not ignored ones. `None` when git fails.
pub(crate) fn git_status_porcelain(dir: &Path) -> Option<String> {
    porcelain_status(dir, "no")
}

/// Like [`git_status_porcelain`], and also lists ignored files.
fn git_status_ignored(dir: &Path) -> Option<String> {
    porcelain_status(dir, "traditional")
}

fn porcelain_status(dir: &Path, ignored: &str) -> Option<String> {
    let output = git_command()
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .arg(format!("--ignored={ignored}"))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_selected_subdir_marker(
    git: &GitSourceSpec,
    output_dir: &Path,
) -> Result<(), SourceProviderError> {
    if let Some(subdir) = &git.subdir {
        fs::write(output_dir.join("selected-subdir.txt"), subdir).map_err(|error| {
            SourceProviderError::runtime_state(format!(
                "failed to write selected git subdir marker '{}': {error}",
                output_dir.join("selected-subdir.txt").display()
            ))
        })?;
    }
    Ok(())
}

/// One mirror per repository, shared by every branch, tag and revision.
pub(crate) fn remote_git_cache_key(repo: &str) -> String {
    let mut fingerprint = gaia_spec::Fingerprint::new("git-mirror");
    fingerprint.str(repo);
    format!("repo-{}", fingerprint.finish().short_hex())
}

/// Directory name of a repository's mirror under `.gaia/cache/git`.
pub fn remote_git_mirror_dir_name(repo: &str) -> String {
    format!("{}.git", remote_git_cache_key(repo))
}

/// The name mirrors had before stable fingerprints: `std`'s `DefaultHasher`
/// over the URL. Only computed to migrate such a mirror (see
/// [`remote_git_mirror_dir`]); a Rust release that changes that hasher only
/// makes the migration miss, and the mirror is cloned again.
fn legacy_remote_git_mirror_dir_name(repo: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    repo.hash(&mut hasher);
    format!("repo-{:016x}.git", hasher.finish())
}

/// The mirror directory of `repo` under `cache_dir`. A mirror still under its
/// legacy name is renamed to the current one first, so mirrors made by an
/// older Gaia keep serving their objects instead of being cloned again.
pub fn remote_git_mirror_dir(cache_dir: &Path, repo: &str) -> PathBuf {
    let mirror = cache_dir.join(remote_git_mirror_dir_name(repo));
    if !mirror.exists() {
        let legacy = cache_dir.join(legacy_remote_git_mirror_dir_name(repo));
        if legacy != mirror
            && legacy.join("HEAD").is_file()
            && let Err(error) = fs::rename(&legacy, &mirror)
        {
            // Another process may have renamed it first; otherwise the
            // mirror is cloned again under the current name.
            if !mirror.exists() {
                tracing::warn!(
                    legacy = %legacy.display(),
                    mirror = %mirror.display(),
                    %error,
                    "could not rename a git mirror to its current name"
                );
            }
        }
    }
    mirror
}

/// Workspace-relative directory holding the shared git mirrors.
pub const GIT_MIRROR_CACHE_DIR: &str = ".gaia/cache/git";

/// Resolves the commit a git source's configured ref points at, using
/// `git ls-remote` (works for remote URLs, `file://` URLs and local paths).
/// Sources pinned with `rev` resolve to that revision unchanged.
pub fn resolve_git_source_commit(git: &GitSourceSpec, timeout: Duration) -> Result<String, String> {
    if let Some(rev) = &git.rev {
        return Ok(rev.clone());
    }
    let repo = git.repo.as_str();
    let (kind, value) = git_selected_ref(git);
    let mut command = git_command();
    command.arg("ls-remote").arg(repo);
    let wanted: Vec<String> = match kind {
        "branch" => {
            command.arg(format!("refs/heads/{value}"));
            vec![format!("refs/heads/{value}")]
        }
        "tag" => {
            // Annotated tags list the tag object and the peeled commit
            // (`refs/tags/<tag>^{}`); the peeled line only matches a glob.
            command.arg(format!("refs/tags/{value}"));
            command.arg(format!("refs/tags/{value}^*"));
            vec![
                format!("refs/tags/{value}^{{}}"),
                format!("refs/tags/{value}"),
            ]
        }
        _ => {
            command.arg("HEAD");
            vec!["HEAD".into()]
        }
    };
    let shown_repo = redact_url_userinfo(repo);
    let output = gaia_process::run_command_with_timeout(
        &mut command,
        timeout,
        &format!("git ls-remote for '{shown_repo}'"),
        None,
        None,
    )
    .map_err(|error| redact_url_userinfo(&error.message))?
    .output;
    if !output.status.success() {
        return Err(redact_url_userinfo(&format!(
            "git ls-remote failed for '{shown_repo}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let refs = listing
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect::<Vec<_>>();
    wanted
        .iter()
        .find_map(|name| {
            refs.iter()
                .find(|(_, candidate)| candidate == name)
                .map(|(sha, _)| sha.clone())
        })
        .ok_or_else(|| format!("{kind} '{value}' was not found in '{shown_repo}'"))
}

pub(crate) fn resolve_remote_git_refs(
    git: &GitSourceSpec,
    execution: &SourceExecutionContext,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<String, SourceProviderError> {
    let selector = git_selected_ref(git).1;
    let attempts = policy.attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=attempts {
        let mut command = git_command();
        command.arg("ls-remote").arg(&git.repo).arg(selector);
        let output = command_output_with_timeout(
            &mut command,
            execution,
            Duration::from_secs(policy.timeout_seconds.max(1)),
            &format!("git ls-remote for '{}'", redact_url_userinfo(&git.repo)),
            policy.output_retention,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        last_error = Some(redact_url_userinfo(&format!(
            "git ls-remote failed for '{}' on attempt {}/{}: {}",
            git.repo,
            attempt,
            attempts,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
        if attempt < attempts {
            let retry_backoff = retry_backoff_duration(
                policy.retry_backoff_strategy,
                policy.retry_backoff_ms,
                attempt,
            );
            if !sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                return Err(SourceProviderError::new(
                    SourceProviderErrorKind::Cancelled,
                    format!(
                        "git ls-remote for '{}' cancelled during retry backoff",
                        git.repo
                    ),
                ));
            }
        }
    }
    Err(SourceProviderError::backend_command(
        last_error.unwrap_or_else(|| {
            format!(
                "git ls-remote failed for '{}' after {} attempt(s)",
                git.repo, attempts
            )
        }),
    ))
}

pub(crate) fn git_selected_ref(git: &GitSourceSpec) -> (&'static str, &str) {
    if let Some(branch) = git.branch.as_deref() {
        ("branch", branch)
    } else if let Some(tag) = git.tag.as_deref() {
        ("tag", tag)
    } else if let Some(rev) = git.rev.as_deref() {
        ("rev", rev)
    } else {
        ("head", "HEAD")
    }
}

pub(crate) fn git_head_commit(repo_dir: &Path) -> Option<String> {
    let output = git_command()
        .arg("-C")
        .arg(repo_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether `git` runs on the host. Probed once per process.
pub(crate) fn host_git_available() -> bool {
    static HOST_GIT: OnceLock<bool> = OnceLock::new();
    *HOST_GIT.get_or_init(|| {
        Command::new("git")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Execution context for the git commands of one git source, and a note
/// when the configured backend could not be honoured.
pub(crate) fn git_command_execution(
    spec: &ResolvedBuildSpec,
    git: &GitSourceSpec,
    source_id: &str,
) -> (SourceExecutionContext, Option<String>) {
    git_command_execution_with(
        execution_context(spec),
        git,
        source_id,
        host_git_available(),
    )
}

/// Pure selection rule behind [`git_command_execution`]. With
/// `git_backend = "host"` (the default) git runs on the host even when the
/// build executes in docker, so it sees the user's credentials. It falls
/// back to the execution backend only when host git is missing.
pub(crate) fn git_command_execution_with(
    mut execution: SourceExecutionContext,
    git: &GitSourceSpec,
    source_id: &str,
    host_git: bool,
) -> (SourceExecutionContext, Option<String>) {
    if git.git_backend == GitBackendSpec::Execution || execution.docker.is_none() {
        return (execution, None);
    }
    if host_git {
        execution.docker = None;
        return (execution, None);
    }
    let note = format!(
        "git is not installed on the host, so git for source '{source_id}' runs through the docker execution backend (host git credentials are not available there)"
    );
    (execution, Some(note))
}

pub(crate) fn parse_resolved_remote_ref(contents: &str) -> Option<(String, String)> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?.to_string();
        let name = parts.next().unwrap_or("HEAD").to_string();
        Some((sha, name))
    })
}

pub(crate) fn sanitize_state_value(value: &str) -> String {
    value.replace('\n', "\\n")
}
