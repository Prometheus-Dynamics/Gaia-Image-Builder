//! Cargo git dependencies taken from declared git sources
//! (`cargo_git_sources`).
//!
//! Cargo fetches a git dependency from the URL in its `Cargo.lock`, which
//! needs the user's credentials, and a docker container has none. Gaia
//! instead works on the host with the mirror its git source provider keeps
//! under `.gaia/cache/git`:
//! - every commit the lock pins for the URL is present in the mirror (fetched
//!   on the host when missing);
//! - each pinned commit is reachable from `refs/heads/gaia-locked/<commit>`,
//!   because cargo's CLI fetch only asks for branches and HEAD;
//! - cargo runs with git URL rewriting (`url.<mirror>.insteadOf`) so the lock's
//!   URL is read from the mirror. In docker the mirror is mounted read-only at
//!   its own path.
//!
//! The mirror's refs are pruned by the git source provider's refresh, so each
//! build writes them again before cargo runs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gaia_artifact_providers::{
    ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactProviderError,
    ArtifactProviderErrorKind, ProcessLogLine, ProcessLogSink, ProcessLogStream,
};
use gaia_process::git_command;
use gaia_source_providers::{GIT_MIRROR_CACHE_DIR, remote_git_mirror_dir};
use gaia_spec::{CargoGitSourceSpec, cargo_git_url_key, redact_url_userinfo};

/// Branch namespace in a mirror that keeps each locked commit reachable.
const LOCKED_BRANCH_PREFIX: &str = "refs/heads/gaia-locked/";

/// One lock per mirror, by mirror path. Artifacts built in parallel may share
/// a mirror, and its clone, fetch and ref updates must not interleave; other
/// mirrors are not held up by it.
static MIRROR_LOCKS: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>> = Mutex::new(BTreeMap::new());

/// The lock of `mirror`, created on first use.
fn mirror_lock(mirror: &Path) -> Arc<Mutex<()>> {
    MIRROR_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(mirror.to_path_buf())
        .or_default()
        .clone()
}

/// One `git+<url>?...#<commit>` entry of a Cargo.lock.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LockedGitPin {
    /// The repository URL as the lock records it, without `git+` and query.
    pub(crate) url: String,
    pub(crate) commit: String,
}

/// Every git dependency a Cargo.lock pins, sorted and without duplicates.
pub(crate) fn locked_git_pins(lock: &str) -> Result<Vec<LockedGitPin>, String> {
    let table: toml::Table = lock
        .parse()
        .map_err(|error| format!("Cargo.lock is not valid TOML: {error}"))?;
    let mut pins = Vec::new();
    for package in table
        .get("package")
        .and_then(|packages| packages.as_array())
        .into_iter()
        .flatten()
    {
        let Some(source) = package.get("source").and_then(|source| source.as_str()) else {
            continue;
        };
        let Some(location) = source.strip_prefix("git+") else {
            continue;
        };
        let Some((location, commit)) = location.rsplit_once('#') else {
            continue;
        };
        let url = location.split('?').next().unwrap_or(location);
        pins.push(LockedGitPin {
            url: url.to_string(),
            commit: commit.to_string(),
        });
    }
    pins.sort();
    pins.dedup();
    Ok(pins)
}

/// The URLs and commits the lock pins for `mapping`'s repository, keyed by
/// the URL spelling the lock uses.
fn pins_for(
    pins: &[LockedGitPin],
    mapping: &CargoGitSourceSpec,
) -> BTreeMap<String, BTreeSet<String>> {
    let key = cargo_git_url_key(&mapping.url);
    let mut by_url = BTreeMap::<String, BTreeSet<String>>::new();
    for pin in pins {
        if cargo_git_url_key(&pin.url) == key {
            by_url
                .entry(pin.url.clone())
                .or_default()
                .insert(pin.commit.clone());
        }
    }
    by_url
}

/// Rejects a rewrite that would also catch another repository the lock
/// fetches. Git rewrites by prefix, so `https://h/a` would send
/// `https://h/a-core.git` to the mirror of `https://h/a` unless `a-core`
/// has its own rewrite (git picks the longest match). `rewritten` holds the
/// URLs every mapping of the build rewrites.
fn check_rewrite_prefixes(
    pins: &[LockedGitPin],
    rewritten: &BTreeSet<String>,
) -> Result<(), String> {
    for url in rewritten {
        let collision = pins.iter().map(|pin| pin.url.as_str()).find(|other| {
            *other != url.as_str() && other.starts_with(url.as_str()) && !rewritten.contains(*other)
        });
        if let Some(other) = collision {
            return Err(format!(
                "cargo git dependency '{url}' would be rewritten to its git source's mirror, but the lock also fetches '{other}', which starts with that URL and is not mapped; map '{other}' too or rename the repository"
            ));
        }
    }
    Ok(())
}

fn provider_error(kind: ArtifactProviderErrorKind, message: String) -> ArtifactProviderError {
    ArtifactProviderError::new(kind, message)
}

/// Runs one host git command, returning its stdout. A failure names what
/// was being done, with URL credentials redacted.
fn run_git(
    mut command: Command,
    timeout: Duration,
    what: &str,
    log_sink: Option<&ProcessLogSink>,
) -> Result<String, ArtifactProviderError> {
    let result = gaia_process::run_command_with_timeout(
        &mut command,
        timeout,
        what,
        log_sink.cloned(),
        None,
    )
    .map_err(|error| {
        provider_error(
            ArtifactProviderErrorKind::BackendCommand,
            redact_url_userinfo(&format!("{what} failed: {}", error.message)),
        )
    })?;
    if !result.output.status.success() {
        return Err(provider_error(
            ArtifactProviderErrorKind::BackendCommand,
            redact_url_userinfo(&format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&result.output.stderr).trim()
            )),
        ));
    }
    Ok(String::from_utf8_lossy(&result.output.stdout).into_owned())
}

/// Whether `commit` is an object of the mirror.
fn commit_present(mirror: &Path, commit: &str) -> bool {
    git_command()
        .arg("-C")
        .arg(mirror)
        .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The commit the locked branch for `commit` points at, when it exists.
fn locked_ref_commit(mirror: &Path, commit: &str) -> Option<String> {
    let output = git_command()
        .arg("-C")
        .arg(mirror)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{LOCKED_BRANCH_PREFIX}{commit}"),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A full object name, as the lock records it (40 hex, or 64 for sha256).
fn is_full_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Brings the mirror of `mapping`'s repository up to the commit the lock
/// pins, and makes that commit reachable from a branch.
fn pin_commit(
    mirror: &Path,
    mapping: &CargoGitSourceSpec,
    url: &str,
    commit: &str,
    timeout: Duration,
    log_sink: Option<&ProcessLogSink>,
    offline: bool,
) -> Result<(), ArtifactProviderError> {
    if !is_full_commit(commit) {
        return Err(provider_error(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!("Cargo.lock pins '{url}' at '{commit}', which is not a full commit id"),
        ));
    }
    if !commit_present(mirror, commit) && offline {
        return Err(provider_error(
            ArtifactProviderErrorKind::BackendCommand,
            redact_url_userinfo(&format!(
                "offline: cargo git dependency '{url}' is locked at commit {commit}, which the git mirror '{}' does not contain; run 'gaia prefetch' (or 'gaia run --prefetch') online first",
                mirror.display()
            )),
        ));
    }
    if !commit_present(mirror, commit) {
        let mut fetch = git_command();
        fetch
            .arg("-C")
            .arg(mirror)
            .args(["fetch", "--no-tags", "origin", commit]);
        // The commit may be missing because the mirror predates it; a
        // failed fetch is reported below with the commit it could not get.
        let _ = run_git(
            fetch,
            timeout,
            &format!("fetch locked commit {commit} for '{url}'"),
            log_sink,
        );
        if !commit_present(mirror, commit) {
            return Err(provider_error(
                ArtifactProviderErrorKind::BackendCommand,
                redact_url_userinfo(&format!(
                    "cargo git dependency '{url}' is locked at commit {commit} (Cargo.lock), which git source '{}' ({}) does not contain; check the commit is pushed to that repository and that the source names it",
                    mapping.source.as_str(),
                    mapping.repo
                )),
            ));
        }
    }
    // Every build pins every commit again. A branch that already names the
    // commit needs no write, which would take the mirror's ref lock.
    if locked_ref_commit(mirror, commit).is_some_and(|at| at.eq_ignore_ascii_case(commit)) {
        return Ok(());
    }
    let mut update = git_command();
    update.arg("-C").arg(mirror).args([
        "update-ref",
        &format!("{LOCKED_BRANCH_PREFIX}{commit}"),
        commit,
    ]);
    run_git(
        update,
        timeout,
        &format!("pin locked commit {commit} in the mirror"),
        log_sink,
    )?;
    Ok(())
}

/// Creates the mirror of `repo` when the git source provider has not
/// (a `git clone --mirror`, like the provider's own).
fn ensure_mirror(
    repo: &str,
    mirror: &Path,
    timeout: Duration,
    log_sink: Option<&ProcessLogSink>,
    offline: bool,
) -> Result<(), ArtifactProviderError> {
    if mirror.join("HEAD").is_file() {
        return Ok(());
    }
    if offline {
        return Err(provider_error(
            ArtifactProviderErrorKind::BackendCommand,
            redact_url_userinfo(&format!(
                "offline: no git mirror of '{repo}' in '{}'; run 'gaia prefetch' (or 'gaia run --prefetch') online first",
                mirror.display()
            )),
        ));
    }
    if let Some(parent) = mirror.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            provider_error(
                ArtifactProviderErrorKind::RuntimeState,
                format!("failed to create '{}': {error}", parent.display()),
            )
        })?;
    }
    let mut clone = git_command();
    clone.arg("clone").arg("--mirror").arg(repo).arg(mirror);
    run_git(
        clone,
        timeout,
        &format!("clone git mirror of '{}'", redact_url_userinfo(repo)),
        log_sink,
    )?;
    Ok(())
}

/// `GIT_CONFIG_COUNT` the cargo environment must extend. A host command
/// inherits the user's git config entries, so new entries are appended after
/// them. A docker container gets none of the host environment.
fn inherited_git_config_count(
    contract: &ArtifactExecutionContract,
) -> Result<usize, ArtifactProviderError> {
    if matches!(
        contract.execution_backend,
        ArtifactExecutionBackend::Docker(_)
    ) {
        return Ok(0);
    }
    match std::env::var("GIT_CONFIG_COUNT") {
        Ok(count) => count.trim().parse::<usize>().map_err(|_| {
            provider_error(
                ArtifactProviderErrorKind::RuntimeState,
                format!("GIT_CONFIG_COUNT is not a number: '{count}'"),
            )
        }),
        Err(_) => Ok(0),
    }
}

/// The environment cargo needs to read `rewrites` (URL, mirror) from their
/// mirrors: CLI git fetches, plus one `insteadOf` entry per URL appended
/// after `existing` entries of git config.
fn insteadof_env(rewrites: &[(String, PathBuf)], existing: usize) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "CARGO_NET_GIT_FETCH_WITH_CLI".to_string(),
            "true".to_string(),
        ),
        (
            "GIT_CONFIG_COUNT".to_string(),
            (existing + rewrites.len()).to_string(),
        ),
    ];
    for (index, (url, mirror)) in rewrites.iter().enumerate() {
        let slot = existing + index;
        env.push((
            format!("GIT_CONFIG_KEY_{slot}"),
            format!("url.file://{}.insteadOf", mirror.display()),
        ));
        env.push((format!("GIT_CONFIG_VALUE_{slot}"), url.clone()));
    }
    env
}

/// Prepares every cargo git dependency of a build on the host and returns
/// the environment its cargo run needs. Empty when there are none.
pub(crate) fn cargo_git_env(
    sources: &[CargoGitSourceSpec],
    contract: &ArtifactExecutionContract,
    source_dir: &str,
    log_sink: Option<&ProcessLogSink>,
) -> Result<Vec<(String, String)>, ArtifactProviderError> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let workspace_root = contract
        .workspace_root
        .as_deref()
        .filter(|root| !root.is_empty())
        .ok_or_else(|| {
            provider_error(
                ArtifactProviderErrorKind::RuntimeState,
                "cargo_git_sources needs a resolved workspace root".to_string(),
            )
        })?;
    let lock_path = Path::new(source_dir).join("Cargo.lock");
    let lock = fs::read_to_string(&lock_path).map_err(|error| {
        provider_error(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "cargo_git_sources needs the workspace's Cargo.lock at '{}' (commit it, so the locked commits are known): {error}",
                lock_path.display()
            ),
        )
    })?;
    let pins = locked_git_pins(&lock).map_err(|message| {
        provider_error(
            ArtifactProviderErrorKind::RuntimeState,
            format!("'{}': {message}", lock_path.display()),
        )
    })?;
    let timeout = Duration::from_secs(contract.timeout_seconds.max(1));
    let cache_dir = Path::new(workspace_root).join(GIT_MIRROR_CACHE_DIR);
    // The repositories this build fetches that some mapping serves, with the
    // URLs and commits the lock pins for each.
    let planned = sources
        .iter()
        .filter_map(|mapping| {
            let by_url = pins_for(&pins, mapping);
            (!by_url.is_empty()).then_some((mapping, by_url))
        })
        .collect::<Vec<_>>();
    let rewritten = planned
        .iter()
        .flat_map(|(_, by_url)| by_url.keys().cloned())
        .collect::<BTreeSet<_>>();
    check_rewrite_prefixes(&pins, &rewritten)
        .map_err(|message| provider_error(ArtifactProviderErrorKind::PolicyBlocked, message))?;
    let mut rewrites = Vec::<(String, PathBuf)>::new();
    for (mapping, by_url) in planned {
        if mapping.repo.trim().is_empty() {
            return Err(provider_error(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "cargo_git_sources maps to source '{}', which is not a git source",
                    mapping.source.as_str()
                ),
            ));
        }
        let mirror = remote_git_mirror_dir(&cache_dir, &mapping.repo);
        let lock = mirror_lock(&mirror);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure_mirror(&mapping.repo, &mirror, timeout, log_sink, contract.offline)?;
        gaia_process::register_read_only_docker_mount(&mirror);
        for (url, commits) in by_url {
            for commit in &commits {
                pin_commit(
                    &mirror,
                    mapping,
                    &url,
                    commit,
                    timeout,
                    log_sink,
                    contract.offline,
                )?;
            }
            if let Some(sink) = log_sink {
                sink(ProcessLogLine {
                    stream: ProcessLogStream::Stdout,
                    line: format!(
                        "cargo git dependency '{url}' is served from git source '{}' ({})",
                        mapping.source.as_str(),
                        mirror.display()
                    ),
                });
            }
            rewrites.push((url, mirror.clone()));
        }
    }
    if rewrites.is_empty() {
        return Ok(Vec::new());
    }
    let existing = inherited_git_config_count(contract)?;
    Ok(insteadof_env(&rewrites, existing))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"

[[package]]
name = "corelib"
version = "0.3.0"
source = "git+https://example.com/org/Corelib.git?branch=main#1111111111111111111111111111111111111111"

[[package]]
name = "corelib-extra"
version = "0.1.0"
source = "git+https://example.com/org/Corelib.git?rev=abc#2222222222222222222222222222222222222222"

[[package]]
name = "corelib-old"
version = "0.1.0"
source = "git+https://example.com/org/Corelib.git?branch=main#1111111111111111111111111111111111111111"

[[package]]
name = "other"
version = "1.0.0"
source = "git+https://example.com/Other.git#3333333333333333333333333333333333333333"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
"#;

    fn mapping(url: &str) -> CargoGitSourceSpec {
        CargoGitSourceSpec {
            url: url.into(),
            source: gaia_spec::SourceId::new("corelib"),
            repo: "https://example.com/org/Corelib.git".into(),
        }
    }

    #[test]
    fn lock_pins_are_git_sources_only_with_url_and_commit() {
        let pins = locked_git_pins(LOCK).expect("lock parses");
        let shown = pins
            .iter()
            .map(|pin| format!("{} {}", pin.url, &pin.commit[..4]))
            .collect::<Vec<_>>();
        assert_eq!(
            shown,
            [
                "https://example.com/Other.git 3333",
                "https://example.com/org/Corelib.git 1111",
                "https://example.com/org/Corelib.git 2222",
            ]
        );
    }

    #[test]
    fn mapping_matches_lock_urls_with_and_without_dot_git_and_slash() {
        for key in [
            "https://example.com/org/Corelib.git",
            "https://example.com/org/Corelib",
            "https://example.com/org/Corelib/",
        ] {
            let pins = locked_git_pins(LOCK).expect("lock parses");
            let by_url = pins_for(&pins, &mapping(key));
            assert_eq!(
                by_url.keys().map(String::as_str).collect::<Vec<_>>(),
                ["https://example.com/org/Corelib.git"],
                "{key}"
            );
            assert_eq!(by_url.values().next().map(BTreeSet::len), Some(2));
        }
        let pins = locked_git_pins(LOCK).expect("lock parses");
        assert!(pins_for(&pins, &mapping("https://example.com/org/Other")).is_empty());
    }

    #[test]
    fn a_lock_url_that_prefixes_an_unmapped_repository_is_rejected() {
        let pins = vec![
            LockedGitPin {
                url: "https://h/a".into(),
                commit: "1".repeat(40),
            },
            LockedGitPin {
                url: "https://h/a-core".into(),
                commit: "2".repeat(40),
            },
        ];
        let rewritten = BTreeSet::from(["https://h/a".to_string()]);
        let error = check_rewrite_prefixes(&pins, &rewritten).expect_err("prefix collision");
        assert!(error.contains("https://h/a-core"), "{error}");
        // Another mapping rewriting `a-core` too keeps both correct: git
        // picks the longest matching rewrite.
        let both = BTreeSet::from(["https://h/a".to_string(), "https://h/a-core".to_string()]);
        assert!(check_rewrite_prefixes(&pins, &both).is_ok());
    }

    #[test]
    fn offline_cargo_runs_fetch_nothing_and_name_the_prefetch() {
        assert_eq!(
            crate::offline_cargo_env(true),
            vec![("CARGO_NET_OFFLINE", "true")]
        );
        assert!(crate::offline_cargo_env(false).is_empty());

        let scratch = gaia_testkit::TempDir::new("gaia-cargo-git-offline");
        let timeout = Duration::from_secs(5);
        // No mirror: nothing is cloned.
        let missing = scratch.join("missing.git");
        let error = ensure_mirror(
            "https://offline-test.invalid/dep.git",
            &missing,
            timeout,
            None,
            true,
        )
        .expect_err("no mirror offline");
        assert!(error.message.contains("offline:"), "{}", error.message);
        assert!(error.message.contains("gaia prefetch"), "{}", error.message);
        assert!(!missing.exists());

        // A mirror without the locked commit: nothing is fetched to find it.
        let bare = scratch.join("bare.git");
        let init = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .status()
            .expect("git init");
        assert!(init.success());
        let commit = "2".repeat(40);
        let mapping = CargoGitSourceSpec {
            url: "https://offline-test.invalid/dep.git".into(),
            source: gaia_spec::SourceId::new("dep"),
            repo: "https://offline-test.invalid/dep.git".into(),
        };
        let error = pin_commit(&bare, &mapping, &mapping.url, &commit, timeout, None, true)
            .expect_err("a missing locked commit offline");
        assert!(error.message.contains("offline:"), "{}", error.message);
        assert!(error.message.contains(&commit), "{}", error.message);
    }

    #[test]
    fn insteadof_env_starts_at_zero_or_appends_after_existing_entries() {
        let rewrites = vec![
            (
                "https://h/a.git".to_string(),
                PathBuf::from("/ws/.gaia/cache/git/a.git"),
            ),
            (
                "https://h/b".to_string(),
                PathBuf::from("/ws/.gaia/cache/git/b.git"),
            ),
        ];
        let fresh = insteadof_env(&rewrites, 0);
        let get = |env: &[(String, String)], name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            get(&fresh, "CARGO_NET_GIT_FETCH_WITH_CLI").as_deref(),
            Some("true")
        );
        assert_eq!(get(&fresh, "GIT_CONFIG_COUNT").as_deref(), Some("2"));
        assert_eq!(
            get(&fresh, "GIT_CONFIG_KEY_0").as_deref(),
            Some("url.file:///ws/.gaia/cache/git/a.git.insteadOf")
        );
        assert_eq!(
            get(&fresh, "GIT_CONFIG_VALUE_0").as_deref(),
            Some("https://h/a.git")
        );
        assert_eq!(
            get(&fresh, "GIT_CONFIG_KEY_1").as_deref(),
            Some("url.file:///ws/.gaia/cache/git/b.git.insteadOf")
        );
        assert_eq!(
            get(&fresh, "GIT_CONFIG_VALUE_1").as_deref(),
            Some("https://h/b")
        );

        let appended = insteadof_env(&rewrites, 3);
        assert_eq!(get(&appended, "GIT_CONFIG_COUNT").as_deref(), Some("5"));
        assert_eq!(
            get(&appended, "GIT_CONFIG_VALUE_3").as_deref(),
            Some("https://h/a.git")
        );
        assert!(get(&appended, "GIT_CONFIG_VALUE_0").is_none());
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = git_command()
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn each_mirror_has_its_own_lock() {
        let a = Path::new("/gaia-test/mirror-a.git");
        let b = Path::new("/gaia-test/mirror-b.git");
        assert!(Arc::ptr_eq(&mirror_lock(a), &mirror_lock(a)));
        assert!(!Arc::ptr_eq(&mirror_lock(a), &mirror_lock(b)));
        let held = mirror_lock(a);
        let _guard = held.lock().expect("mirror a is free");
        // A build preparing mirror a does not hold up mirror b.
        assert!(mirror_lock(b).try_lock().is_ok());
        assert!(mirror_lock(a).try_lock().is_err());
    }

    #[test]
    fn pinning_a_locked_commit_is_repeatable_and_leaves_a_readable_branch() {
        let git_available = git_command()
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !git_available {
            eprintln!("skipped: git is not on PATH");
            return;
        }
        let scratch = gaia_testkit::TempDir::new("gaia-cargo-git-pin");
        let work = scratch.join("work");
        fs::create_dir_all(&work).expect("work dir");
        git_in(&work, &["init", "--quiet"]);
        git_in(
            &work,
            &[
                "-c",
                "user.name=gaia",
                "-c",
                "user.email=gaia@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "pinned",
            ],
        );
        let commit = git_in(&work, &["rev-parse", "HEAD"]);
        let mirror = scratch.join("mirror.git");
        let output = git_command()
            .arg("clone")
            .arg("--quiet")
            .arg("--mirror")
            .arg(&work)
            .arg(&mirror)
            .output()
            .expect("clone runs");
        assert!(output.status.success(), "clone failed");

        let mapping = CargoGitSourceSpec {
            url: "https://example.invalid/dep.git".into(),
            source: gaia_spec::SourceId::new("dep"),
            repo: "https://example.invalid/dep.git".into(),
        };
        let timeout = Duration::from_secs(30);
        assert_eq!(locked_ref_commit(&mirror, &commit), None);
        pin_commit(
            &mirror,
            &mapping,
            &mapping.url,
            &commit,
            timeout,
            None,
            false,
        )
        .expect("first pin");
        assert_eq!(locked_ref_commit(&mirror, &commit), Some(commit.clone()));
        // A second pin finds the branch in place and writes nothing new.
        pin_commit(
            &mirror,
            &mapping,
            &mapping.url,
            &commit,
            timeout,
            None,
            false,
        )
        .expect("second pin");
        assert_eq!(locked_ref_commit(&mirror, &commit), Some(commit));
    }

    #[test]
    fn commits_must_be_full_object_names() {
        assert!(is_full_commit(&"a".repeat(40)));
        assert!(is_full_commit(&"0".repeat(64)));
        assert!(!is_full_commit("--upload-pack=x"));
        assert!(!is_full_commit(&"a".repeat(12)));
    }
}
