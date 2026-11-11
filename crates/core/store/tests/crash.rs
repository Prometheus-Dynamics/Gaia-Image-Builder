//! A process killed mid-commit leaves no partial entry. The test runs this
//! binary again as a child that commits step after step, kills it with
//! SIGKILL at many different moments, and checks the store each time.

use gaia_store::{Check, Digest, Keep, StepKey, Store};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const ROOT_VAR: &str = "GAIA_STORE_CRASH_CHILD_ROOT";

/// Runs only as the child: commits steps until killed.
#[test]
fn crash_child() {
    let Ok(root) = std::env::var(ROOT_VAR) else {
        return;
    };
    let store = Store::open(&root).expect("store");
    let mut step = 0_u64;
    loop {
        step += 1;
        let scratch = store.scratch().expect("scratch");
        let out = scratch.path().join("out");
        for dir in 0..8 {
            fs::create_dir_all(out.join(format!("d{dir}"))).expect("dir");
            for file in 0..40 {
                let bytes = format!("{step} {dir} {file} {}", "x".repeat(2048));
                fs::write(out.join(format!("d{dir}/f{file}")), bytes).expect("write");
            }
        }
        fs::write(scratch.path().join("bin"), step.to_le_bytes()).expect("write");
        let key = StepKey(Digest::of_bytes(&step.to_le_bytes()));
        scratch
            .commit_step(
                &key,
                &[("out", Path::new("out")), ("bin", Path::new("bin"))],
            )
            .expect("commit");
    }
}

#[test]
fn a_crash_mid_commit_leaves_no_partial_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("store");
    let exe = std::env::current_exe().expect("test binary");
    let mut leftovers = 0;
    let mut entries = 0;
    let mut orphans = 0;
    for round in 0..24_u64 {
        let mut child = Command::new(&exe)
            .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
            .env(ROOT_VAR, &root)
            .spawn()
            .expect("spawn child");
        std::thread::sleep(Duration::from_millis(30 + round * 29));
        child.kill().expect("SIGKILL");
        child.wait().expect("reap");

        let store = Store::open(&root).expect("store");
        entries = 0;
        for shard in fs::read_dir(root.join("entries")).expect("ls") {
            for file in fs::read_dir(shard.expect("shard").path()).expect("ls") {
                let name = file
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                let Some(key) = name.strip_suffix(".json").and_then(Digest::parse) else {
                    continue; // a temporary name, never read as an entry
                };
                let entry = store
                    .lookup(&StepKey(key), Check::Digest)
                    .expect("entry parses");
                assert!(
                    entry.is_some(),
                    "round {round}: entry {name} names a missing object"
                );
                entries += 1;
            }
        }
        // Clear what the child had in flight when it died; whole entries
        // stay whole.
        let report = store.retain(&everything(&store), true).expect("retain");
        assert_eq!(report.entries_removed, 0, "round {round}");
        leftovers += report.leftovers_removed;
        // Objects moved in before the kill, whose entry was never written.
        orphans += report.objects_removed;
    }
    assert!(entries > 0, "the child never committed a step");
    assert!(leftovers > 0, "no kill landed mid-step");
    println!(
        "{entries} whole entries at the end; kills left {leftovers} scratch directories or temporary entries and {orphans} objects without an entry; no partial entry"
    );
}

/// Keeps every entry, so retention removes only what a kill left.
fn everything(store: &Store) -> Keep {
    let mut keep = Keep::default();
    for shard in fs::read_dir(store.root().join("entries")).expect("ls") {
        for file in fs::read_dir(shard.expect("shard").path()).expect("ls") {
            let name = file
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if let Some(key) = name.strip_suffix(".json").and_then(Digest::parse) {
                keep.entries.insert(StepKey(key));
            }
        }
    }
    keep
}
