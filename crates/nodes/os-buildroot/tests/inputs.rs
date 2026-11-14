//! The OS config's input trees, with a stand-in Buildroot: a Makefile that
//! writes `allnoconfig`, keeps the merged config, and on `all` records the
//! config it was given and the override paths it was run with. Runs
//! everywhere: no network, no real Buildroot.
//!
//! - An input named `@name@` in a fragment reaches the config as its path
//!   in the store, and an override reaches make as `<PACKAGE>_OVERRIDE_SRCDIR`.
//! - The key is the text as written: the same config and trees are found in
//!   the store; a changed fragment, or a changed input tree, is a new step.
//! - A config naming more trees than are given, or an unknown `@name@`, is
//!   an error.

use gaia_os_buildroot::{BuildrootHost, ConfigFragment, OsConfig, build_os};
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_types::{SourceOverride, Tree};
use std::fs;
use std::path::{Path, PathBuf};

const MAKEFILE: &str = "\
all:
\tmkdir -p $(O)/target $(O)/images
\tcp $(O)/.config $(O)/images/seen.config
\tprintf '%s\\n' '$(LINUX_OVERRIDE_SRCDIR)' > $(O)/images/linux-override.txt

allnoconfig:
\tmkdir -p $(O)
\tprintf 'BR2_HAVE_DOT_CONFIG=y\\n' > $(O)/.config

olddefconfig show-build-order:
\t@true

linux-configure:
\tmkdir -p $(O)/build/linux-custom
\tprintf 'CONFIG_X=y\\nCONFIG_Y=y\\n# CONFIG_Z is not set\\n' > $(O)/build/linux-custom/.config
\ttouch $(O)/build/linux-custom/Makefile
";

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let store = root.join("store");
    Store::open(&store).expect("store");
    Fixture {
        _dir: dir,
        store,
        root,
    }
}

impl Fixture {
    fn tree(&self, name: &str, files: &[(&str, &str)]) -> Tree {
        let dir = self.root.join("trees").join(name);
        fs::create_dir_all(&dir).expect("tree dir");
        for (file, text) in files {
            fs::write(dir.join(file), text).expect("write");
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("stored").into(),
            entries: Vec::new(),
        }
    }

    fn path(&self, tree: &Tree) -> PathBuf {
        let store = Store::open(&self.store).expect("store");
        store.path(&tree.object.object_ref().expect("stored"))
    }

    /// Runs the step: the image output's path, and whether its body ran.
    fn run(&self, config: &OsConfig, inputs: &[Tree]) -> Result<(PathBuf, bool), String> {
        let source = self.tree("buildroot", &[("Makefile", MAKEFILE)]);
        let host = BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: String::new(),
            cpus: String::new(),
            jobs: 1,
            download_dir: self.root.join("dl").display().to_string(),
        };
        let out = build_os(
            &self.store.display().to_string(),
            source,
            config,
            inputs,
            &gaia_os_buildroot::no_seed(&self.store.display().to_string()).expect("seed"),
            &host,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        let made = step_records(&logs_dir(&store))
            .into_iter()
            .rev()
            .find(|r| r.step == "gaia.os.buildroot")
            .expect("a record")
            .made;
        let images = out.5.object.object_ref().expect("images");
        Ok((store.path(&images), made))
    }
}

fn config(text: &str) -> OsConfig {
    OsConfig {
        fragments: vec![ConfigFragment {
            name: "board.kernel".into(),
            text: text.into(),
        }],
        inputs: vec!["board.linux".into(), "files.0".into()],
        overrides: vec![SourceOverride {
            package: "LINUX".into(),
            input: "board.linux".into(),
        }],
        externals: Vec::new(),
        kernel_contracts: Vec::new(),
    }
}

const TEXT: &str =
    "BR2_LINUX_KERNEL=y\nBR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"@files.0@/linux.fragment\"\n";

fn find(images: &Path, name: &str) -> String {
    for entry in walk(images) {
        if entry.file_name().is_some_and(|n| n == name) {
            return fs::read_to_string(entry).expect("read");
        }
    }
    panic!("no {name} under {}", images.display());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[test]
fn input_names_become_store_paths_and_overrides_reach_make() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    let files = f.tree("files", &[("linux.fragment", "CONFIG_X=y\n")]);
    let (images, made) = f
        .run(&config(TEXT), &[linux.clone(), files.clone()])
        .expect("the step");
    assert!(made, "a first run makes its outputs");
    let seen = find(&images, "seen.config");
    println!("config Buildroot was given:\n{seen}");
    let expected = format!(
        "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"{}/linux.fragment\"",
        f.path(&files).display()
    );
    assert!(seen.contains(&expected), "{seen}");
    assert!(!seen.contains('@'), "no name is left: {seen}");
    let srcdir = find(&images, "linux-override.txt");
    println!("LINUX_OVERRIDE_SRCDIR={}", srcdir.trim());
    assert_eq!(srcdir.trim(), f.path(&linux).display().to_string());
}

#[test]
fn the_key_is_the_text_as_written_and_the_trees() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    let files = f.tree("files", &[("linux.fragment", "CONFIG_X=y\n")]);
    let inputs = [linux.clone(), files.clone()];
    let (_, first) = f.run(&config(TEXT), &inputs).expect("first");
    let (_, again) = f.run(&config(TEXT), &inputs).expect("again");
    println!("first made={first}, same config again made={again}");
    assert!(first && !again, "the same config and trees are found");
    let changed = format!("{TEXT}BR2_LINUX_KERNEL_DTS_SUPPORT=y\n");
    let (_, fragment) = f.run(&config(&changed), &inputs).expect("changed");
    println!("a changed fragment made={fragment}");
    assert!(fragment, "a changed fragment is a new step");
    let files2 = f.tree("files2", &[("linux.fragment", "CONFIG_Y=y\n")]);
    let (_, tree) = f
        .run(&config(TEXT), &[linux, files2])
        .expect("changed tree");
    println!("a changed input tree made={tree}");
    assert!(tree, "a changed input tree is a new step");
}

#[test]
fn a_tree_count_that_differs_from_the_names_is_an_error() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    let why = f
        .run(&config(TEXT), &[linux])
        .expect_err("one tree for two");
    assert!(
        why.contains("names 2 input trees, and 1 are given"),
        "{why}"
    );
}

#[test]
fn an_unknown_name_in_a_fragment_is_an_error() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    let files = f.tree("files", &[("linux.fragment", "")]);
    let why = f
        .run(&config("BR2_X=\"@files.9@/f\"\n"), &[linux, files])
        .expect_err("unknown name");
    assert!(why.contains("@files.9@ names no input tree"), "{why}");
}

#[test]
fn a_kernel_fragment_line_the_kernel_did_not_take_fails_before_the_build() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    // The stand-in kernel's config has X and Y on and Z off. This fragment
    // asks for X (taken), for Z (not taken) and for a symbol it does not have.
    let files = f.tree(
        "files",
        &[(
            "linux.fragment",
            "CONFIG_X=y\nCONFIG_Z=y\nCONFIG_NO_SUCH_DRIVER=y\n",
        )],
    );
    let why = f
        .run(&config(TEXT), &[linux, files])
        .expect_err("two lines did not take");
    println!("{why}");
    assert!(why.contains("linux's config did not take 2 lines"), "{why}");
    assert!(
        why.contains("linux.fragment: `CONFIG_Z=y` became `# CONFIG_Z is not set`"),
        "{why}"
    );
    assert!(
        why.contains("linux.fragment: `CONFIG_NO_SUCH_DRIVER=y` was dropped"),
        "{why}"
    );
}

/// A kernel contract: a file in an input tree lists what must be built in,
/// and the kernel's final config is held to it before the kernel compiles.
#[test]
fn a_kernel_that_does_not_meet_its_contract_fails_before_the_build() {
    use gaia_types::KernelContract;
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    let files = f.tree("files", &[("linux.fragment", "CONFIG_X=y\n")]);
    // The stand-in kernel's config has X and Y on and Z off.
    let contract = |always: &str, storage: &str| {
        let text = format!(
            "version = 1\n[always]\nall = [{always}]\n[kind.disk]\nall = []\none_of = [[\"Z\", \"Y\"]]\n[optional]\nall = []\n[initramfs]\nnone = \"\"\n[boot_check]\nchecked = []\n"
        );
        let tree = f.tree(
            &format!("contract-{}", always.len()),
            &[("slot-kernel.toml", &text)],
        );
        let mut config = config(TEXT);
        config.inputs.push("contract.init".into());
        config.kernel_contracts.push(KernelContract {
            input: "contract.init".into(),
            path: "slot-kernel.toml".into(),
            asked_by: "init".into(),
            board: "demo".into(),
            kind: "disk".into(),
            storage_drivers: vec![storage.into()],
        });
        (config, tree)
    };

    // Met: X always, one of Z or Y, the storage driver Y.
    let (config, tree) = contract("\"X\"", "CONFIG_Y");
    let (_, made) = f
        .run(&config, &[linux.clone(), files.clone(), tree])
        .expect("the contract is met");
    assert!(made);

    // Not met: Z is off, and so is the board's storage driver.
    let (config, tree) = contract("\"X\", \"Z\"", "CONFIG_Z");
    let why = f
        .run(&config, &[linux, files, tree])
        .expect_err("the contract is not met");
    println!("{why}");
    assert!(
        why.contains("the kernel for the board demo does not meet the contract of init"),
        "{why}"
    );
    assert!(
        why.contains("CONFIG_Z is not set, and must be =y (slot-kernel.toml: always.all)"),
        "{why}"
    );
    assert!(
        why.contains("CONFIG_Z is not set, and must be =y (the board demo's storage_drivers)"),
        "{why}"
    );
}

/// A complete kernel configuration the build starts from: every line of it
/// is held to the kernel's final config, as the first of the fragments, so
/// a later fragment's line for a symbol is the one that counts.
#[test]
fn a_complete_kernel_configuration_is_held_to_the_final_one() {
    let f = fixture();
    let linux = f.tree("linux", &[("Makefile", "# a kernel\n")]);
    // The stand-in kernel's config has X and Y on and Z off.
    let text = "BR2_LINUX_KERNEL=y\nBR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y\nBR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE=\"@files.0@/full.config\"\n";
    let with_fragment = "BR2_LINUX_KERNEL=y\nBR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y\nBR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE=\"@files.0@/full.config\"\nBR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"@files.0@/linux.fragment\"\n";

    // The same as the final config: it holds.
    let same = f.tree(
        "same",
        &[(
            "full.config",
            "CONFIG_X=y\nCONFIG_Y=y\n# CONFIG_Z is not set\n",
        )],
    );
    let (_, made) = f
        .run(&config(text), &[linux.clone(), same])
        .expect("the configuration holds");
    assert!(made);

    // A line of it that the kernel did not keep.
    let other = f.tree(
        "other",
        &[(
            "full.config",
            "CONFIG_X=y\nCONFIG_Y=m\n# CONFIG_Z is not set\n",
        )],
    );
    let why = f
        .run(&config(text), &[linux.clone(), other])
        .expect_err("one line did not hold");
    println!("{why}");
    assert!(why.contains("linux's config did not take 1 line"), "{why}");
    assert!(
        why.contains("full.config: `CONFIG_Y=m` became `CONFIG_Y=y`"),
        "{why}"
    );

    // The same file with a fragment after it that sets that symbol: the
    // fragment's line is the one checked.
    let both = f.tree(
        "both",
        &[
            (
                "full.config",
                "CONFIG_X=y\nCONFIG_Y=m\n# CONFIG_Z is not set\n",
            ),
            ("linux.fragment", "CONFIG_Y=y\n"),
        ],
    );
    f.run(&config(with_fragment), &[linux, both])
        .expect("the fragment's line holds");
}
