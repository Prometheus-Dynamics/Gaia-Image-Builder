//! `git_backend` on a git source: where its git commands run. Defaults to the
//! host; `execution` keeps the execution backend.

use gaia_config::try_resolve_config;
use gaia_spec::{GitBackendSpec, SourceDefinition};
use std::fs;
use std::path::{Path, PathBuf};

fn write_build(name: &str, source_extra: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gaia-git-backend-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    let build = dir.join("build.toml");
    fs::write(
        &build,
        format!(
            "build_name = \"recipe\"\n\n[workspace]\nroot_dir = \".\"\n\n[[sources]]\nid = \"upstream\"\nkind = \"git\"\nrepo = \"https://example.invalid/org/repo.git\"\nbranch = \"main\"\n{source_extra}\n"
        ),
    )
    .expect("build file");
    build
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
