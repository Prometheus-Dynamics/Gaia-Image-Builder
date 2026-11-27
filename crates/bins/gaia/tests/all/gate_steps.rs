//! `scripts/gate.sh` fails when any of its steps fails, names it, and
//! records a pass only when every one passed; its steps run side by side. The real gate scripts run in a repository of
//! their own, with `cargo`, `rustc` and the built `gaia` replaced by stubs
//! that pass, or fail where a case plants the failure. Each step is planted
//! in turn, the second of the two test commands among them: a red step
//! followed by a green one must still fail the gate.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts")
}

fn write(path: &Path, text: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// `cargo`: logs its arguments and the commit it would record, fails when
/// they contain `$GATE_FAIL`; `nextest run` says its cases started and runs
/// them until clippy has run (10 s at most), then logs `cases ended`; and
/// for `build -p gaia` writes a `gaia` whose `store verify` checks each
/// file object against its name, as the real one does.
const CARGO: &str = r#"#!/bin/sh
echo "$* [commit=${GAIA_COMMIT:-}]" >> "$GATE_LOG"
case " $* " in *" $GATE_FAIL "*) echo "planted failure: cargo $*" >&2; exit 101 ;; esac
case "$1" in
  nextest)
    case "$2" in
      --version) echo "cargo-nextest 0.9.0" ;;
      run)
        echo "    Starting 1 tests across 1 binary" >&2
        i=0
        while [ $i -lt 50 ] && ! grep -q '^clippy' "$GATE_LOG"; do sleep 0.2; i=$((i + 1)); done
        echo "cases ended [commit=${GAIA_COMMIT:-}]" >> "$GATE_LOG" ;;
    esac ;;
  metadata) echo '{"packages":[],"workspace_members":[],"resolve":null}' ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/debug"
    cat > "$CARGO_TARGET_DIR/debug/gaia" <<'EOF'
#!/bin/sh
for f in "$4"/objects/file/*/*; do
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "$(basename "$f")" ] || exit 2
done
EOF
    chmod +x "$CARGO_TARGET_DIR/debug/gaia" ;;
esac
exit 0
"#;

const RUSTC: &str = "#!/bin/sh\necho 'host: x86_64-unknown-linux-gnu'\n";

struct Run {
    code: Option<i32>,
    recorded: bool,
    cargo: String,
    output: String,
}

/// The gate, in a fresh repository with its own pass records, with the
/// cargo command that contains `fail` (a space-separated word) failing.
fn gate(dir: &Path, fail: &str) -> Run {
    gate_on(dir, fail, false)
}

/// [`gate`], as on a CI runner when `ci`.
fn gate_on(dir: &Path, fail: &str, ci: bool) -> Run {
    let repo = dir.join("repo");
    let bin = dir.join("bin");
    let records = dir.join("records");
    let log = dir.join("cargo.log");
    for name in [
        "gate.sh",
        "gate-lanes.py",
        "gate-target.sh",
        "nice.sh",
        "test-bound.py",
        "product-names",
    ] {
        let text = std::fs::read_to_string(scripts().join(name)).expect("a gate script");
        write(&repo.join("scripts").join(name), &text, 0o755);
    }
    write(
        &repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*/*\"]\n",
        0o644,
    );
    write(
        &repo.join("crates/core/x/Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        0o644,
    );
    write(&repo.join("crates/core/x/src/lib.rs"), "\n", 0o644);
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    write(&bin.join("cargo"), CARGO, 0o755);
    write(&bin.join("rustc"), RUSTC, 0o755);
    write(&log, "", 0o644);

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    if ci {
        command.env("GITHUB_ACTIONS", "true");
    } else {
        command.env_remove("GITHUB_ACTIONS");
    }
    let out = command
        .arg(repo.join("scripts/gate.sh"))
        .current_dir(&repo)
        .env("PATH", path)
        .env("GATE_FAIL", fail)
        .env("GATE_LOG", &log)
        .env("GAIA_COMMIT", "from-the-caller")
        .env("GAIA_GATE_RECORDS", &records)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("TMPDIR", dir)
        .env_remove("GAIA_GATE_FULL")
        .output()
        .expect("the gate runs");
    Run {
        code: out.status.code(),
        recorded: std::fs::read_dir(&records).is_ok_and(|mut d| d.next().is_some()),
        cargo: std::fs::read_to_string(&log).expect("log"),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

#[test]
fn a_gate_with_every_step_green_passes_and_records_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = gate(dir.path(), "no-such-word");
    assert_eq!(run.code, Some(0), "{}", run.output);
    assert!(run.recorded, "the pass is recorded");
    assert!(run.output.contains("gate passed"), "{}", run.output);
    // Every step ran: the doc tests after the cases, the build for the
    // store check last.
    for step in [
        "metadata",
        "fmt",
        "--no-run",
        "clippy",
        "nextest run",
        "--doc",
        "build",
    ] {
        assert!(run.cargo.contains(step), "{step} ran: {}", run.cargo);
    }
    // Side by side: clippy ran while the cases were running.
    let at = |word: &str| {
        run.cargo
            .find(word)
            .unwrap_or_else(|| panic!("{word}: {}", run.cargo))
    };
    assert!(
        at("nextest run") < at("clippy") && at("clippy") < at("cases ended"),
        "{}",
        run.cargo
    );
    // Every build in the gate records `gate` as its commit, whatever the
    // caller set: no binary it tests carries a HEAD.
    for line in run.cargo.lines() {
        assert!(line.ends_with(" [commit=gate]"), "{line}");
    }
}

#[test]
fn a_failing_step_fails_the_gate_and_records_nothing() {
    // Each step, side by side with others or not, fails the gate by name.
    for (fail, step) in [
        ("metadata", "lock"),
        ("fmt", "step 'format'"),
        ("--no-run", "step 'test build'"),
        ("run", "step 'tests'"),
        ("clippy", "step 'clippy'"),
        ("--doc", "step 'doc tests'"),
        ("build", "step 'store verify'"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = gate(dir.path(), fail);
        assert!(
            run.output.contains(step),
            "{fail}: {step} not named: {}",
            run.output
        );
        assert_ne!(
            run.code,
            Some(0),
            "{fail} failed, the gate passed: {}",
            run.output
        );
        assert!(!run.recorded, "{fail} failed, a pass was recorded");
        assert!(
            !run.output.contains("gate passed"),
            "{fail}: {}",
            run.output
        );
        assert!(
            run.output.contains("planted failure"),
            "{fail} was run: {}",
            run.output
        );
    }
}

#[test]
fn a_check_that_fails_while_the_cases_pass_fails_the_gate_by_name() {
    // Locally clippy fails while nextest runs its cases, which pass: the
    // gate stops the cases and the rest of clippy's line. On CI the steps
    // run one after another and clippy's failure ends the gate before the
    // cases. Red either way, clippy named, nothing recorded.
    for ci in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = gate_on(dir.path(), "clippy", ci);
        assert_ne!(run.code, Some(0), "ci {ci}: {}", run.output);
        assert!(!run.recorded, "ci {ci}: a pass was recorded");
        assert!(
            run.output.contains("gate failed: step 'clippy'"),
            "ci {ci}: {}",
            run.output
        );
        assert!(
            !run.output.contains("gate passed"),
            "ci {ci}: {}",
            run.output
        );
        assert!(!run.cargo.contains("--doc"), "ci {ci}: {}", run.cargo);
        assert!(!run.cargo.contains("cases ended"), "ci {ci}: {}", run.cargo);
        if ci {
            assert!(!run.cargo.contains("nextest run"), "{}", run.cargo);
        } else {
            assert!(
                run.output.contains("step 'tests' was stopped"),
                "{}",
                run.output
            );
        }
    }
}

#[test]
fn on_ci_the_steps_run_one_after_another_and_locally_side_by_side() {
    for ci in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = gate_on(dir.path(), "no-such-word", ci);
        assert_eq!(run.code, Some(0), "ci {ci}: {}", run.output);
        assert!(run.recorded, "ci {ci}: the pass is recorded");
        let at = |word: &str| {
            run.cargo
                .find(word)
                .unwrap_or_else(|| panic!("ci {ci}, {word}: {}", run.cargo))
        };
        if ci {
            // fmt, clippy, the cases to their end, the doc tests, the store
            // check: each after the one before.
            let order = [
                "fmt",
                "clippy",
                "nextest run",
                "cases ended",
                "--doc",
                "build",
            ];
            for pair in order.windows(2) {
                assert!(at(pair[0]) < at(pair[1]), "{pair:?}: {}", run.cargo);
            }
            assert!(!run.cargo.contains("--no-run"), "{}", run.cargo);
        } else {
            assert!(
                at("nextest run") < at("clippy") && at("clippy") < at("cases ended"),
                "{}",
                run.cargo
            );
        }
    }
}
