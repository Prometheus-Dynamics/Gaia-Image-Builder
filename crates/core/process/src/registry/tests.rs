//! Registry client tests against the fake registry, with a stub signing tool
//! that writes the SHA-256 of the signed bytes as its signature. Nothing here
//! contacts a real registry.

use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use gaia_testkit::TempDir;

use super::auth::{Challenge, Credentials, decode_basic, parse_challenge};
use super::fake::{BEARER, FakeRegistry, base64};
use super::http::{Auth, Request, curl_args, curl_config};
use super::*;
use crate::sha256_hex_bytes;

const IMAGE: &str = "owner/gaia-cache/package";
const KEY: &str = "0123abcd";
const REPOSITORY: &str = "owner/gaia-cache";

const STUB_SIGNER: &str = r#"#!/bin/sh
mode="$1"; file=""; sig=""
while [ $# -gt 0 ]; do
  case "$1" in
    -m) file="$2"; shift ;;
    -x) sig="$2"; shift ;;
  esac
  shift
done
digest=$(sha256sum "$file" | cut -d' ' -f1)
if [ "$mode" = "-S" ]; then printf '%s' "$digest" > "$sig"; exit 0; fi
if [ "$mode" = "-V" ]; then [ "$digest" = "$(cat "$sig")" ]; exit $?; fi
exit 2
"#;

/// A directory holding the stub `minisign`, used as the tool directory.
fn stub_tools(dir: &TempDir) -> PathBuf {
    let tool = dir.path().join("minisign");
    fs::write(&tool, STUB_SIGNER).expect("write stub signer");
    fs::set_permissions(&tool, Permissions::from_mode(0o755)).expect("chmod stub signer");
    dir.path().to_path_buf()
}

fn target(addr: &str, tools: &Path, sign: bool, verify: bool) -> RegistryTarget {
    RegistryTarget {
        registry: addr.to_string(),
        repository: REPOSITORY.to_string(),
        kind: SignatureKind::Minisign,
        sign_key: sign.then(|| "/unused/secret.key".to_string()),
        verify_key: verify.then(|| "/unused/public.key".to_string()),
        trust_unsigned: false,
        token: None,
        username: None,
        tool_dir: Some(tools.to_path_buf()),
    }
}

fn cache(addr: &str, tools: &Path, sign: bool, verify: bool) -> RemoteCache {
    RemoteCache::with_credentials(target(addr, tools, sign, verify), Credentials::Anonymous)
}

/// Writes a small entry tree under `work` and packs it to `work/<name>.tar.zst`.
fn make_entry(work: &Path, name: &str) -> PathBuf {
    let member = work.join(name);
    fs::create_dir_all(&member).expect("entry dir");
    fs::write(member.join("hello.txt"), format!("content of {name}")).expect("entry file");
    let out = work.join(format!("{name}.tar.zst"));
    pack_entry(work, &[name], &out).expect("pack entry");
    out
}

fn extra_annotations() -> BTreeMap<String, String> {
    BTreeMap::from([("dev.gaia.relocatable".to_string(), "true".to_string())])
}

#[test]
fn push_then_pull_round_trips_bytes_annotations_and_manifest() {
    let work = TempDir::new("registry-roundtrip");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");

    let PushOutcome::Pushed {
        digest,
        replaced: false,
    } = client
        .push("package", KEY, &blob, &extra_annotations())
        .expect("push")
    else {
        panic!("expected a push");
    };

    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    let pulled = client
        .pull("package", KEY, &dest)
        .expect("pull")
        .expect("entry exists");
    assert_eq!(
        fs::read(&pulled.blob).expect("pulled blob"),
        fs::read(&blob).expect("blob")
    );
    let blob_hex = crate::sha256_file(&blob).expect("blob digest");
    assert_eq!(pulled.blob, dest.join(format!("{blob_hex}.tar.zst")));
    assert_eq!(pulled.annotations["dev.gaia.kind"], "package");
    assert_eq!(pulled.annotations["dev.gaia.key"], KEY);
    assert_eq!(pulled.annotations["dev.gaia.relocatable"], "true");
    assert!(
        pulled
            .annotations
            .contains_key("org.opencontainers.image.created")
    );
    assert_eq!(
        pulled.annotations["dev.gaia.version"],
        env!("CARGO_PKG_VERSION")
    );
    let stored = registry.state().manifests[IMAGE][KEY].clone();
    assert_eq!(pulled.manifest, stored);
    assert_eq!(digest, format!("sha256:{}", sha256_hex_bytes(&stored)));
}

#[test]
fn tampered_manifest_fails_the_signature_and_returns_no_entry() {
    let work = TempDir::new("registry-tamper-manifest");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push");
    registry
        .state()
        .manifests
        .get_mut(IMAGE)
        .expect("image")
        .get_mut(KEY)
        .expect("tag")
        .push(b' ');

    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    let result = client.pull("package", KEY, &dest);
    assert!(
        matches!(result, Err(RegistryError::Signature(_))),
        "{result:?}"
    );
}

#[test]
fn a_push_replaces_a_tag_whose_signature_does_not_verify() {
    let work = TempDir::new("registry-replace-poisoned");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push");
    // A verified entry is left alone.
    assert_eq!(
        client
            .push("package", KEY, &blob, &BTreeMap::new())
            .expect("second push"),
        PushOutcome::AlreadyPresent
    );
    // A poisoned one is replaced, and pulls verify again afterwards.
    registry
        .state()
        .manifests
        .get_mut(IMAGE)
        .expect("image")
        .get_mut(KEY)
        .expect("tag")
        .push(b' ');
    let outcome = client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("replacing push");
    assert!(
        matches!(outcome, PushOutcome::Pushed { replaced: true, .. }),
        "{outcome:?}"
    );
    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    assert!(
        client
            .pull("package", KEY, &dest)
            .expect("pull verifies")
            .is_some()
    );
}

#[test]
fn corrupted_blob_fails_its_sha256_and_leaves_no_file() {
    let work = TempDir::new("registry-tamper-blob");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    let PushOutcome::Pushed { .. } = client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push")
    else {
        panic!("expected a push");
    };
    registry
        .state()
        .blobs
        .get_mut(&format!(
            "sha256:{}",
            crate::sha256_file(&blob).expect("blob digest")
        ))
        .expect("blob")
        .push(0);

    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    let result = client.pull("package", KEY, &dest);
    assert!(
        matches!(result, Err(RegistryError::Protocol(_))),
        "{result:?}"
    );
    let hex = crate::sha256_file(&blob).expect("blob digest");
    assert!(!dest.join(format!("{hex}.tar.zst")).exists());
}

#[test]
fn pull_without_verify_key_is_refused_before_any_request() {
    let work = TempDir::new("registry-unsigned");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, false);
    let result = client.pull("package", KEY, work.path());
    assert!(
        matches!(result, Err(RegistryError::Unsigned(_))),
        "{result:?}"
    );
    assert!(registry.state().requests.is_empty());
}

#[test]
fn trust_unsigned_pull_skips_the_signature_check() {
    let work = TempDir::new("registry-trust");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let signing = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    signing
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push");
    registry
        .state()
        .manifests
        .get_mut(IMAGE)
        .expect("image")
        .remove(&format!("{KEY}.sig"));

    let mut trusting = target(registry.addr(), &tools, false, false);
    trusting.trust_unsigned = true;
    let client = RemoteCache::with_credentials(trusting, Credentials::Anonymous);
    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    let pulled = client.pull("package", KEY, &dest).expect("pull");
    assert!(pulled.is_some());
}

#[test]
fn push_without_sign_key_is_disabled_and_sends_nothing() {
    let work = TempDir::new("registry-disabled");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, false, true);
    let blob = make_entry(work.path(), "pkg");
    let outcome = client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push");
    assert_eq!(outcome, PushOutcome::Disabled);
    assert!(registry.state().requests.is_empty());
}

#[test]
fn existing_key_is_already_present_without_uploads() {
    let work = TempDir::new("registry-present");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("first push");
    assert!(client.has("package", KEY).expect("has"));
    assert!(!client.has("package", "ffff").expect("has other"));

    let before = registry.state().requests.len();
    let outcome = client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("second push");
    assert_eq!(outcome, PushOutcome::AlreadyPresent);
    // The existing entry is read and verified, and nothing is uploaded.
    let state = registry.state();
    let sent = &state.requests[before..];
    assert!(
        sent.iter()
            .all(|request| request.starts_with("HEAD ") || request.starts_with("GET ")),
        "{sent:?}"
    );
}

#[test]
fn bearer_challenge_is_answered_with_basic_credentials_and_cached() {
    let work = TempDir::new("registry-auth");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(Some(("gaia", "s3cret")));
    let credentials = Credentials::Basic {
        username: "gaia".to_string(),
        secret: "s3cret".to_string(),
    };
    let client =
        RemoteCache::with_credentials(target(registry.addr(), &tools, true, true), credentials);
    let blob = make_entry(work.path(), "pkg");
    client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push with auth");
    let dest = work.join("pulled");
    fs::create_dir_all(&dest).expect("dest");
    assert!(
        client
            .pull("package", KEY, &dest)
            .expect("pull with auth")
            .is_some()
    );

    let state = registry.state();
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|request| *request == "GET /token")
            .count(),
        1,
        "the token is fetched once and cached: {:?}",
        state.requests
    );

    let bearer = format!("Bearer {BEARER}");
    assert!(state.api_authorization.contains(&bearer));
    drop(state);

    let wrong = RemoteCache::with_credentials(
        target(registry.addr(), &tools, true, true),
        Credentials::Basic {
            username: "gaia".to_string(),
            secret: "wrong".to_string(),
        },
    );
    let refused = wrong.pull("package", KEY, &dest);
    assert!(
        matches!(refused, Err(RegistryError::Transport(_))),
        "{refused:?}"
    );
}

#[test]
fn curl_arguments_and_config_never_carry_the_secret_on_the_command_line() {
    let mut request = Request::new("PUT", "http://127.0.0.1:1/v2/x/manifests/y");
    request.auth = Auth::Basic {
        user: "gaia".to_string(),
        secret: "s3cret-value".to_string(),
    };
    let config = curl_config(&request).expect("config");
    assert!(
        config.contains("s3cret-value"),
        "the secret goes in the config file"
    );
    let args = curl_args(&request, Path::new("cfg-file"), Path::new("hdr-file"));
    assert!(
        args.iter()
            .all(|arg| !arg.to_string_lossy().contains("s3cret-value")),
        "{args:?}"
    );
    assert!(
        !format!("{request:?}").contains("s3cret-value"),
        "Debug hides secrets"
    );
}

#[test]
fn list_tags_manifest_digest_get_and_delete_manifest() {
    let work = TempDir::new("registry-helpers");
    let tools = stub_tools(&work);
    let registry = FakeRegistry::start(None);
    let client = cache(registry.addr(), &tools, true, true);
    let blob = make_entry(work.path(), "pkg");
    let PushOutcome::Pushed { digest, .. } = client
        .push("package", KEY, &blob, &BTreeMap::new())
        .expect("push")
    else {
        panic!("expected a push");
    };

    let tags = client.list_tags(IMAGE).expect("tags");
    assert!(
        tags.contains(&KEY.to_string()) && tags.contains(&format!("{KEY}.sig")),
        "{tags:?}"
    );
    assert_eq!(
        client.list_tags("owner/gaia-cache/unknown").expect("none"),
        Vec::<String>::new()
    );
    assert_eq!(
        client.manifest_digest(IMAGE, KEY).expect("digest"),
        Some(digest.clone())
    );
    assert_eq!(
        client.manifest_digest(IMAGE, "absent").expect("absent"),
        None
    );
    let (bytes, got) = client
        .get_manifest(IMAGE, KEY)
        .expect("get")
        .expect("present");
    assert_eq!(got, digest);
    assert_eq!(bytes, registry.state().manifests[IMAGE][KEY]);

    client.delete_manifest(IMAGE, &digest).expect("delete");
    assert_eq!(client.manifest_digest(IMAGE, KEY).expect("gone"), None);
    assert!(client.get_manifest(IMAGE, KEY).expect("get gone").is_none());
    client
        .delete_manifest(IMAGE, &digest)
        .expect("delete twice is fine");
}

#[test]
fn pack_then_unpack_round_trips_members() {
    let work = TempDir::new("registry-archive");
    let blob = make_entry(work.path(), "pkg");
    fs::write(work.join("pkg.json"), "{}").expect("json member");
    let out = work.join("both.tar.zst");
    pack_entry(work.path(), &["pkg", "pkg.json"], &out).expect("pack");

    let dest = work.join("unpacked");
    let members = unpack_entry(&out, &dest, "pkg").expect("unpack");
    assert!(
        members.contains(&"pkg/hello.txt".to_string()),
        "{members:?}"
    );
    assert!(members.contains(&"pkg.json".to_string()), "{members:?}");
    assert_eq!(
        fs::read_to_string(dest.join("pkg/hello.txt")).expect("file"),
        "content of pkg"
    );
    assert!(blob.is_file());
}

/// A tar.zst whose member names are `member`, made by renaming a real file
/// with tar's own transform.
fn archive_with_member(work: &TempDir, member: &str) -> PathBuf {
    let source = work.join("source");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("file"), "x").expect("source file");
    let tar_path = work.join("crafted.tar");
    let status = Command::new("tar")
        .arg("-C")
        .arg(&source)
        .arg(format!("--transform=s,^file$,{member},"))
        .arg("-cf")
        .arg(&tar_path)
        .arg("file")
        .status()
        .expect("run tar");
    assert!(status.success());
    let out = work.join("crafted.tar.zst");
    let status = Command::new("zstd")
        .args(["-q", "-f", "-o"])
        .arg(&out)
        .arg(&tar_path)
        .status()
        .expect("run zstd");
    assert!(status.success());
    out
}

#[test]
fn unpack_refuses_members_outside_the_entry() {
    let work = TempDir::new("registry-evil");
    let archive = archive_with_member(&work, "../evil");
    let result = unpack_entry(&archive, &work.join("dest"), "pkg");
    assert!(
        matches!(&result, Err(RegistryError::Protocol(message)) if message.contains("evil")),
        "{result:?}"
    );
    assert!(!work.join("evil").exists());
}

#[test]
fn unpack_refuses_symlinks() {
    let work = TempDir::new("registry-symlink");
    let entry = work.join("pkg");
    fs::create_dir_all(&entry).expect("entry dir");
    std::os::unix::fs::symlink("/etc", entry.join("link")).expect("symlink");
    let out = work.join("linked.tar.zst");
    pack_entry(work.path(), &["pkg"], &out).expect("pack symlink");
    let result = unpack_entry(&out, &work.join("dest"), "pkg");
    assert!(
        matches!(result, Err(RegistryError::Protocol(_))),
        "{result:?}"
    );
}

#[test]
fn base64_and_basic_decoding_round_trip() {
    assert_eq!(base64(b"gaia:s3cret"), "Z2FpYTpzM2NyZXQ=");
    let decoded = decode_basic("Z2FpYTpzM2NyZXQ=").expect("decode");
    assert_eq!(
        decoded,
        Credentials::Basic {
            username: "gaia".to_string(),
            secret: "s3cret".to_string(),
        }
    );
    assert!(decode_basic("!!!").is_none());
}

#[test]
fn bearer_challenge_keeps_commas_inside_quoted_scopes() {
    let parsed = parse_challenge(
        r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:owner/gaia-cache/package:pull,push""#,
    );
    assert_eq!(
        parsed,
        Challenge::Bearer {
            realm: "https://ghcr.io/token".to_string(),
            service: "ghcr.io".to_string(),
            scope: "repository:owner/gaia-cache/package:pull,push".to_string(),
        }
    );
    assert_eq!(parse_challenge("Basic realm=\"x\""), Challenge::Basic);
}

#[test]
fn signature_kind_parses_the_config_words() {
    assert_eq!(
        SignatureKind::parse("minisign"),
        Some(SignatureKind::Minisign)
    );
    assert_eq!(SignatureKind::parse("gpg"), Some(SignatureKind::Gpg));
    assert_eq!(
        SignatureKind::parse("cosign-blob"),
        Some(SignatureKind::CosignBlob)
    );
    assert_eq!(SignatureKind::parse("pgp"), None);
}

#[test]
fn registry_target_debug_hides_key_values() {
    let mut text = target("127.0.0.1:1", Path::new("/unused"), true, true);
    text.sign_key = Some("super-secret-key-path".to_string());
    assert!(!format!("{text:?}").contains("super-secret-key-path"));
}
