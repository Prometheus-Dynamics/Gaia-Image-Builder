//! `kind = "archive"` with a `url`: the archive is downloaded into the shared
//! download cache, verified against its sha256 before it is extracted, and
//! extracted with the same tar call as a `path` archive.

use crate::ArchiveSourceProvider;
use crate::SourceProvider;
use crate::SourceProviderErrorKind;
use crate::digest::sha256_or_placeholder;
use crate::download::download_cache_path;
use crate::state::materialized_dir;
use gaia_spec::{
    ArchiveSourceSpec, CleanPolicy, ResolvedBuildSpec, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec, SourceSpec, WorkspaceSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A workspace under `root`, with its build dir inside.
fn spec_in(root: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("archive-url");
    spec.workspace = WorkspaceSpec {
        root_dir: root.display().to_string(),
        build_dir: root.join("build").display().to_string(),
        out_dir: root.join("out").display().to_string(),
        clean_policy: CleanPolicy::None,
        named_paths: Vec::new(),
    };
    spec
}

/// A `.tar.gz` of `payload/` (a `hello.txt` in a top directory `top/`), in
/// `dir`, and its sha256.
fn make_archive(dir: &Path) -> (PathBuf, String) {
    let payload = dir.join("payload/top");
    fs::create_dir_all(&payload).expect("payload");
    fs::write(payload.join("hello.txt"), "hello from the archive").expect("hello");
    let archive = dir.join("bundle.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.join("payload"))
        .arg("top")
        .status()
        .expect("tar runs");
    assert!(status.success(), "tar failed");
    let sha = sha256_or_placeholder(&archive);
    (archive, sha)
}

fn url_source(url: &Path, sha: &str, strip_components: u32) -> SourceSpec {
    SourceSpec::new(
        "bundle",
        SourceDefinition::Archive(ArchiveSourceSpec {
            path: String::new(),
            url: Some(format!("file://{}", url.display())),
            sha256: Some(sha.to_string()),
            strip_components,
            allow_absolute_symlinks: false,
            members: Vec::new(),
            extract_to: None,
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Locked,
        }),
    )
}

#[test]
fn a_url_archive_is_downloaded_verified_cached_and_extracted() {
    let scratch = TempDir::new("gaia-archive-url-ok");
    let workspace = scratch.join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    let (archive, sha) = make_archive(&scratch.join("served"));
    let spec = spec_in(&workspace);
    let source = url_source(&archive, &sha, 1);

    let messages = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("the archive extracts");
    assert!(messages[0].contains("extracted"), "{messages:?}");
    assert_eq!(
        fs::read_to_string(materialized_dir(&spec, &source).join("hello.txt")).unwrap(),
        "hello from the archive",
        "strip_components = 1 drops the top directory"
    );
    let cached =
        download_cache_path(&fs::canonicalize(&workspace).unwrap(), &sha).expect("a valid sha");
    assert!(cached.is_file(), "{}", cached.display());
    let state = fs::read_to_string(materialized_dir(&spec, &source).join(".gaia-source-state.txt"))
        .expect("source state");
    assert!(state.contains("archive_sha256="), "{state}");
    assert!(state.contains(&sha), "{state}");
}

#[test]
fn a_url_archive_with_the_wrong_sha256_is_refused_before_extraction() {
    let scratch = TempDir::new("gaia-archive-url-mismatch");
    let workspace = scratch.join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    let (archive, actual) = make_archive(&scratch.join("served"));
    let expected = "0".repeat(64);
    let spec = spec_in(&workspace);
    let source = url_source(&archive, &expected, 1);

    let error = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("a mismatched archive is refused");
    assert!(
        error.message.contains(&format!("expected {expected}")),
        "{}",
        error.message
    );
    assert!(
        error.message.contains(&format!("got {actual}")),
        "{}",
        error.message
    );
    assert!(
        !materialized_dir(&spec, &source).join("hello.txt").exists(),
        "nothing is extracted from an unverified archive"
    );
    let cached = download_cache_path(&fs::canonicalize(&workspace).unwrap(), &expected)
        .expect("a valid sha");
    assert!(!cached.exists(), "a mismatched download is not cached");
}

#[test]
fn offline_a_url_archive_comes_only_from_the_cache() {
    let scratch = TempDir::new("gaia-archive-url-offline");
    let workspace = scratch.join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    let served = scratch.join("served");
    let (archive, sha) = make_archive(&served);
    let mut spec = spec_in(&workspace);
    spec.policy.execution.offline = true;
    let source = url_source(&archive, &sha, 1);

    let error = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("offline with no cache entry fails");
    assert_eq!(error.kind, SourceProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("offline"), "{}", error.message);
    assert!(
        error.message.contains("not in the download cache"),
        "{}",
        error.message
    );

    // An online run fills the cache; the URL is then gone, and offline still
    // extracts from the cache alone.
    spec.policy.execution.offline = false;
    ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("online fill");
    fs::remove_file(&archive).expect("remove the served archive");
    spec.policy.execution.offline = true;
    ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("offline from the cache");
    assert_eq!(
        fs::read_to_string(materialized_dir(&spec, &source).join("hello.txt")).unwrap(),
        "hello from the archive"
    );
}

#[test]
fn members_extract_only_their_files_once_per_workspace() {
    let scratch = TempDir::new("gaia-archive-url-members");
    let workspace = scratch.join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    let served = scratch.join("served");
    fs::create_dir_all(served.join("payload/top/rtl_nic")).expect("payload");
    fs::write(served.join("payload/top/rtl_nic/rtl8153a-2.fw"), "fw a").expect("fw");
    fs::write(served.join("payload/top/rtl_nic/rtl8156b-2.fw"), "fw b").expect("fw");
    fs::write(served.join("payload/top/other.bin"), "other").expect("other");
    let (archive, sha) = make_archive(&served);
    let mut source = url_source(&archive, &sha, 1);
    if let SourceDefinition::Archive(spec) = &mut source.definition {
        spec.members = vec!["*/rtl_nic/rtl815*".into()];
    }

    // The first build dir extracts; the second copies the cached tree.
    let mut spec = spec_in(&workspace);
    let first = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("the members extract");
    assert!(first[0].contains("extracted into"), "{first:?}");
    // The second build dir puts the tree under extract_to, from the cache.
    spec.workspace.build_dir = workspace.join("build-clean").display().to_string();
    if let SourceDefinition::Archive(spec) = &mut source.definition {
        spec.extract_to = Some("rootfs/lib/firmware".into());
    }
    let second = ArchiveSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("the cached tree restores");
    assert!(
        second[0].contains("restored from the extract cache"),
        "{second:?}"
    );
    let dir = materialized_dir(&spec, &source).join("rootfs/lib/firmware");
    assert_eq!(
        fs::read_to_string(dir.join("rtl_nic/rtl8156b-2.fw")).unwrap(),
        "fw b"
    );
    assert!(dir.join("rtl_nic/rtl8153a-2.fw").is_file());
    assert!(!dir.join("other.bin").exists(), "only the members");
    assert!(!dir.join("hello.txt").exists(), "only the members");

    // A pattern that matches nothing fails the extract.
    if let SourceDefinition::Archive(spec) = &mut source.definition {
        spec.members = vec!["*/rtl_nic/rtl8159*".into()];
    }
    spec.workspace.build_dir = workspace.join("build-missing").display().to_string();
    assert!(
        ArchiveSourceProvider
            .execute_source(&spec, &source, None, None)
            .is_err()
    );
}
