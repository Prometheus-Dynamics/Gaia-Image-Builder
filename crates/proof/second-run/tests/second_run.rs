//! The A3 proof: the A2 graph run again, each run in a process of its own,
//! over one store. Two chains, each from a local git repository made here.

use gaia_step::{StepRecord, logs_dir, step_records};
use gaia_store::{ObjectRef, Store};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT: &str = "/usr/bin/git";

/// A one-file crate with a binary named `bin`, committed with fixed
/// authors and dates. Returns the commit.
fn repository(dir: &Path, bin: &str) -> String {
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{bin}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("write");
    std::fs::write(
        dir.join("Cargo.lock"),
        format!("version = 4\n\n[[package]]\nname = \"{bin}\"\nversion = \"0.1.0\"\n"),
    )
    .expect("write");
    std::fs::write(
        dir.join("src/main.rs"),
        format!("fn main() {{\n    println!(\"{bin}\");\n}}\n"),
    )
    .expect("write");
    git(dir, &["init", "--quiet", "--initial-branch", "main"]);
    commit(dir, "first")
}

fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "--all"]);
    git(dir, &["commit", "--quiet", "--message", message]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(GIT)
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "proof")
        .env("GIT_AUTHOR_EMAIL", "proof@localhost")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "proof")
        .env("GIT_COMMITTER_EMAIL", "proof@localhost")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).expect("utf-8")
}

fn toolchain() -> String {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc --print sysroot");
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

struct Chain {
    label: &'static str,
    repository: PathBuf,
    commit: String,
    bin: &'static str,
}

/// What one run in a new process did.
#[derive(Debug)]
struct Run {
    pid: u32,
    tars: BTreeMap<String, String>,
    records: Vec<StepRecord>,
    run_logs: usize,
}

impl Run {
    fn made(&self) -> Vec<&StepRecord> {
        self.records.iter().filter(|record| record.made).collect()
    }

    /// `(step, hit or made)` for each record, sorted.
    fn summary(&self) -> Vec<(String, &'static str)> {
        let mut summary: Vec<_> = self
            .records
            .iter()
            .map(|record| {
                (
                    record.step.clone(),
                    if record.made { "made" } else { "hit" },
                )
            })
            .collect();
        summary.sort();
        summary
    }
}

fn run(root: &Path, chains: &[Chain]) -> Run {
    let store = root.join("store");
    let mut command = Command::new(env!("CARGO_BIN_EXE_gaia-second-run"));
    command.arg(&store).arg(GIT).arg(toolchain());
    for chain in chains {
        command
            .arg(chain.label)
            .arg(format!("file://{}", chain.repository.display()))
            .arg(&chain.commit)
            .arg(chain.bin);
    }
    let child = command
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let pid = child.id();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success(), "the run failed: {out:?}");
    let tars = String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(label, digest)| (label.to_string(), digest.to_string()))
        .collect();
    let logs = logs_dir(&Store::open(&store).expect("store"));
    let records = step_records(&logs)
        .into_iter()
        .filter(|record| record.pid == pid)
        .collect();
    Run {
        pid,
        tars,
        records,
        run_logs: run_logs(&logs),
    }
}

/// How many tool run logs there are.
fn run_logs(logs: &Path) -> usize {
    std::fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|step| std::fs::read_dir(step.path()).map_or(0, Iterator::count))
        .sum()
}

/// The object a chain's cargo step made: found by the `--bin` in its run log.
fn binary_of(root: &Path, first: &Run, bin: &str) -> ObjectRef {
    let logs = root.join("logs").join("gaia.proof.cargo");
    let key = std::fs::read_dir(&logs)
        .expect("ls")
        .flatten()
        .map(|log| std::fs::read_to_string(log.path()).expect("read"))
        .find(|text| text.contains(&format!("--bin {bin} ")))
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("# key ").map(str::to_string))
        })
        .expect("the cargo run log");
    first
        .records
        .iter()
        .find(|record| record.key.0.to_string() == key)
        .map(|record| record.outputs["bin"])
        .expect("the cargo record")
}

fn show(name: &str, run: &Run) {
    println!(
        "{name} (pid {}): {} made, {} hit; tool run logs in total: {}",
        run.pid,
        run.made().len(),
        run.records.len() - run.made().len(),
        run.run_logs
    );
    for record in &run.records {
        let outputs: Vec<String> = record
            .outputs
            .iter()
            .map(|(name, object)| format!("{name}={object}"))
            .collect();
        println!(
            "  {:4} {:16} {} {}",
            if record.made { "made" } else { "hit" },
            record.step,
            &record.key.0.to_string()[..19],
            outputs.join(" ")
        );
    }
    println!("  tars: {:?}", run.tars);
}

#[test]
fn the_second_run_makes_nothing_and_an_edit_remakes_one_chain() {
    let root = tempfile::tempdir().expect("tempdir");
    let root = root.path();
    let mut chains = vec![
        Chain {
            label: "a",
            repository: root.join("repo-a"),
            commit: String::new(),
            bin: "alpha",
        },
        Chain {
            label: "b",
            repository: root.join("repo-b"),
            commit: String::new(),
            bin: "beta",
        },
    ];
    for chain in &mut chains {
        chain.commit = repository(&chain.repository, chain.bin);
    }
    let every = |made: &'static str| {
        let mut all = vec![
            ("gaia.proof.cargo".to_string(), made),
            ("gaia.proof.cargo".to_string(), made),
            ("gaia.proof.git".to_string(), made),
            ("gaia.proof.git".to_string(), made),
            ("gaia.proof.pack".to_string(), made),
            ("gaia.proof.pack".to_string(), made),
        ];
        all.sort();
        all
    };

    // First run: everything is made.
    let first = run(root, &chains);
    show("first run", &first);
    assert_eq!(first.summary(), every("made"));

    // Second run, a new process: nothing is made, no tool runs, same tars.
    let second = run(root, &chains);
    show("second run", &second);
    assert_eq!(second.summary(), every("hit"));
    assert_eq!(second.run_logs, first.run_logs, "a tool ran");
    assert_eq!(second.tars, first.tars);

    // One artifact deleted from the store: chain a's binary.
    let binary = binary_of(root, &first, "alpha");
    let store = Store::open(root.join("store")).expect("store");
    std::fs::remove_file(store.path(&binary)).expect("rm");
    println!("deleted {binary}");
    let deleted = run(root, &chains);
    show("after deleting chain a's binary", &deleted);
    let made: Vec<_> = deleted
        .made()
        .iter()
        .map(|record| (record.step.as_str(), record.outputs.get("bin").copied()))
        .collect();
    // Only the step that made it runs. It makes the same bytes, so the
    // steps after it keep their keys and are found.
    assert_eq!(made, [("gaia.proof.cargo", Some(binary))]);
    assert_eq!(deleted.tars, first.tars);

    // One file edited in chain a's source.
    std::fs::write(
        chains[0].repository.join("src/main.rs"),
        "fn main() {\n    println!(\"alpha, edited\");\n}\n",
    )
    .expect("edit");
    chains[0].commit = commit(&chains[0].repository, "edit");
    let edited = run(root, &chains);
    show("after editing one file in chain a", &edited);
    let mut made: Vec<_> = edited
        .made()
        .iter()
        .map(|record| record.step.as_str())
        .collect();
    made.sort_unstable();
    assert_eq!(
        made,
        ["gaia.proof.cargo", "gaia.proof.git", "gaia.proof.pack"]
    );
    // The three steps found are chain b's, with the outputs they had: its
    // pack step's tar is chain b's tar.
    let found: Vec<_> = edited
        .records
        .iter()
        .filter(|record| !record.made)
        .collect();
    assert_eq!(found.len(), 3);
    for record in &found {
        assert!(
            first
                .records
                .iter()
                .any(|old| old.key == record.key && old.outputs == record.outputs),
            "{record:?}"
        );
    }
    assert!(found.iter().any(|record| record.step == "gaia.proof.pack"
        && record.outputs["tar"].digest.to_string() == first.tars["b"]));
    assert_ne!(edited.tars["a"], first.tars["a"]);
    assert_eq!(edited.tars["b"], first.tars["b"]);
}
