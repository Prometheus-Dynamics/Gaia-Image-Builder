//! The OCI source against a fake registry served from this test: pull,
//! digest checks, layer order, whiteouts, an index, and credentials.

use gaia_source_oci::source;
use gaia_step::{Ctx, StepError};
use gaia_store::Store;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CURL: &str = "/usr/bin/curl";
const CREDS_VAR: &str = "GAIA_OCI_TEST_CREDENTIALS";

/// What the fake registry serves, and how.
#[derive(Default)]
struct Content {
    /// By digest: media type (empty for a blob) and bytes.
    objects: BTreeMap<String, (String, Vec<u8>)>,
    /// Bytes served instead, by digest.
    tampered: BTreeMap<String, Vec<u8>>,
    /// Whether a bearer token is needed, and the basic credentials the
    /// token service wants (`None`: it gives tokens to anyone).
    bearer: bool,
    token_credentials: Option<String>,
}

/// (path, Authorization header) per request.
type Requests = Arc<Mutex<Vec<(String, Option<String>)>>>;

struct Fake {
    host: String,
    content: Arc<Mutex<Content>>,
    /// (path, Authorization header) per request.
    requests: Requests,
}

fn fake() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let host = format!("127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let content = Arc::new(Mutex::new(Content::default()));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (shared, seen, realm) = (
        Arc::clone(&content),
        Arc::clone(&requests),
        format!("http://{host}/token"),
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            let path = first
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let mut auth = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    auth = Some(value.trim().to_string());
                }
            }
            seen.lock()
                .expect("lock")
                .push((path.clone(), auth.clone()));
            let content = shared.lock().expect("lock");
            let (status, kind, body, extra) = respond(&content, &path, auth.as_deref(), &realm);
            let mut head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if !kind.is_empty() {
                head.push_str(&format!("Content-Type: {kind}\r\n"));
            }
            head.push_str(&extra);
            head.push_str("\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    Fake {
        host,
        content,
        requests,
    }
}

fn respond(
    content: &Content,
    path: &str,
    auth: Option<&str>,
    realm: &str,
) -> (u16, String, Vec<u8>, String) {
    if let Some(query) = path.strip_prefix("/token?") {
        let allowed = match &content.token_credentials {
            None => true,
            Some(credentials) => auth == Some(&format!("Basic {}", base64(credentials.as_bytes()))),
        };
        return if allowed {
            assert!(
                query.contains("scope=repository%3Atest%2Fimage%3Apull"),
                "{query}"
            );
            (
                200,
                "application/json".into(),
                br#"{"token":"tok"}"#.to_vec(),
                String::new(),
            )
        } else {
            (401, String::new(), Vec::new(), String::new())
        };
    }
    if content.bearer && auth != Some("Bearer tok") {
        let challenge = format!(
            "WWW-Authenticate: Bearer realm=\"{realm}\",service=\"fake\",scope=\"repository:test/image:pull\"\r\n"
        );
        return (401, String::new(), Vec::new(), challenge);
    }
    let digest = path.rsplit('/').next().unwrap_or_default();
    match content.objects.get(digest) {
        Some((kind, bytes)) => {
            let bytes = content
                .tampered
                .get(digest)
                .cloned()
                .unwrap_or_else(|| bytes.clone());
            (200, kind.clone(), bytes, String::new())
        }
        None => (404, String::new(), Vec::new(), String::new()),
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// A layer member.
enum Member<'a> {
    Dir(&'a str, u32),
    File(&'a str, &'a str, u32, u64),
    Link(&'a str, &'a str),
    Hard(&'a str, &'a str),
}

fn layer(members: &[Member<'_>], gzip: bool) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for member in members {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(1);
        header.set_uid(0);
        header.set_gid(0);
        match member {
            Member::Dir(path, mode) => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(*mode);
                header.set_size(0);
                builder
                    .append_data(&mut header, path, &[][..])
                    .expect("dir");
            }
            Member::File(path, data, mode, owner) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_uid(*owner);
                header.set_gid(*owner);
                header.set_size(data.len() as u64);
                builder
                    .append_data(&mut header, path, data.as_bytes())
                    .expect("file");
            }
            Member::Link(path, target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                builder
                    .append_link(&mut header, path, target)
                    .expect("link");
            }
            Member::Hard(path, target) => {
                header.set_entry_type(tar::EntryType::Link);
                header.set_mode(0o4755);
                header.set_size(0);
                builder
                    .append_link(&mut header, path, target)
                    .expect("hard");
            }
        }
    }
    let tar = builder.into_inner().expect("tar");
    if !gzip {
        return tar;
    }
    let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    out.write_all(&tar).expect("gz");
    out.finish().expect("gz")
}

fn lower() -> Vec<u8> {
    use Member::*;
    layer(
        &[
            Dir("etc/", 0o755),
            File("etc/a.txt", "one\n", 0o644, 0),
            File("etc/b.txt", "b\n", 0o644, 0),
            Dir("var/lib/", 0o755),
            File("var/lib/x", "x\n", 0o644, 0),
            Dir("usr/bin/", 0o755),
            File("usr/bin/tool", "#!/bin/sh\n", 0o4755, 1000),
            Dir("tmp/", 0o1777),
            Link("bin", "usr/bin"),
        ],
        true,
    )
}

fn upper() -> Vec<u8> {
    use Member::*;
    layer(
        &[
            File("etc/a.txt", "two\n", 0o600, 0),
            File("etc/.wh.b.txt", "", 0o644, 0),
            File("var/lib/.wh..wh..opq", "", 0o644, 0),
            File("var/lib/y", "y\n", 0o644, 0),
            Hard("usr/bin/tool2", "usr/bin/tool"),
            // Through the lower layer's `bin` symlink: replaces it with a
            // directory, never writes into usr/bin.
            File("bin/sh", "sh\n", 0o755, 0),
        ],
        false,
    )
}

/// Serves an image with `layers` (in order); returns its manifest digest.
fn publish(fake: &Fake, layers: &[Vec<u8>], arch: &str) -> String {
    let mut content = fake.content.lock().expect("lock");
    let config =
        format!(r#"{{"architecture":"{arch}","os":"linux","config":{{"Env":["PATH=/usr/bin"]}}}}"#)
            .into_bytes();
    let config_digest = sha(&config);
    content
        .objects
        .insert(config_digest.clone(), (String::new(), config.clone()));
    let mut descriptors = Vec::new();
    for layer in layers {
        let digest = sha(layer);
        content
            .objects
            .insert(digest.clone(), (String::new(), layer.clone()));
        descriptors.push(format!(
            r#"{{"mediaType":"application/vnd.oci.image.layer.v1.tar+gzip","digest":"{digest}","size":{}}}"#,
            layer.len()
        ));
    }
    let manifest = format!(
        r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"{config_digest}","size":{}}},"layers":[{}]}}"#,
        config.len(),
        descriptors.join(",")
    )
    .into_bytes();
    let digest = sha(&manifest);
    content.objects.insert(
        digest.clone(),
        (
            "application/vnd.oci.image.manifest.v1+json".into(),
            manifest,
        ),
    );
    digest
}

fn pull(
    root: &Path,
    fake: &Fake,
    digest: &str,
    platform: &str,
    credentials: &str,
) -> Result<(PathBuf, gaia_types::Tree, Vec<u8>), StepError> {
    let store = root.join("store");
    let reference = format!("{}/test/image:latest", fake.host);
    let (tree, config) = source(
        &store.display().to_string(),
        CURL,
        &reference,
        digest,
        platform,
        credentials,
        &Ctx::new(),
    )?;
    let store = Store::open(&store).expect("store");
    let rootfs = store.path(&tree.object.object_ref().expect("rootfs"));
    let config =
        std::fs::read(store.path(&config.object.object_ref().expect("config"))).expect("config");
    Ok((rootfs, tree, config))
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("stat")
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn an_image_is_pulled_with_its_layers_in_order_and_whiteouts_applied() {
    let fake = fake();
    let digest = publish(&fake, &[lower(), upper()], "arm64");
    let root = tempfile::tempdir().expect("tempdir");
    let (rootfs, tree, config) =
        pull(root.path(), &fake, &digest, "linux/arm64", "").expect("pulls");
    let read = |p: &str| std::fs::read_to_string(rootfs.join(p)).ok();
    println!("manifest {digest} -> rootfs {}", tree.object.digest);
    // Layer order: the upper layer's a.txt, with its mode.
    assert_eq!(read("etc/a.txt").as_deref(), Some("two\n"));
    assert_eq!(mode(&rootfs.join("etc/a.txt")), 0o600);
    // Whiteouts: a file, and an opaque directory.
    assert!(!rootfs.join("etc/b.txt").exists());
    assert!(!rootfs.join("etc/.wh.b.txt").exists());
    assert!(!rootfs.join("var/lib/x").exists());
    assert_eq!(read("var/lib/y").as_deref(), Some("y\n"));
    assert!(!rootfs.join("var/lib/.wh..wh..opq").exists());
    // Modes kept: setuid, sticky.
    assert_eq!(mode(&rootfs.join("usr/bin/tool")), 0o4755);
    assert_eq!(mode(&rootfs.join("tmp")), 0o1777);
    // The upper layer's bin/sh replaced the bin symlink with a directory.
    assert!(rootfs.join("bin").is_dir() && !rootfs.join("bin").is_symlink());
    assert!(!rootfs.join("usr/bin/sh").exists());
    assert_eq!(read("usr/bin/tool2").as_deref(), Some("#!/bin/sh\n"));
    // Owners other than root.
    let owners: Vec<_> = tree
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.uid, e.gid, e.mode))
        .collect();
    println!("owners: {owners:?}");
    assert_eq!(owners, [("usr/bin/tool", 1000, 1000, 0o4755)]);
    assert!(
        String::from_utf8(config)
            .expect("utf-8")
            .contains("\"architecture\":\"arm64\"")
    );

    // The same layers the other way round: the lower layer's a.txt wins.
    let first = layer(
        &[
            Member::File("etc/a.txt", "two\n", 0o600, 0),
            Member::File("etc/.wh.b.txt", "", 0o644, 0),
        ],
        false,
    );
    let swapped = publish(&fake, &[first, lower()], "arm64");
    let other = tempfile::tempdir().expect("tempdir");
    let (rootfs, _, _) = pull(other.path(), &fake, &swapped, "linux/arm64", "").expect("pulls");
    assert_eq!(
        std::fs::read_to_string(rootfs.join("etc/a.txt")).expect("a"),
        "one\n"
    );
    assert!(rootfs.join("etc/b.txt").exists());

    // Pulled again: found in the store, no request made.
    let before = fake.requests.lock().expect("lock").len();
    pull(root.path(), &fake, &digest, "linux/arm64", "").expect("again");
    assert_eq!(fake.requests.lock().expect("lock").len(), before);
}

#[test]
fn an_index_gives_the_platform_s_manifest() {
    let fake = fake();
    let arm = publish(&fake, &[lower()], "arm64");
    let amd = publish(&fake, &[upper()], "amd64");
    let index = format!(
        r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"{amd}","size":1,"platform":{{"os":"linux","architecture":"amd64"}}}},{{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"{arm}","size":1,"platform":{{"os":"linux","architecture":"arm64"}}}}]}}"#
    )
    .into_bytes();
    let digest = sha(&index);
    fake.content.lock().expect("lock").objects.insert(
        digest.clone(),
        ("application/vnd.oci.image.index.v1+json".into(), index),
    );
    let root = tempfile::tempdir().expect("tempdir");
    let (_, _, config) = pull(root.path(), &fake, &digest, "linux/arm64", "").expect("pulls");
    assert!(String::from_utf8(config).expect("utf-8").contains("arm64"));
    let error =
        pull(root.path(), &fake, &digest, "linux/riscv64", "").expect_err("no such platform");
    println!("{error}");
    assert!(
        error
            .to_string()
            .contains("no manifest for `linux/riscv64`")
    );
}

#[test]
fn every_digest_is_checked() {
    let fake = fake();
    let digest = publish(&fake, &[lower(), upper()], "arm64");
    let layer = sha(&upper());
    fake.content
        .lock()
        .expect("lock")
        .tampered
        .insert(layer.clone(), lower());
    let root = tempfile::tempdir().expect("tempdir");
    let error = pull(root.path(), &fake, &digest, "linux/arm64", "").expect_err("a layer is wrong");
    println!("tampered layer: {error}");
    let text = error.to_string();
    assert!(
        text.contains(&format!("wanted {layer}"))
            && text.contains(&format!("got {}", sha(&lower()))),
        "{text}"
    );

    // The lock's digest is not the manifest served for it.
    let manifest = fake.content.lock().expect("lock").objects[&digest].clone();
    let wrong = sha(b"another manifest");
    fake.content
        .lock()
        .expect("lock")
        .objects
        .insert(wrong.clone(), manifest);
    let error =
        pull(root.path(), &fake, &wrong, "linux/arm64", "").expect_err("the manifest is wrong");
    println!("wrong manifest: {error}");
    assert!(
        error.to_string().contains(&format!("wanted {wrong}")),
        "{error}"
    );
    let store = Store::open(root.path().join("store")).expect("store");
    assert!(gaia_step::step_records(&gaia_step::logs_dir(&store)).is_empty());
}

#[test]
fn anonymous_pulls_answer_a_bearer_challenge() {
    let fake = fake();
    let digest = publish(&fake, &[lower()], "arm64");
    fake.content.lock().expect("lock").bearer = true;
    let root = tempfile::tempdir().expect("tempdir");
    pull(root.path(), &fake, &digest, "linux/arm64", "").expect("pulls with an anonymous token");
    let requests = fake.requests.lock().expect("lock").clone();
    // The token request carried no credentials.
    assert!(
        requests
            .iter()
            .any(|(path, auth)| path.starts_with("/token") && auth.is_none())
    );
    assert!(
        requests
            .iter()
            .filter(|(path, _)| path.starts_with("/v2/"))
            .any(|(_, auth)| auth.as_deref() == Some("Bearer tok"))
    );

    // A token service that wants credentials, and none declared.
    fake.content.lock().expect("lock").token_credentials = Some("user:secret".into());
    let other = tempfile::tempdir().expect("tempdir");
    let error = pull(other.path(), &fake, &digest, "linux/arm64", "").expect_err("no credentials");
    println!("{error}");
    assert!(
        error.to_string().contains("no token anonymously"),
        "{error}"
    );
}

/// Runs as a child with the credentials variable set (this process cannot
/// set its own environment).
#[test]
fn credentials_child() {
    let (Ok(host), Ok(digest), Ok(root)) = (
        std::env::var("GAIA_OCI_TEST_HOST"),
        std::env::var("GAIA_OCI_TEST_DIGEST"),
        std::env::var("GAIA_OCI_TEST_ROOT"),
    ) else {
        return;
    };
    let reference = format!("{host}/test/image:latest");
    let store = Path::new(&root).join("store");
    source(
        &store.display().to_string(),
        CURL,
        &reference,
        &digest,
        "linux/arm64",
        CREDS_VAR,
        &Ctx::new(),
    )
    .expect("pulls with the credentials declared");
}

#[test]
fn declared_credentials_go_to_the_token_service_and_never_into_logs() {
    let fake = fake();
    let digest = publish(&fake, &[lower()], "arm64");
    {
        let mut content = fake.content.lock().expect("lock");
        content.bearer = true;
        content.token_credentials = Some("user:s3cret-value".into());
    }
    let root = tempfile::tempdir().expect("tempdir");
    let status = std::process::Command::new(std::env::current_exe().expect("exe"))
        .args([
            "--exact",
            "credentials_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CREDS_VAR, "user:s3cret-value")
        .env("GAIA_OCI_TEST_HOST", &fake.host)
        .env("GAIA_OCI_TEST_DIGEST", &digest)
        .env("GAIA_OCI_TEST_ROOT", root.path())
        .status()
        .expect("child");
    assert!(status.success());
    let requests = fake.requests.lock().expect("lock").clone();
    let token = requests
        .iter()
        .find(|(path, _)| path.starts_with("/token"))
        .expect("a token request");
    assert_eq!(
        token.1.as_deref(),
        Some(format!("Basic {}", base64(b"user:s3cret-value")).as_str())
    );
    // No run log holds the secret.
    let logs = root.path().join("logs");
    let mut seen = 0;
    for step in std::fs::read_dir(&logs).expect("logs").flatten() {
        if !step.path().is_dir() {
            continue;
        }
        for log in std::fs::read_dir(step.path()).expect("ls").flatten() {
            let text = std::fs::read_to_string(log.path()).expect("read");
            assert!(!text.contains("s3cret"), "{}", log.path().display());
            seen += 1;
        }
    }
    println!("{seen} run logs, none with the secret");
    assert!(seen > 0);
}

#[test]
fn a_reference_reads_like_docker_reads_it() {
    use gaia_source_oci::Reference;
    let r = Reference::parse("alpine:3.20").expect("parses");
    assert_eq!(
        (r.base.as_str(), r.repository.as_str()),
        ("https://registry-1.docker.io", "library/alpine")
    );
    let r = Reference::parse("ghcr.io/org/app:v1@sha256:abc").expect("parses");
    assert_eq!(
        (r.base.as_str(), r.repository.as_str(), r.digest.as_deref()),
        ("https://ghcr.io", "org/app", Some("sha256:abc"))
    );
    let r = Reference::parse("localhost:5000/a/b").expect("parses");
    assert_eq!(r.base, "http://localhost:5000");
    assert!(Reference::parse("ghcr.io/Org/../x").is_err());
}

#[test]
fn the_node_registers() {
    let mut registry = daedalus::prelude::Registry::new();
    registry
        .register_all(&gaia_source_oci::NODES)
        .expect("registers");
}
