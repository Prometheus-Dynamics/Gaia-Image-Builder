//! Gaia's own changes to the Buildroot source it builds with.
//!
//! Three patches ship in `patches/`, as `git format-patch` writes them for
//! Buildroot (`-p1`). They are applied to a copy of the source before any
//! Buildroot step uses it, so every make (the host stage, its plan, the OS
//! build) runs the same patched tree:
//!
//! - **0001**: per-package trees are cloned with `cp --reflink=auto`
//!   instead of one `rsync`, so assembling `target/` and `host/` makes
//!   copy-on-write clones on a filesystem with reflinks.
//! - **0002**: two `$(shell)` calls made while the Makefiles are read become
//!   `:=` and `firstword`/`patsubst`, with the same values.
//! - **0003**: the host build of dtc takes the machine's bison and flex when
//!   they are at least the versions Buildroot builds for its own host tools,
//!   and builds host-bison and host-flex (with host-m4) only when they are
//!   not. See [`PATCHES`] for the minimum and the reasons.
//!
//! 0001 and 0002 change no output, so they are applied to the exact upstream
//! text, strictly (`patch -p1 --fuzz=0`). 0003 changes what the host stage
//! builds (`changes_outputs`). A patch that does not apply to a Buildroot
//! version is skipped with a warning line that names it: all three are
//! changes a future Buildroot must still build without.
//!
//! The keys: the package-level keys (the host stage's and the plan's, and
//! the OS build's) hash the unpatched source's digest. A patch that changes
//! no output adds nothing to the key when it applies, so the two shipped
//! patches add nothing to any key. A skipped patch, or the off switch, adds
//! an entry, because the build then differs from the default one. A patch
//! that changes outputs (`changes_outputs`) adds its digest when it applies.
//!
//! `GAIA_BUILDROOT_PATCHES=off` builds the upstream source, for debugging.

use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Tool, Work};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const NAME: &str = "gaia.os.buildroot.patch";
/// The work directory this step expects when no earlier run recorded its
/// size. In RAM when there is room (see `gaia_step::Step::ram_work`): a tree
/// of many small files is written once to the store's disk, not written,
/// read back and renamed there.
const WORK_BYTES: u64 = 1 << 30;
/// Changes when how the patches are applied changes.
/// 2: the copy keeps file modes (1 lost them, and Buildroot's scripts with them).
const RECIPE_VERSION: u32 = 2;
/// The environment variable that turns the patches off (`off`) or on.
pub(crate) const ENV: &str = "GAIA_BUILDROOT_PATCHES";
/// The key entry the choice adds, when it is not the default.
pub(crate) const KEY_NAME: &str = "gaia.buildroot-patches";
/// The step's report: one line per patch, `applied <name>` or `skipped <name>`.
const REPORT: &str = "patches.txt";

/// One patch of Buildroot's source, as the text `patch` reads.
pub(crate) struct GaiaPatch {
    pub name: &'static str,
    pub text: &'static str,
    /// Whether the patch changes what Buildroot makes (not only how fast):
    /// when it applies, its digest is in the key of every build.
    pub changes_outputs: bool,
}

/// The patches, in the order they are applied.
///
/// 0003 changes the host build of dtc (`package/dtc/dtc.mk`), so it is
/// `changes_outputs`: once it applies, its digest is in the key of every
/// Buildroot build, so a store entry made without it is never reused with
/// it (and the other way round). The keys of builds without host-dtc change
/// too, once: a coarse key, kept because the entry is the same for all of
/// them. The host plan's values also carry the machine's choice, since they
/// list `HOST_DTC_BISON_OK` and `HOST_DTC_DEPENDENCIES`, so a machine with
/// older tools keys a different host stage.
///
/// The minimum: bison 3.8.2 and flex 2.6.4, the versions Buildroot builds as
/// its own host tools. The dtc 1.7.2 tarball states no minimum: its parser
/// and lexer are generated at build time (`dtc-parser.y` uses `%locations`,
/// `dtc-lexer.l` `%option noyywrap nounput noinput never-interactive`). A
/// program older than the minimum is not tried: the build makes host-bison
/// or host-flex instead, as it did before the patch.
pub(crate) const PATCHES: [GaiaPatch; 3] = [
    GaiaPatch {
        name: "0001-package-pkg-utils-clone-per-package-trees-with-reflinks.patch",
        text: include_str!(
            "../patches/0001-package-pkg-utils-clone-per-package-trees-with-reflinks.patch"
        ),
        changes_outputs: false,
    },
    GaiaPatch {
        name: "0002-package-avoid-shell-calls-while-parsing-the-Makefiles.patch",
        text: include_str!(
            "../patches/0002-package-avoid-shell-calls-while-parsing-the-Makefiles.patch"
        ),
        changes_outputs: false,
    },
    GaiaPatch {
        name: "0003-package-dtc-host-build-uses-the-machine-s-bison-and-flex.patch",
        text: include_str!(
            "../patches/0003-package-dtc-host-build-uses-the-machine-s-bison-and-flex.patch"
        ),
        changes_outputs: true,
    },
];

/// Whether the patches are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    On,
    Off,
}

/// The mode for the value of [`ENV`] (`None` when unset): unset, empty or
/// `on` is on, `off` is off, anything else is an error.
pub(crate) fn mode(value: Option<&str>) -> Result<Mode, String> {
    match value {
        None | Some("") | Some("on") => Ok(Mode::On),
        Some("off") => Ok(Mode::Off),
        Some(other) => Err(format!("{ENV} is {other:?}: it must be on or off")),
    }
}

/// The mode the process's environment asks for.
pub(crate) fn mode_from_env() -> Result<Mode, String> {
    mode(std::env::var(ENV).ok().as_deref())
}

/// A patched source: the tree in the store, and each patch's outcome in
/// [`PATCHES`] order (`true` when it applied).
pub(crate) struct Patched {
    pub tree: PathBuf,
    pub outcomes: Vec<(String, bool)>,
}

/// The key entry the choice of patches adds to a build's key, or `None` for
/// the default: every patch applied, and none that changes outputs.
pub(crate) fn key_entry(mode: Mode, outcomes: &[(String, bool)]) -> Option<String> {
    if mode == Mode::Off {
        return Some("off".into());
    }
    let mut lines = Vec::new();
    for patch in &PATCHES {
        let applied = outcomes
            .iter()
            .find(|(name, _)| name == patch.name)
            .is_some_and(|(_, applied)| *applied);
        if !applied {
            lines.push(format!("skipped {}", patch.name));
        } else if patch.changes_outputs {
            lines.push(format!(
                "applied {} {}",
                patch.name,
                Digest::of_bytes(patch.text.as_bytes())
            ));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Reads a step's report: each line `applied <name>` or `skipped <name>`.
pub(crate) fn parse_report(text: &str) -> Result<Vec<(String, bool)>, String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once(' ') {
            Some(("applied", name)) => Ok((name.to_string(), true)),
            Some(("skipped", name)) => Ok((name.to_string(), false)),
            _ => Err(format!("the patches report has a line {line:?}")),
        })
        .collect()
}

/// The report text for `outcomes`.
fn report_text(outcomes: &[(String, bool)]) -> String {
    outcomes
        .iter()
        .map(|(name, applied)| {
            let word = if *applied { "applied" } else { "skipped" };
            format!("{word} {name}\n")
        })
        .collect()
}

/// Applies [`PATCHES`] to a copy of `source` (the store path of the
/// Buildroot tree, with digest `source_digest`) and returns the copy. The
/// copy is a step's output, keyed on the source's digest and the patches'
/// texts, so a copy made once is looked up after.
pub(crate) fn patched_source(
    store: &Store,
    cx: &Ctx,
    source: &Path,
    source_digest: Digest,
) -> Result<Patched, StepError> {
    let mut key = KeyBuilder::new(NAME, RECIPE_VERSION).input("source", source_digest);
    for patch in &PATCHES {
        key = key.input(
            &format!("patch {}", patch.name),
            Digest::of_bytes(patch.text.as_bytes()),
        );
    }
    let made = Step::new(store, cx, NAME, key.finish())
        .ram_work(WORK_BYTES)
        .look_up_or_make(
            &[("tree", Path::new("tree")), ("report", Path::new(REPORT))],
            |work| make_patched(work, source),
        )?;
    let report = fs::read_to_string(store.path(&made.outputs["report"]))
        .map_err(|e| StepError::body(NAME, format!("the patches report: {e}")))?;
    let outcomes = parse_report(&report).map_err(|e| StepError::body(NAME, e))?;
    Ok(Patched {
        tree: store.path(&made.outputs["tree"]),
        outcomes,
    })
}

/// The body of [`patched_source`]: a copy of `source` in the step's
/// directory, the patches applied to it in turn, and the report.
fn make_patched(work: &mut Work<'_>, source: &Path) -> Result<(), StepError> {
    let dir = work.dir().to_path_buf();
    let tree = dir.join("tree");
    for (program, args) in copy_commands(source, &tree) {
        work.run(&Tool::new(program).args(args).env("PATH", crate::PATH))?;
    }
    let outcomes =
        apply_all(&tree, &dir.join("patches")).map_err(|why| StepError::body(NAME, why))?;
    fs::write(dir.join(REPORT), report_text(&outcomes))
        .map_err(|e| StepError::body(NAME, format!("the patches report: {e}")))?;
    Ok(())
}

/// The commands that copy the source at `source` to `tree` for patching:
/// a copy that keeps each file's mode, since Buildroot runs its own scripts
/// (`support/scripts/*`) straight from its tree, and then the owner's write
/// permission on all of it, since the store's files are read-only and a
/// patch edits its copy.
fn copy_commands(source: &Path, tree: &Path) -> [(&'static str, Vec<String>); 2] {
    let (source, tree) = (source.display().to_string(), tree.display().to_string());
    [
        (
            "cp",
            vec![
                "-R".into(),
                "--reflink=auto".into(),
                "--preserve=mode".into(),
                source,
                tree.clone(),
            ],
        ),
        ("chmod", vec!["-R".into(), "u+w".into(), tree]),
    ]
}

/// Applies each of [`PATCHES`] to the tree at `tree` in turn, writing the
/// patch texts into `scratch` first. Returns each patch's outcome.
fn apply_all(tree: &Path, scratch: &Path) -> Result<Vec<(String, bool)>, String> {
    fs::create_dir_all(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let mut outcomes = Vec::with_capacity(PATCHES.len());
    for patch in &PATCHES {
        let file = scratch.join(patch.name);
        fs::write(&file, patch.text).map_err(|e| format!("{}: {e}", patch.name))?;
        let applied = apply_one(tree, &file).map_err(|why| format!("{}: {why}", patch.name))?;
        outcomes.push((patch.name.to_string(), applied));
    }
    Ok(outcomes)
}

/// Applies the patch in `file` to the tree at `tree`, strictly, if it
/// applies to the exact text: a dry run first, so a patch that does not
/// apply leaves the tree as it was. Returns whether it applied.
fn apply_one(tree: &Path, file: &Path) -> Result<bool, String> {
    let dry = patch_command(tree, file).arg("--dry-run").output();
    let dry = dry.map_err(|e| format!("patch: {e}"))?;
    if !dry.status.success() {
        return Ok(false);
    }
    let applied = patch_command(tree, file)
        .output()
        .map_err(|e| format!("patch: {e}"))?;
    if !applied.status.success() {
        return Err(format!(
            "patch failed after its dry run: {}",
            String::from_utf8_lossy(&applied.stdout).trim()
        ));
    }
    Ok(true)
}

/// `patch` for `file` in `tree`, with the options Gaia applies patches with.
fn patch_command(tree: &Path, file: &Path) -> Command {
    let mut command = Command::new("patch");
    command
        .args(["-p1", "--fuzz=0", "--batch", "--forward", "-s"])
        .args(["--no-backup-if-mismatch", "-i"])
        .arg(file)
        .current_dir(tree)
        .env_clear()
        .env("PATH", crate::PATH)
        .env("LC_ALL", "C");
    command
}

/// Warns, one line per skipped patch, on standard error.
pub(crate) fn warn_skipped(outcomes: &[(String, bool)]) {
    for (name, applied) in outcomes {
        if !applied {
            eprintln!(
                "warning: Buildroot patch {name} does not apply to this Buildroot source, and is left out of the build"
            );
        }
    }
}

/// Makes `prepared`'s source the patched copy (or leaves the upstream one
/// for [`Mode::Off`]), and sets the key entry the choice makes. Every
/// Buildroot step of a build calls this before it keys or runs anything.
pub(crate) fn apply_to(
    store: &Store,
    cx: &Ctx,
    prepared: &mut crate::Prepared,
    mode: Mode,
) -> Result<(), StepError> {
    if mode == Mode::Off {
        prepared.patch_key = key_entry(mode, &[]);
        return Ok(());
    }
    let patched = patched_source(store, cx, &prepared.src, prepared.src_digest)?;
    warn_skipped(&patched.outcomes);
    prepared.patch_key = key_entry(mode, &patched.outcomes);
    prepared.src = patched.tree;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The touched files of a Buildroot tree, from the fixtures of `version`.
    fn fixture(version: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("buildroot-{version}"))
    }

    /// A copy of the fixture's touched files in a new directory.
    fn copy_of(version: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = fixture(version).join("package");
        let to = dir.path().join("package");
        fs::create_dir(&to).expect("package dir");
        for file in ["pkg-utils.mk", "Makefile.in", "pkg-luarocks.mk"] {
            fs::copy(from.join(file), to.join(file)).expect("copy fixture");
        }
        fs::create_dir(to.join("dtc")).expect("dtc dir");
        fs::copy(from.join("dtc/dtc.mk"), to.join("dtc/dtc.mk")).expect("copy fixture");
        dir
    }

    fn read(dir: &Path, file: &str) -> String {
        fs::read_to_string(dir.join("package").join(file)).expect("read")
    }

    /// The shipped dtc patch's text, applied to a copy of the fixture's
    /// `package/dtc/dtc.mk` of `version`: the file the make tests run.
    fn patched_dtc_mk(version: &str) -> tempfile::TempDir {
        let tree = copy_of(version);
        let scratch = tempfile::tempdir().expect("tempdir");
        let file = scratch.path().join(PATCHES[2].name);
        fs::write(&file, PATCHES[2].text).expect("write");
        let applied = apply_one(tree.path(), &file).expect("apply");
        assert!(applied, "0003 applies to {version}");
        tree
    }

    fn all_applied(outcomes: &[(String, bool)]) -> bool {
        outcomes.len() == PATCHES.len() && outcomes.iter().all(|(_, applied)| *applied)
    }

    #[test]
    fn both_patches_apply_to_buildroot_2026_02_3() {
        let tree = copy_of("2026.02.3");
        let scratch = tempfile::tempdir().expect("tempdir");
        let outcomes =
            apply_all(tree.path(), scratch.path().join("patches").as_path()).expect("apply");
        assert!(all_applied(&outcomes), "{outcomes:?}");
        assert!(read(tree.path(), "pkg-utils.mk").contains("cp -a --reflink=auto"));
        assert!(read(tree.path(), "Makefile.in").contains("export PERL:=$(shell which perl)"));
        assert!(read(tree.path(), "pkg-luarocks.mk").contains("firstword"));
        assert!(read(tree.path(), "dtc/dtc.mk").contains("HOST_DTC_BISON_OK :="));
    }

    #[test]
    fn both_patches_apply_to_buildroot_2026_08() {
        let tree = copy_of("2026.08");
        let scratch = tempfile::tempdir().expect("tempdir");
        let outcomes = apply_all(tree.path(), scratch.path()).expect("apply");
        assert!(all_applied(&outcomes), "{outcomes:?}");
        assert!(read(tree.path(), "pkg-utils.mk").contains("cp -a --reflink=auto"));
        assert!(read(tree.path(), "Makefile.in").contains("export PERL:=$(shell which perl)"));
        assert!(read(tree.path(), "pkg-luarocks.mk").contains("firstword"));
        assert!(read(tree.path(), "dtc/dtc.mk").contains("HOST_DTC_BISON_OK :="));
    }

    #[test]
    fn a_patch_that_does_not_apply_is_skipped_and_leaves_its_files_as_they_were() {
        let tree = copy_of("2026.08");
        // Makefile.in differs from upstream: 0002 names it, so 0002 must not
        // apply at all, not even to pkg-luarocks.mk, which it also changes.
        let makefile = tree.path().join("package/Makefile.in");
        let text = fs::read_to_string(&makefile).expect("read");
        let changed = text.replace(
            "export PERL=$(shell which perl)",
            "export PERL=/usr/bin/perl",
        );
        assert_ne!(text, changed, "the fixture names the line");
        fs::write(&makefile, changed).expect("write");
        let before = read(tree.path(), "pkg-luarocks.mk");
        let scratch = tempfile::tempdir().expect("tempdir");
        let outcomes = apply_all(tree.path(), scratch.path()).expect("apply");
        assert_eq!(
            outcomes,
            vec![
                (PATCHES[0].name.to_string(), true),
                (PATCHES[1].name.to_string(), false),
                (PATCHES[2].name.to_string(), true),
            ]
        );
        assert_eq!(read(tree.path(), "pkg-luarocks.mk"), before);
        assert!(read(tree.path(), "pkg-utils.mk").contains("cp -a --reflink=auto"));
    }

    /// A Buildroot whose dtc.mk differs from the one 0003 was made against
    /// skips 0003 with its warning, leaves dtc.mk as it was, and keeps the
    /// two other patches applied.
    #[test]
    fn the_dtc_patch_is_skipped_on_a_dtc_mk_it_does_not_match() {
        let tree = copy_of("2026.08");
        let file = tree.path().join("package/dtc/dtc.mk");
        let text = fs::read_to_string(&file).expect("read");
        let changed = text.replace(
            "HOST_DTC_DEPENDENCIES = host-bison",
            "HOST_DTC_DEPENDENCIES = host-bison-x",
        );
        assert_ne!(text, changed, "the fixture names the line");
        fs::write(&file, &changed).expect("write");
        let scratch = tempfile::tempdir().expect("tempdir");
        let outcomes = apply_all(tree.path(), scratch.path()).expect("apply");
        assert_eq!(outcomes[2], (PATCHES[2].name.to_string(), false));
        assert!(outcomes[0].1 && outcomes[1].1, "{outcomes:?}");
        assert_eq!(fs::read_to_string(&file).expect("read"), changed);
        let entry = key_entry(Mode::On, &outcomes).expect("an entry");
        assert!(
            entry.contains(&format!("skipped {}", PATCHES[2].name)),
            "{entry}"
        );
        assert!(!entry.contains("applied "), "{entry}");
    }

    #[test]
    fn the_shipped_patches_are_git_format_patches() {
        // The texts are `git format-patch` output: a mail header and the
        // diff, one `diff` per touched file.
        for patch in &PATCHES {
            assert!(patch.text.starts_with("From: "), "{}", patch.name);
            assert!(patch.text.contains("\n---\n"), "{}", patch.name);
        }
    }

    /// Only the patch that changes outputs is in the key when all apply:
    /// its digest, so a build with it never shares a store entry with one
    /// without it.
    #[test]
    fn an_applied_output_changing_patch_is_in_the_key_by_its_digest() {
        let outcomes: Vec<(String, bool)> =
            PATCHES.iter().map(|p| (p.name.to_string(), true)).collect();
        let entry = key_entry(Mode::On, &outcomes).expect("an entry");
        assert_eq!(
            entry,
            format!(
                "applied {} {}",
                PATCHES[2].name,
                Digest::of_bytes(PATCHES[2].text.as_bytes())
            )
        );
        assert!(!entry.contains(PATCHES[0].name), "{entry}");
        assert!(!entry.contains(PATCHES[1].name), "{entry}");
    }

    /// The digest is of the patch text: a changed patch is a changed key.
    #[test]
    fn the_key_entry_changes_with_the_patch_text() {
        let outcomes: Vec<(String, bool)> =
            PATCHES.iter().map(|p| (p.name.to_string(), true)).collect();
        let entry = key_entry(Mode::On, &outcomes).expect("an entry");
        let other = format!("applied {} {}", PATCHES[2].name, Digest::of_bytes(b"other"));
        assert_ne!(entry, other);
    }

    /// A patch that changes no output adds nothing when it applies, so the
    /// two speed patches keep the keys they had.
    #[test]
    fn a_patch_that_changes_no_output_adds_nothing_when_it_applies() {
        for (index, patch) in PATCHES.iter().enumerate() {
            if patch.changes_outputs {
                continue;
            }
            let outcomes: Vec<(String, bool)> =
                PATCHES.iter().map(|p| (p.name.to_string(), true)).collect();
            let entry = key_entry(Mode::On, &outcomes).unwrap_or_default();
            assert!(!entry.contains(patch.name), "patch {index}: {entry}");
        }
    }

    #[test]
    fn a_skipped_patch_is_in_the_key() {
        let mut outcomes: Vec<(String, bool)> =
            PATCHES.iter().map(|p| (p.name.to_string(), true)).collect();
        outcomes[1].1 = false;
        let entry = key_entry(Mode::On, &outcomes).expect("an entry");
        // 0003 applies and changes outputs, so its digest is there as well.
        let lines: Vec<&str> = entry.lines().collect();
        assert_eq!(lines.len(), 2, "{entry}");
        assert_eq!(lines[0], format!("skipped {}", PATCHES[1].name));
        assert!(lines[1].starts_with(&format!("applied {}", PATCHES[2].name)));
    }

    #[test]
    fn the_off_switch_is_in_the_key() {
        let outcomes: Vec<(String, bool)> =
            PATCHES.iter().map(|p| (p.name.to_string(), true)).collect();
        assert_eq!(key_entry(Mode::Off, &outcomes).as_deref(), Some("off"));
        assert_ne!(key_entry(Mode::Off, &[]), key_entry(Mode::On, &outcomes));
    }

    /// Makes `HOST_DTC_DEPENDENCIES` of the patched dtc.mk of `version`,
    /// with `tools` on the PATH: each `(program, first line of --version)`.
    /// The PATH has no other bison or flex, and only the text tools the
    /// test needs beside them.
    fn host_dtc_dependencies(version: &str, tools: &[(&str, &str)]) -> String {
        let tree = patched_dtc_mk(version);
        let bin = tempfile::tempdir().expect("tempdir");
        for (program, line) in tools {
            let file = bin.path().join(program);
            fs::write(&file, format!("#!/bin/sh\necho '{line}'\n")).expect("write");
            let executable =
                <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755);
            fs::set_permissions(&file, executable).expect("mode");
        }
        let base = tempfile::tempdir().expect("tempdir");
        for program in ["sed", "sort", "head", "printf"] {
            let found = ["/usr/bin", "/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(program))
                .find(|path| path.is_file())
                .expect("a text tool");
            std::os::unix::fs::symlink(found, base.path().join(program)).expect("link");
        }
        let probe = tree.path().join("probe.mk");
        fs::write(
            &probe,
            "BR2_PACKAGE_HOST_DTC := y\ninclude package/dtc/dtc.mk\n$(info $(HOST_DTC_DEPENDENCIES))\nall: ;\n",
        )
        .expect("probe");
        let path = format!("{}:{}", bin.path().display(), base.path().display());
        // Found before the PATH under test is set: the child's PATH is not
        // searched for its own program.
        let make = ["/usr/bin/make", "/bin/make"]
            .into_iter()
            .find(|path| Path::new(path).is_file())
            .expect("make");
        let output = Command::new(make)
            .args(["-s", "-f"])
            .arg(&probe)
            .current_dir(tree.path())
            .env("PATH", path)
            .output()
            .expect("make runs");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Make leaves a space where an empty `$(if)` is: the words are what
        // matter.
        String::from_utf8(output.stdout)
            .expect("utf-8")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The machine's bison and flex are used when they are at least the
    /// versions Buildroot builds, and compared as versions (3.10 is newer
    /// than 3.8.2). An older one, or none, gets its host package.
    #[test]
    fn the_host_dtc_takes_only_bison_and_flex_at_least_buildroots_versions() {
        let new_bison = "bison (GNU Bison) 3.8.2";
        let new_flex = "flex 2.6.4";
        assert_eq!(
            host_dtc_dependencies("2026.02.3", &[("bison", new_bison), ("flex", new_flex)]),
            "host-pkgconf"
        );
        assert_eq!(
            host_dtc_dependencies(
                "2026.08",
                &[("bison", "bison (GNU Bison) 3.10"), ("flex", new_flex)]
            ),
            "host-pkgconf"
        );
        assert_eq!(
            host_dtc_dependencies(
                "2026.02.3",
                &[("bison", "bison (GNU Bison) 2.3"), ("flex", new_flex)]
            ),
            "host-bison host-pkgconf"
        );
        assert_eq!(
            host_dtc_dependencies(
                "2026.02.3",
                &[("bison", new_bison), ("flex", "flex 2.5.35")]
            ),
            "host-flex host-pkgconf"
        );
        assert_eq!(
            host_dtc_dependencies("2026.08", &[]),
            "host-bison host-flex host-pkgconf"
        );
    }

    #[test]
    fn the_mode_is_on_unless_off_is_asked_for() {
        assert_eq!(mode(None), Ok(Mode::On));
        assert_eq!(mode(Some("")), Ok(Mode::On));
        assert_eq!(mode(Some("on")), Ok(Mode::On));
        assert_eq!(mode(Some("off")), Ok(Mode::Off));
        let why = mode(Some("0")).expect_err("not a mode");
        assert!(why.contains(ENV), "{why}");
    }

    #[test]
    fn the_report_reads_back_what_it_wrote() {
        let outcomes = vec![
            ("a.patch".to_string(), true),
            ("b.patch".to_string(), false),
        ];
        let text = report_text(&outcomes);
        assert_eq!(parse_report(&text).expect("parse"), outcomes);
        assert!(parse_report("maybe a.patch\n").is_err());
    }

    /// The copy a patch edits keeps what the source could run and can be
    /// written: a read-only script of the source is still a script.
    #[test]
    fn the_copy_keeps_the_exec_bit_and_can_be_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("support/scripts")).expect("dirs");
        let script = source.join("support/scripts/mkmakefile");
        fs::write(&script, "#!/bin/sh\n").expect("script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o555)).expect("mode");
        let plain = source.join("Makefile");
        fs::write(&plain, "all:\n").expect("file");
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o444)).expect("mode");
        let tree = dir.path().join("tree");
        for (program, args) in copy_commands(&source, &tree) {
            let status = std::process::Command::new(program)
                .args(&args)
                .status()
                .expect("run");
            assert!(status.success(), "{program} {args:?}");
        }
        let mode = |path: &Path| fs::metadata(path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&tree.join("support/scripts/mkmakefile")), 0o755);
        assert_eq!(mode(&tree.join("Makefile")), 0o644);
    }
}
