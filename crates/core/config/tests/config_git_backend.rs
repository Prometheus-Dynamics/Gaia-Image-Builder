//! `git_backend` on a git source: where its git commands run. Defaults to the
//! host; `execution` keeps the execution backend.

use gaia_config::try_resolve_config;
use gaia_spec::{GitBackendSpec, SourceDefinition};
use gaia_testkit::TempDir;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A build file in its own scratch directory, removed when dropped.
struct Build {
    path: PathBuf,
    _scratch: TempDir,
}

impl Deref for Build {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.path
    }
}

fn write_build(name: &str, source_extra: &str) -> Build {
    let scratch = TempDir::new(&format!("gaia-git-backend-{name}"));
    let build = scratch.join("build.toml");
    fs::write(
        &build,
        format!(
            "build_name = \"recipe\"\n\n[workspace]\nroot_dir = \".\"\n\n[[sources]]\nid = \"upstream\"\nkind = \"git\"\nrepo = \"https://example.invalid/org/repo.git\"\nbranch = \"main\"\n{source_extra}\n"
        ),
    )
    .expect("build file");
    Build {
        path: build,
        _scratch: scratch,
    }
}

fn git_backend_of(build: &Path) -> GitBackendSpec {
    let spec = try_resolve_config(build.to_str().expect("utf-8")).expect("resolve");
    let source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "upstream")
        .expect("upstream source");
    match &source.definition {
        SourceDefinition::Git(git) => git.git_backend,
        other => panic!("expected a git source, got {other:?}"),
    }
}

#[test]
fn git_sources_run_git_on_the_host_by_default() {
    let build = write_build("default", "");
    assert_eq!(git_backend_of(&build), GitBackendSpec::Host);
}

#[test]
fn git_backend_execution_is_accepted() {
    let build = write_build("execution", "git_backend = \"execution\"\n");
    assert_eq!(git_backend_of(&build), GitBackendSpec::Execution);
}

#[test]
fn git_backend_host_is_accepted() {
    let build = write_build("host", "git_backend = \"host\"\n");
    assert_eq!(git_backend_of(&build), GitBackendSpec::Host);
}

#[test]
fn unknown_git_backend_is_rejected() {
    let build = write_build("unknown", "git_backend = \"docker\"\n");
    assert!(try_resolve_config(build.to_str().expect("utf-8")).is_err());
}
