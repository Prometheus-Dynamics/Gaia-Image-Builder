//! Push and pull against the fake registry, signed and verified by the real
//! minisign and cosign with throwaway keys. Each test skips, saying so, when
//! its tool is not on PATH. cosign signs offline (crate::cosign), so nothing
//! here reaches Sigstore or any registry.

use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gaia_testkit::TempDir;

use super::auth::Credentials;
use super::fake::FakeRegistry;
use super::*;

const KEY: &str = "0123abcd";
const IMAGE: &str = "owner/gaia-cache/package";

/// The first `program` on PATH.
fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn succeeds(command: &mut Command) -> bool {
    let output = command.stdin(Stdio::null()).output().expect("run tool");
    output.status.success()
}

/// Pushes an entry signed with `sign_key`, pulls it back verified with
/// `verify_key`, then changes the stored manifest and expects the pull to be
/// refused.
fn round_trip(
    work: &TempDir,
    kind: SignatureKind,
    tools: &Path,
    sign_key: &Path,
    verify_key: &Path,
) {
    let registry = FakeRegistry::start(None);
    let target = RegistryTarget {
        registry: registry.addr().to_string(),
        repository: "owner/gaia-cache".to_string(),
        kind,
        sign_key: Some(sign_key.display().to_string()),
        verify_key: Some(verify_key.display().to_string()),
        trust_unsigned: false,
        token: None,
        username: None,
        tool_dir: Some(tools.to_path_buf()),
    };
    let client = RemoteCache::with_credentials(target, Credentials::Anonymous);
    let member = work.join("pkg");
    fs::create_dir_all(&member).expect("entry dir");
    fs::write(member.join("hello.txt"), "content").expect("entry file");
    let blob = work.join("pkg.tar.zst");
    pack_entry(work.path(), &["pkg"], &blob).expect("pack entry");
    client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push");

    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    let pulled = client
        .pull("package", KEY, &dest)
        .expect("pull")
        .expect("entry exists");
    assert_eq!(
        fs::read(&pulled.blob).expect("pulled"),
        fs::read(&blob).expect("blob")
    );

    registry
        .state()
        .manifests
        .get_mut(IMAGE)
        .expect("image")
        .get_mut(KEY)
        .expect("tag")
        .push(b' ');
    let result = client.pull("package", KEY, &dest);
    assert!(
        matches!(result, Err(RegistryError::Signature(_))),
        "{result:?}"
    );
}

#[test]
fn minisign_signs_and_verifies_a_pushed_entry() {
    let Some(minisign) = on_path("minisign") else {
        eprintln!("skipped: no minisign on PATH");
        return;
    };
    let work = TempDir::new("registry-minisign");
    let tools = work.join("tools");
    fs::create_dir_all(&tools).expect("tools");
    std::os::unix::fs::symlink(&minisign, tools.join("minisign")).expect("link minisign");
    let (public, secret) = (work.join("test.pub"), work.join("test.key"));
    assert!(succeeds(
        Command::new(&minisign)
            .args(["-G", "-W", "-p"])
            .arg(&public)
            .arg("-s")
            .arg(&secret)
    ));
    round_trip(&work, SignatureKind::Minisign, &tools, &secret, &public);
}

#[test]
fn cosign_signs_offline_and_verifies_a_pushed_entry() {
    let Some(cosign) = on_path("cosign") else {
        eprintln!("skipped: no cosign on PATH");
        return;
    };
    let work = TempDir::new("registry-cosign");
    let tools = work.join("tools");
    fs::create_dir_all(&tools).expect("tools");
    // The throwaway key has an empty password, which cosign reads from
    // COSIGN_PASSWORD; the wrapper sets it for this tool only.
    let wrapper = tools.join("cosign");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nCOSIGN_PASSWORD= exec '{}' \"$@\"\n",
            cosign.display()
        ),
    )
    .expect("write wrapper");
    fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).expect("chmod wrapper");
    assert!(succeeds(
        Command::new(&wrapper)
            .arg("generate-key-pair")
            .current_dir(work.path())
    ));
    round_trip(
        &work,
        SignatureKind::CosignBlob,
        &tools,
        &work.join("cosign.key"),
        &work.join("cosign.pub"),
    );
}
