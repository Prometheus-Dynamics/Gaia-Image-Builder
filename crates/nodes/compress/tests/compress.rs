//! Same input, same bytes: at every thread count, and in every store.

use gaia_compress::{compress_gzip, compress_xz, compress_zstd, tar_tree};
use gaia_step::{Ctx, logs_dir, step_records};
use gaia_store::{Digest, Store};
use gaia_types::{Artifact, ArtifactKind, FileEntry, Tree};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn digest_of(artifact: &Artifact) -> Digest {
    artifact.object.object_ref().expect("object").digest
}

/// 12 MiB that compresses but not to nothing: words from a fixed
/// pseudo-random sequence, enough for four xz blocks at -1 (3 MiB each)
/// and two zstd jobs at -3 (8 MiB each).
fn input(dir: &Path) -> std::path::PathBuf {
    const WORDS: [&str; 16] = [
        "gaia", "node", "tree", "store", "image", "board", "step", "key", "lane", "boot", "root",
        "disk", "part", "sign", "tar", "zstd",
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut data = Vec::with_capacity(12 << 20);
    while data.len() < 12 << 20 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        data.extend_from_slice(WORDS[(state >> 60) as usize].as_bytes());
        data.push(if (state >> 50) & 7 == 0 { b'\n' } else { b' ' });
        data.extend_from_slice(&(state >> 40).to_le_bytes()[..1]);
    }
    let path = dir.join("input.bin");
    std::fs::write(&path, data).expect("write");
    path
}

fn stored(store: &Store, path: &Path) -> Artifact {
    Artifact {
        kind: ArtifactKind::parse("disk"),
        object: store.put_file(path).expect("put").into(),
    }
}

#[test]
fn compression_is_the_same_at_every_thread_count() {
    let root = tempfile::tempdir().expect("tempdir");
    let store_root = root.path().join("store");
    let store = Store::open(&store_root).expect("store");
    let source = input(root.path());
    let artifact = stored(&store, &source);
    let original = std::fs::read(&source).expect("read");
    for (format, level) in [("xz", 1), ("zstd", 3)] {
        let mut seen = Vec::new();
        for threads in [1, 2, 4, 8] {
            // A store per run, so every run compresses rather than finds.
            let fresh = root
                .path()
                .join(format!("store-{format}-{level}-{threads}"));
            let fresh_str = fresh.display().to_string();
            let fresh_store = Store::open(&fresh).expect("store");
            let artifact = stored(&fresh_store, &source);
            let start = std::time::Instant::now();
            let out = match format {
                "xz" => compress_xz(&fresh_str, artifact, level, threads, &Ctx::new()),
                _ => compress_zstd(&fresh_str, artifact, level, threads, &Ctx::new()),
            }
            .expect("compresses");
            let path = out.object.path_in(&fresh_store).expect("in store");
            println!(
                "{format} -{level} threads {threads}: {} ({} bytes, {} ms)",
                digest_of(&out),
                std::fs::metadata(&path).expect("size").len(),
                start.elapsed().as_millis()
            );
            assert_eq!(out.kind.as_str(), format!("disk+{format}"));
            seen.push(digest_of(&out));
            if threads == 1 {
                let mut back = Vec::new();
                let file = std::fs::File::open(&path).expect("open");
                match format {
                    "xz" => xz2::read::XzDecoder::new_multi_decoder(file).read_to_end(&mut back),
                    _ => zstd::stream::read::Decoder::new(file)
                        .expect("zstd")
                        .read_to_end(&mut back),
                }
                .expect("decodes");
                assert!(back == original, "{format} round trip");
            }
        }
        assert!(
            seen.windows(2).all(|w| w[0] == w[1]),
            "{format} -{level}: {seen:?}"
        );
    }
    // The thread count is not in the key: another count finds the stored output.
    let store_str = store_root.display().to_string();
    compress_zstd(&store_str, artifact.clone(), 3, 1, &Ctx::new()).expect("one thread");
    compress_zstd(&store_str, artifact.clone(), 3, 8, &Ctx::new()).expect("eight threads");
    compress_zstd(&store_str, artifact.clone(), 4, 8, &Ctx::new()).expect("another level");
    // Every store here shares the tempdir's logs; these are the last three.
    let records = step_records(&logs_dir(&store));
    let made: Vec<bool> = records[records.len() - 3..]
        .iter()
        .map(|r| r.made)
        .collect();
    assert_eq!(made, [true, false, true]);
}

#[test]
fn gzip_is_the_same_twice_and_has_no_name_or_time() {
    let root = tempfile::tempdir().expect("tempdir");
    let source = root.path().join("small");
    std::fs::write(&source, "gzip me\n".repeat(1000)).expect("write");
    let mut seen = Vec::new();
    for run in 0..2 {
        let dir = root.path().join(format!("store{run}"));
        let store = Store::open(&dir).expect("store");
        let artifact = stored(&store, &source);
        let out =
            compress_gzip(&dir.display().to_string(), artifact, 9, &Ctx::new()).expect("gzip");
        let bytes = std::fs::read(out.object.path_in(&store).expect("path")).expect("read");
        assert_eq!(bytes[3], 0, "no FNAME or other flags");
        assert_eq!(bytes[4..8], [0, 0, 0, 0], "mtime 0");
        println!("gzip -9: {}", digest_of(&out));
        seen.push(digest_of(&out));
    }
    assert_eq!(seen[0], seen[1]);
    let dir = root.path().join("store0");
    let err = compress_gzip(
        &dir.display().to_string(),
        stored(&Store::open(&dir).expect("store"), &source),
        12,
        &Ctx::new(),
    )
    .expect_err("level 12");
    assert!(err.to_string().contains("level 12"), "{err}");
}

/// A tree written with this process's umask and times, so two copies differ
/// in everything a tar must not carry.
fn sample(dir: &Path, salt: u64) {
    std::fs::create_dir_all(dir.join("etc/app")).expect("mkdir");
    std::fs::create_dir_all(dir.join("usr/bin")).expect("mkdir");
    std::fs::write(dir.join("etc/app/config"), "a = 1\n").expect("write");
    std::fs::write(dir.join("usr/bin/app"), "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(
        dir.join("usr/bin/app"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    std::os::unix::fs::symlink("app", dir.join("usr/bin/app-link")).expect("symlink");
    std::thread::sleep(std::time::Duration::from_millis(salt));
}

#[test]
fn a_tree_tars_to_one_digest_in_every_store() {
    let root = tempfile::tempdir().expect("tempdir");
    let entries = vec![
        FileEntry {
            path: "usr/bin/app".into(),
            mode: 0o4755,
            uid: 0,
            gid: 0,
        },
        FileEntry {
            path: "etc/app".into(),
            mode: 0o750,
            uid: 1000,
            gid: 1000,
        },
    ];
    let mut seen = Vec::new();
    for run in 0..2u64 {
        let dir = root.path().join(format!("store{run}"));
        let store = Store::open(&dir).expect("store");
        let src = root.path().join(format!("src{run}"));
        sample(&src, 20 * run);
        let tree = Tree {
            object: store.put_tree(&src).expect("put").into(),
            entries: entries.clone(),
        };
        let archive = tar_tree(&dir.display().to_string(), tree, &Ctx::new()).expect("tar");
        assert_eq!(archive.kind, ArtifactKind::Archive);
        println!("tar in store{run}: {}", digest_of(&archive));
        seen.push(digest_of(&archive));
        if run == 0 {
            let file =
                std::fs::File::open(archive.object.path_in(&store).expect("path")).expect("open");
            let mut listing = Vec::new();
            for member in tar::Archive::new(file).entries().expect("entries") {
                let member = member.expect("member");
                let h = member.header();
                listing.push(format!(
                    "{:o} {}/{} {} {}",
                    h.mode().expect("mode"),
                    h.uid().expect("uid"),
                    h.gid().expect("gid"),
                    h.mtime().expect("mtime"),
                    member.path().expect("path").display()
                ));
            }
            println!("{}", listing.join("\n"));
            assert_eq!(
                listing,
                [
                    "755 0/0 0 etc/",
                    "750 1000/1000 0 etc/app/",
                    "644 0/0 0 etc/app/config",
                    "755 0/0 0 usr/",
                    "755 0/0 0 usr/bin/",
                    "4755 0/0 0 usr/bin/app",
                    "777 0/0 0 usr/bin/app-link",
                ]
            );
        }
    }
    assert_eq!(seen[0], seen[1]);
    // An owner for a path the tree lacks is an error naming it.
    let dir = root.path().join("store0");
    let store = Store::open(&dir).expect("store");
    let tree = Tree {
        object: store
            .put_tree(&root.path().join("src0"))
            .expect("put")
            .into(),
        entries: vec![FileEntry {
            path: "missing/file".into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
        }],
    };
    let err = tar_tree(&dir.display().to_string(), tree, &Ctx::new()).expect_err("missing");
    assert!(err.to_string().contains("missing/file"), "{err}");
}
