//! `gaia.generate`: a tool run on a board's description, with a shell script
//! as the tool. Runs everywhere.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::{generate, unknown_tokens};
use gaia_types::Tree;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// The stand-in tool: `gen SOCS DESCRIPTION OUT SLOT [extra]`. It writes a
/// file from the description and the source's data, and a directory.
const TOOL: &str = r#"#!/bin/sh
set -e
printf 'board %s soc %s\n' "$(cat "$2")" "$(cat "$1/soc")" > "$3/board.dts"
mkdir -p "$3/boot"
printf 'slot=%s\n' "$4" > "$3/boot/config.txt"
if [ "$5" = "extra" ]; then echo x > "$3/surprise"; fi
if [ "$5" = "short" ]; then rm "$3/board.dts"; fi
env > "$3/boot/env"
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store").display().to_string();
        Self {
            _dir: dir,
            root,
            store,
        }
    }

    fn tree(&self, name: &str, files: &[(&str, &str, u32)]) -> Tree {
        let dir = self.root.join(name);
        for (path, text, mode) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, text).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).expect("chmod");
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    fn run(
        &self,
        name: &str,
        source_extra: &str,
        args: &str,
        slot: &str,
    ) -> Result<PathBuf, String> {
        let tool = self.tree("tool", &[("bin/gen", TOOL, 0o755)]);
        let source = self.tree(
            &format!("source-{name}"),
            &[
                ("socs/soc", "rk", 0o644),
                ("src/main.rs", source_extra, 0o644),
            ],
        );
        let description = self.tree("description", &[("daemon", "zero", 0o644)]);
        let made = generate(
            &self.store,
            tool,
            "bin/gen",
            source,
            description,
            "daemon",
            args,
            r#"["board.dts", "boot"]"#,
            slot,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        Ok(made.object.path_in(&store).expect("in the store"))
    }

    fn made(&self) -> usize {
        let store = Store::open(&self.store).expect("store");
        step_records(&logs_dir(&store))
            .into_iter()
            .filter(|r| r.step == "gaia.generate" && r.made)
            .count()
    }
}

const ARGS: &str = r#"["@source@/socs", "@description@", "@out@", "@slot@"]"#;

#[test]
fn the_tool_writes_the_declared_outputs_from_the_description() {
    let f = Fixture::new();
    let out = f.run("a", "one", ARGS, "base").expect("generated");
    let read = |name: &str| std::fs::read_to_string(out.join(name)).expect(name);
    assert_eq!(read("board.dts"), "board zero soc rk\n");
    assert_eq!(read("boot/config.txt"), "slot=base\n");
    // Nothing of the caller's environment reaches the tool.
    let env = read("boot/env");
    assert!(!env.contains("HOME=") && !env.contains("USER="), "{env}");

    // The same again is found in the store; another slot is another run.
    f.run("a", "one", ARGS, "base").expect("again");
    assert_eq!(f.made(), 1);
    let other = f.run("a", "one", ARGS, "trial").expect("another slot");
    assert_eq!(
        std::fs::read_to_string(other.join("boot/config.txt")).expect("read"),
        "slot=trial\n"
    );
    assert_eq!(f.made(), 2);
}

#[test]
fn the_source_is_in_the_key_only_when_an_argument_names_it() {
    let f = Fixture::new();
    // Not pointed at the source: its content does not matter. (The tool is
    // given a directory of the description's tree for its first argument.)
    let args = r#"["@description@/../../socs", "@description@", "@out@", "x"]"#;
    let _ = f.run("a", "one", args, "");
    let before = f.made();
    let _ = f.run("b", "two", args, "");
    assert_eq!(f.made(), before, "another source, the same step");
    // Pointed at it: a changed source is a new run.
    f.run("c", "one", ARGS, "s").expect("generated");
    let before = f.made();
    f.run("d", "two", ARGS, "s").expect("generated");
    assert_eq!(f.made(), before + 1);
}

#[test]
fn an_output_not_written_and_a_file_not_declared_fail_by_name() {
    let f = Fixture::new();
    let extra = r#"["@source@/socs", "@description@", "@out@", "s", "extra"]"#;
    let why = f
        .run("a", "one", extra, "")
        .expect_err("an undeclared file");
    println!("{why}");
    assert!(
        why.contains("bin/gen did not write what is declared"),
        "{why}"
    );
    assert!(why.contains("written and not declared: surprise"), "{why}");
    let short = r#"["@source@/socs", "@description@", "@out@", "s", "short"]"#;
    let why = f.run("a", "one", short, "").expect_err("a missing output");
    assert!(why.contains("declared and not written: board.dts"), "{why}");
}

#[test]
fn an_unknown_name_and_a_slot_the_image_does_not_have_are_refused() {
    let f = Fixture::new();
    let why = f
        .run(
            "a",
            "one",
            r#"["@sorce@/socs", "@description@", "@out@"]"#,
            "",
        )
        .expect_err("a misspelt name");
    assert!(
        why.contains("names @sorce@; the names are @source@, @description@, @out@, @slot@"),
        "{why}"
    );
    let why = f.run("a", "one", ARGS, "").expect_err("no slot");
    assert!(
        why.contains("names @slot@, and the image has no slot id"),
        "{why}"
    );
    let why = f.run("a", "one", "not json", "").expect_err("not a list");
    assert!(
        why.contains("the arguments are not a list of strings"),
        "{why}"
    );

    assert_eq!(unknown_tokens("a@b.c and user@host"), Vec::<String>::new());
    assert_eq!(unknown_tokens("@out@/x @other@"), ["@other@"]);
}
