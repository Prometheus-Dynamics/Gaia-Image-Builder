//! The file sources: a local path, archives, downloads from a local server.

use gaia_source_files::{partial_dir, source_archive, source_download, source_path};
use gaia_step::{Ctx, StepError, logs_dir, step_records};
use gaia_store::{Digest, Store, tree_digest};
use gaia_types::{Artifact, ArtifactKind};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn store_str(root: &Path) -> String {
    root.join("store").display().to_string()
}

fn digest_of(object: &gaia_types::StoreRef) -> Digest {
    object.object_ref().expect("object").digest
}

/// A directory with a file, an executable, a subdirectory and a symlink.
fn sample(dir: &Path) {
    std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
    std::fs::write(dir.join("a.txt"), "a\n").expect("write");
    std::fs::write(dir.join("sub/run"), "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(dir.join("sub/run"), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    std::os::unix::fs::symlink("a.txt", dir.join("link")).expect("symlink");
}

#[test]
fn a_path_is_stored_as_its_tree() {
    let root = tempfile::tempdir().expect("tempdir");
    let src = root.path().join("src");
    sample(&src);
    let want = tree_digest(&src).expect("digest");
    let tree = source_path(
        &store_str(root.path()),
        &src.display().to_string(),
        &Ctx::new(),
    )
    .expect("stores");
    println!("path {} -> {}", src.display(), digest_of(&tree.object));
    assert_eq!(digest_of(&tree.object), want);
    // Again: found by its tree digest, not copied.
    source_path(
        &store_str(root.path()),
        &src.display().to_string(),
        &Ctx::new(),
    )
    .expect("again");
    let store = Store::open(root.path().join("store")).expect("store");
    let made: Vec<bool> = step_records(&logs_dir(&store))
        .iter()
        .map(|r| r.made)
        .collect();
    assert_eq!(made, [true, false]);
    // One file edited: another key, another tree.
    std::fs::write(src.join("a.txt"), "b\n").expect("write");
    let edited = source_path(
        &store_str(root.path()),
        &src.display().to_string(),
        &Ctx::new(),
    )
    .expect("edited");
    assert_ne!(digest_of(&edited.object), want);
}

/// `entries` (path, data or symlink target, mode) as a tar, under `top/`.
fn tar_bytes() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut add = |path: &str, data: &[u8], mode: u32, kind: tar::EntryType| {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_mtime(1_000_000);
        header.set_size(if kind == tar::EntryType::Regular {
            data.len() as u64
        } else {
            0
        });
        if kind == tar::EntryType::Symlink {
            header
                .set_link_name(std::str::from_utf8(data).expect("utf-8"))
                .expect("link");
            builder
                .append_data(&mut header, path, &[][..])
                .expect("append");
        } else {
            builder
                .append_data(&mut header, path, data)
                .expect("append");
        }
    };
    add("pkg-1.0/", b"", 0o775, tar::EntryType::Directory);
    add("pkg-1.0/a.txt", b"a\n", 0o664, tar::EntryType::Regular);
    add("pkg-1.0/sub/", b"", 0o700, tar::EntryType::Directory);
    add(
        "pkg-1.0/sub/run",
        b"#!/bin/sh\n",
        0o700,
        tar::EntryType::Regular,
    );
    add("pkg-1.0/link", b"a.txt", 0o777, tar::EntryType::Symlink);
    builder.into_inner().expect("tar")
}

fn compressed(kind: &str, tar: &[u8]) -> Vec<u8> {
    match kind {
        "tar" => tar.to_vec(),
        "gz" => {
            let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            out.write_all(tar).expect("gz");
            out.finish().expect("gz")
        }
        "xz" => {
            let mut out = xz2::write::XzEncoder::new(Vec::new(), 6);
            out.write_all(tar).expect("xz");
            out.finish().expect("xz")
        }
        "zst" => zstd::encode_all(tar, 3).expect("zstd"),
        other => panic!("{other}"),
    }
}

/// Puts `bytes` in the store as an archive artifact.
fn stored_archive(root: &Path, bytes: &[u8]) -> Artifact {
    let file = root.join("upload");
    std::fs::write(&file, bytes).expect("write");
    let store = Store::open(root.join("store")).expect("store");
    let object = store.put_file(&file).expect("put");
    Artifact {
        kind: ArtifactKind::Archive,
        object: object.into(),
    }
}

fn unpack(root: &Path, bytes: &[u8], strip: i64) -> Result<(Digest, PathBuf), StepError> {
    let archive = stored_archive(root, bytes);
    let tree = source_archive(&store_str(root), &archive, strip, &Ctx::new())?;
    let store = Store::open(root.join("store")).expect("store");
    let object = tree.object.object_ref().expect("object");
    Ok((object.digest, store.path(&object)))
}

#[test]
fn an_archive_unpacks_to_the_same_tree_whatever_its_compression() {
    let root = tempfile::tempdir().expect("tempdir");
    let tar = tar_bytes();
    let mut digests = Vec::new();
    for kind in ["tar", "gz", "xz", "zst"] {
        let (digest, tree) = unpack(root.path(), &compressed(kind, &tar), 1).expect(kind);
        println!("{kind}: strip 1 -> {digest}");
        assert_eq!(
            std::fs::read_to_string(tree.join("a.txt")).expect("a"),
            "a\n"
        );
        assert_eq!(
            std::fs::read_link(tree.join("link")).expect("link"),
            Path::new("a.txt")
        );
        let mode = |p: &str| {
            std::fs::symlink_metadata(tree.join(p))
                .expect("stat")
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(
            (mode("a.txt"), mode("sub/run"), mode("sub")),
            (0o644, 0o755, 0o755)
        );
        digests.push(digest);
    }
    assert!(
        digests.windows(2).all(|pair| pair[0] == pair[1]),
        "{digests:?}"
    );
    let (whole, tree) = unpack(root.path(), &tar, 0).expect("strip 0");
    println!("tar: strip 0 -> {whole}");
    assert!(tree.join("pkg-1.0/a.txt").is_file());
    assert_ne!(whole, digests[0]);
}

/// A tar with one member named `name` (written into the header as is) and,
/// when given, a symlink before it.
fn raw_tar(link: Option<(&str, &str)>, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut member = |name: &str, kind: tar::EntryType, data: &[u8], link: Option<&str>| {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        if let Some(link) = link {
            header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        header.set_cksum();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    };
    if let Some((name, target)) = link {
        member(name, tar::EntryType::Symlink, b"", Some(target));
    }
    member(name, tar::EntryType::Regular, b"evil\n", None);
    out.resize(out.len() + 1024, 0);
    out
}

#[test]
fn an_archive_never_writes_outside_its_tree() {
    let root = tempfile::tempdir().expect("tempdir");
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).expect("mkdir");
    let cases = [
        ("a `..` member", raw_tar(None, "../evil")),
        ("an absolute member", raw_tar(None, "/tmp/gaia-evil")),
        (
            "a member through a symlink",
            raw_tar(Some(("l", &outside.display().to_string())), "l/evil"),
        ),
    ];
    for (what, bytes) in cases {
        let error = unpack(root.path(), &bytes, 0).expect_err(what);
        println!("{what}: {error}");
        assert!(error.to_string().contains("not unpacked"), "{error}");
    }
    assert_eq!(std::fs::read_dir(&outside).expect("ls").count(), 0);
    assert!(!root.path().join("evil").exists());
}

/// A local HTTP server. `respond` gets the request number and its Range
/// start, and returns (status, bytes to send, Content-Length to claim).
struct Server {
    url: String,
    ranges: Arc<Mutex<Vec<Option<u64>>>>,
}

fn server(
    respond: impl Fn(usize, Option<u64>) -> (u16, Vec<u8>, usize) + Send + 'static,
) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/file.tar", listener.local_addr().expect("addr"));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&ranges);
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut range = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    range = value.trim().trim_end_matches('-').parse().ok();
                }
            }
            seen.lock().expect("lock").push(range);
            let (status, body, length) = respond(n, range);
            let reason = if status == 206 {
                "Partial Content"
            } else {
                "OK"
            };
            let mut head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {length}\r\nConnection: close\r\n"
            );
            if status == 206 {
                let start = range.unwrap_or(0);
                head.push_str(&format!(
                    "Content-Range: bytes {start}-{}/{}\r\n",
                    start as usize + length - 1,
                    start as usize + length
                ));
            }
            head.push_str("\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    Server { url, ranges }
}

fn fetch(root: &Path, url: &str, digest: &Digest) -> Result<Digest, StepError> {
    let file = source_download(
        &store_str(root),
        "/usr/bin/curl",
        url,
        &digest.to_string(),
        "archive",
        &Ctx::new(),
    )?;
    assert_eq!(file.kind, ArtifactKind::Archive);
    Ok(digest_of(&file.object))
}

fn payload() -> Vec<u8> {
    (0..200_000_u32).flat_map(|n| n.to_le_bytes()).collect()
}

#[test]
fn a_download_that_stops_resumes_on_the_next_run() {
    let body = payload();
    let want = Digest::of_bytes(&body);
    let half = body.len() / 2;
    let served = body.clone();
    let server = server(move |n, range| match (n, range) {
        // Claims the whole length, sends half, closes.
        (0, _) => (200, served[..half].to_vec(), served.len()),
        (_, Some(start)) => (
            206,
            served[start as usize..].to_vec(),
            served.len() - start as usize,
        ),
        (_, None) => (200, served.clone(), served.len()),
    });
    let root = tempfile::tempdir().expect("tempdir");
    let error = fetch(root.path(), &server.url, &want).expect_err("the first run stops");
    println!("first run: {error}");
    let store = Store::open(root.path().join("store")).expect("store");
    let partial = partial_dir(&store).join(format!("{}.part", want.hex()));
    let kept = std::fs::metadata(&partial).expect("partial kept").len();
    println!("kept {kept} of {} bytes", body.len());
    assert_eq!(kept as usize, half);
    let got = fetch(root.path(), &server.url, &want).expect("the second run resumes");
    let ranges = server.ranges.lock().expect("lock").clone();
    println!("second run: {got}; Range starts asked: {ranges:?}");
    assert_eq!(got, want);
    assert_eq!(ranges, [None, Some(half as u64)]);
    assert!(!partial.exists());
}

#[test]
fn a_server_that_will_not_resume_is_fetched_again_whole() {
    let body = payload();
    let want = Digest::of_bytes(&body);
    let served = body.clone();
    let server = server(move |_, _| (200, served.clone(), served.len()));
    let root = tempfile::tempdir().expect("tempdir");
    let store = Store::open(root.path().join("store")).expect("store");
    let partial = partial_dir(&store).join(format!("{}.part", want.hex()));
    std::fs::create_dir_all(partial.parent().expect("dir")).expect("mkdir");
    std::fs::write(&partial, &body[..1000]).expect("write");
    let got = fetch(root.path(), &server.url, &want).expect("fetched whole");
    println!("ranges asked: {:?}", server.ranges.lock().expect("lock"));
    assert_eq!(got, want);
}

#[test]
fn a_wrong_digest_fails_with_both_digests() {
    let body = payload();
    let got = Digest::of_bytes(&body);
    let want = Digest::of_bytes(b"something else");
    let served = body.clone();
    let server = server(move |_, _| (200, served.clone(), served.len()));
    let root = tempfile::tempdir().expect("tempdir");
    let error = fetch(root.path(), &server.url, &want).expect_err("mismatch");
    println!("{error}");
    let text = error.to_string();
    assert!(
        text.contains(&want.to_string()) && text.contains(&got.to_string()),
        "{text}"
    );
    assert!(text.contains(&server.url), "{text}");
    let store = Store::open(root.path().join("store")).expect("store");
    assert!(
        !partial_dir(&store)
            .join(format!("{}.part", want.hex()))
            .exists()
    );
    assert!(step_records(&logs_dir(&store)).is_empty());
}

#[test]
fn the_nodes_register() {
    let mut registry = daedalus::prelude::Registry::new();
    registry
        .register_all(&gaia_source_files::NODES)
        .expect("registers");
}
