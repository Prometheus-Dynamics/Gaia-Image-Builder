//! Real Buildroot builds of the caches: the download directory filled from a
//! lock, the offline build that uses it, and a make that is killed and
//! resumed. Each is gated and prints SKIPPED without its gate.
//!
//! - `GAIA_BUILDROOT_FETCH=1`: `fetch_the_lock` fetches every download in the
//!   lock into the store, with curl (needs the network).
//! - `GAIA_BUILDROOT_TESTS=1`: `offline_build` fills a fresh download
//!   directory from the store and builds with it. Run it with the network
//!   off (see the PR). It never calls curl.
//! - `GAIA_BUILDROOT_RESUME=1`: `killed_make_resumes` kills make by its PID
//!   and runs it again in the same output directory.
//!
//! Shared environment: `GAIA_BUILDROOT_SOURCE` (a Buildroot tree),
//! `GAIA_BUILDROOT_LOCK` (the lock), `GAIA_STORE` (the content store),
//! `GAIA_BUILDROOT_DL` (the download directory to fill), `GAIA_BUILDROOT_CCACHE`
//! (a ccache directory), `GAIA_BUILDROOT_CPUS` (a CPU list for taskset),
//! `GAIA_BUILDROOT_JOBS`, and for the resume test `GAIA_BUILDROOT_RESUME_DIR`
//! (its output directory, which must not exist yet).

use gaia_lock::Lock;
use gaia_os_buildroot::{BuildrootHost, ConfigFragment, OsConfig, build_os};
use gaia_os_buildroot_cache::{
    ccache_fragment, fill, interrupted, locked_file, os_buildroot_downloads, settings,
};
use gaia_source_files::source_download;
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_store::{Kind, ObjectRef};
use gaia_types::{Artifact, ArtifactKind, StoreRef, Tree};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

const BASE: &str = "\
BR2_aarch64=y
BR2_TOOLCHAIN_EXTERNAL=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_BLEEDING_EDGE=y
BR2_PACKAGE_BUSYBOX=y
BR2_TARGET_ROOTFS_CPIO=y
BR2_TARGET_ROOTFS_CPIO_GZIP=y
";

fn gate(name: &str) -> bool {
    if std::env::var_os(name).is_none() {
        println!("SKIPPED: set {name}=1");
        return false;
    }
    true
}

fn env_abs(name: &str) -> PathBuf {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let path = PathBuf::from(value);
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn cpus() -> String {
    std::env::var("GAIA_BUILDROOT_CPUS").unwrap_or_default()
}

fn jobs() -> u32 {
    std::env::var("GAIA_BUILDROOT_JOBS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4)
}

/// The lock's downloads, by package name, with their digests.
fn lock_downloads() -> Vec<(String, gaia_lock::Entry)> {
    let lock = Lock::read(&env_abs("GAIA_BUILDROOT_LOCK")).expect("the lock reads");
    lock.entries
        .into_iter()
        .filter(|(_, entry)| entry.kind() == "download")
        .collect()
}

#[test]
fn fetch_the_lock() {
    if !gate("GAIA_BUILDROOT_FETCH") {
        return;
    }
    let store = env_abs("GAIA_STORE");
    for (name, entry) in lock_downloads() {
        let gaia_lock::Entry::Download { url, digest } = &entry else {
            unreachable!("filtered to downloads")
        };
        let artifact = source_download(
            &store.display().to_string(),
            "/usr/bin/curl",
            url,
            digest,
            "archive",
            &Ctx::new(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        println!(
            "fetched {name}: {}",
            artifact.object.object_ref().expect("ref").digest
        );
    }
}

#[test]
fn offline_build() {
    if !gate("GAIA_BUILDROOT_TESTS") {
        return;
    }
    let store_dir = env_abs("GAIA_STORE");
    let store = Store::open(&store_dir).expect("store");
    let dl = env_abs("GAIA_BUILDROOT_DL");

    // Each locked file, laid out as Buildroot reads it. The store already
    // holds it under its digest (the fetch made that), so no network is used.
    let files: Vec<_> = lock_downloads()
        .into_iter()
        .map(|(name, entry)| {
            let gaia_lock::Entry::Download { digest, .. } = &entry else {
                unreachable!("filtered to downloads")
            };
            let want = gaia_store::Digest::parse(digest).expect("digest");
            let artifact = Artifact {
                kind: ArtifactKind::parse("archive"),
                object: StoreRef::from(ObjectRef {
                    kind: Kind::File,
                    digest: want,
                }),
            };
            locked_file(&name, &entry, artifact).expect("laid out")
        })
        .collect();
    let first = os_buildroot_downloads(
        &store_dir.display().to_string(),
        &dl.display().to_string(),
        files.clone(),
    )
    .expect("the download directory fills");
    println!(
        "download directory {}: {} file(s) copied",
        first.dir, first.copied
    );
    let second = fill(&store, &dl, &files).expect("second fill");
    println!("second fill: {second} file(s) copied");
    assert_eq!(second, 0, "a second fill copies nothing");

    let ccache = ccache_fragment(&env_abs("GAIA_BUILDROOT_CCACHE").display().to_string())
        .expect("ccache fragment");
    let fragments = vec![
        ConfigFragment {
            name: "minimal-aarch64".into(),
            text: BASE.into(),
        },
        ccache,
    ];
    let source = Tree {
        object: store
            .put_tree(&env_abs("GAIA_BUILDROOT_SOURCE"))
            .expect("source into the store")
            .into(),
        entries: Vec::new(),
    };
    let host = BuildrootHost {
        make: "/usr/bin/make".into(),
        taskset: "/usr/bin/taskset".into(),
        cpus: cpus(),
        jobs: jobs(),
        download_dir: dl.display().to_string(),
        config_only: false,
    };
    let (rootfs, _kernel, _dt, _modules, _bootloader, images, config, _kernel_build, _toolchain) =
        build_os(
            &store_dir.display().to_string(),
            source,
            &OsConfig {
                fragments,
                ..OsConfig::default()
            },
            &[],
            &gaia_os_buildroot::no_seed(&store_dir.display().to_string()).expect("seed"),
            &host,
            &Ctx::new(),
        )
        .expect("the offline Buildroot build");
    let made = step_records(&logs_dir(&store))
        .into_iter()
        .rev()
        .find(|record| record.step == "gaia.os.buildroot")
        .expect("a step record")
        .made;
    println!("offline build made={made}");
    assert!(
        rootfs
            .object
            .path_in(&store)
            .expect("rootfs")
            .join("bin/busybox")
            .is_file()
    );
    assert!(
        images
            .object
            .path_in(&store)
            .expect("images")
            .join("rootfs.cpio.gz")
            .is_file()
    );
    let text = fs::read_to_string(config.object.path_in(&store).expect("config")).expect("config");
    assert!(
        text.contains("BR2_CCACHE=y"),
        "the ccache fragment is in the resolved config"
    );
}

/// The make command D1's node runs, for `goals`, in `out`.
fn make(source: &Path, out: &Path, dl: &Path, goals: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/taskset");
    command
        .arg("-c")
        .arg(cpus())
        .arg("/usr/bin/make")
        .arg("--no-print-directory")
        .arg(format!("-j{}", jobs()))
        .arg(format!("O={}", out.display()))
        .arg(format!("BR2_DL_DIR={}", dl.display()))
        .args(goals)
        .current_dir(source)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("HOME", out.display().to_string());
    command
}

/// Writes `out/.config`: the base with each symbol the fragments set replaced,
/// then resolved by `olddefconfig`.
fn configure(source: &Path, out: &Path, dl: &Path, fragments: &[ConfigFragment]) {
    assert!(
        make(source, out, dl, &["allnoconfig"])
            .status()
            .expect("make")
            .success()
    );
    let config = out.join(".config");
    let base = fs::read_to_string(&config).expect("base config");
    let values = settings(fragments).expect("settings");
    let mut text = String::new();
    for line in base.lines() {
        let symbol = line
            .strip_prefix("# ")
            .and_then(|rest| rest.strip_suffix(" is not set"))
            .or_else(|| line.split_once('=').map(|(name, _)| name));
        if !symbol.is_some_and(|name| values.contains_key(name)) {
            text.push_str(line);
            text.push('\n');
        }
    }
    for (symbol, value) in &values {
        if value == "unset" {
            text.push_str(&format!("# {symbol} is not set\n"));
        } else {
            text.push_str(&format!("{symbol}={value}\n"));
        }
    }
    fs::write(&config, text).expect("merged config");
    assert!(
        make(source, out, dl, &["olddefconfig"])
            .status()
            .expect("make")
            .success()
    );
}

/// Each `>>> <name> <version> Building` line, as `<name>-<version>`.
fn building(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|line| {
            let rest = line.split_once(">>> ")?.1.strip_suffix(" Building")?;
            let mut parts = rest.split_whitespace();
            Some(format!("{}-{}", parts.next()?, parts.next()?))
        })
        .collect()
}

/// Runs make in `out` and kills it by its PID once it has started its fourth
/// package, returning the log so far. The make's whole group is killed too,
/// so no compile of it outlives the kill.
fn run_and_kill(source: &Path, out: &Path, dl: &Path, log: &Path) -> String {
    let mut child = make(source, out, dl, &[])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn make");
    let pid = child.id();
    let (sender, receiver) = mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let out_sender = sender.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = out_sender.send(line);
        }
    });
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let mut seen = String::new();
    let mut started = 0;
    while let Ok(line) = receiver.recv_timeout(std::time::Duration::from_secs(1800)) {
        seen.push_str(&line);
        seen.push('\n');
        if line.contains(" Building") && line.contains(">>> ") {
            started += 1;
            if started == 4 {
                break;
            }
        }
    }
    child.kill().expect("kill make by its PID");
    let status = child.wait().expect("reap make");
    assert!(!status.success(), "make was killed, not finished");
    // The compiles it started are in its process group: they go too.
    let _ = Command::new("/usr/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .status();
    while let Ok(line) = receiver.recv_timeout(std::time::Duration::from_secs(5)) {
        seen.push_str(&line);
        seen.push('\n');
    }
    fs::write(log, &seen).expect("log");
    seen
}

#[test]
fn killed_make_resumes() {
    if !gate("GAIA_BUILDROOT_RESUME") {
        return;
    }
    let source = env_abs("GAIA_BUILDROOT_SOURCE");
    let out = env_abs("GAIA_BUILDROOT_RESUME_DIR");
    assert!(
        !out.exists(),
        "the resume output directory must not exist yet"
    );
    let dl = env_abs("GAIA_BUILDROOT_DL");
    fs::create_dir_all(&out).expect("output dir");
    let fragments = vec![ConfigFragment {
        name: "minimal-aarch64".into(),
        text: BASE.into(),
    }];
    configure(&source, &out, &dl, &fragments);

    interrupted::start(&out).expect("marker");
    let first_log = out.with_file_name(format!(
        "{}-run1.log",
        out.file_name().expect("name").to_string_lossy()
    ));
    let seen = run_and_kill(&source, &out, &dl, &first_log);
    let built_first = building(&seen);
    println!("run 1 (killed): started {built_first:?}");
    assert!(
        interrupted::was_killed(&out),
        "the marker says the make was killed"
    );
    let killed = interrupted::in_progress(&out).expect("in progress");
    println!("in progress when killed: {killed:?}");
    assert!(!killed.is_empty(), "a package was in progress");
    let finished: Vec<String> = fs::read_dir(out.join("build"))
        .expect("build dir")
        .flatten()
        .filter(|entry| entry.path().join(".stamp_installed").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    println!("finished before the kill: {finished:?}");
    assert!(
        !finished.is_empty(),
        "some packages finished before the kill"
    );

    let redone = interrupted::redo(&out).expect("redo");
    println!("redone: {redone:?}");
    assert_eq!(redone, killed);

    let second_log = first_log.with_file_name("run2.log");
    let output = make(&source, &out, &dl, &[]).output().expect("second make");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(&second_log, &log).expect("run2 log");
    assert!(output.status.success(), "the second make finishes:\n{log}");
    let built_again = building(&log);
    println!("run 2: built {built_again:?}");
    for done in &finished {
        assert!(
            !built_again.contains(done),
            "{done} finished before the kill and was built again"
        );
    }
    for name in &killed {
        assert!(
            built_again.contains(name),
            "{name} was in progress and is built again"
        );
    }
    assert!(out.join("images/rootfs.cpio.gz").is_file());
}
