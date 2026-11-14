//! The host tools' step, with a stand-in Buildroot: a Makefile with one host
//! package that names its output directory in a text file, a binary and a
//! symlink. Runs everywhere: no network, no real Buildroot.
//!
//! - The seed holds the host directory and the package's marks, with the
//!   output directory's path taken out of text files and links.
//! - A build given the seed finds the package installed, with its own path
//!   put back.
//! - A config line that changes nothing about a host package does not build
//!   the host tools again; one that does, does.

use gaia_os_buildroot::{
    BuildrootHost, ConfigFragment, OsConfig, build_host, build_os, host_goals, no_seed,
};
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_types::Tree;
use std::fs;
use std::path::PathBuf;

const MAKEFILE: &str = "\
all:
\tmkdir -p $(O)/target $(O)/images
\ttest -f $(O)/build/host-a-1/.stamp_host_installed || $(MAKE) host-a
\tcp $(O)/host/etc/a.conf $(O)/images/a.conf
\treadlink $(O)/host/link > $(O)/images/link
\tls $(O)/build/host-a-1 > $(O)/images/build-dir

allnoconfig:
\tmkdir -p $(O)
\tprintf 'BR2_HAVE_DOT_CONFIG=y\\n' > $(O)/.config

olddefconfig show-build-order:
\t@true

show-info:
\t@printf '%s\\n' '{\"host-a\":{\"dependencies\":[]},\"host-b\":{\"dependencies\":[\"libc\"]},\"libc\":{\"dependencies\":[]},\"busybox\":{\"dependencies\":[\"host-a\"]}}'

printvars:
\t@printf 'HOST_A_CONF_OPTS=--prefix=%s --fast=%s\\n' '$(O)/host' \"$$(grep -c BR2_A_FAST=y $(O)/.config || true)\"

host-a:
\tmkdir -p $(O)/host/etc $(O)/host/bin $(O)/build/host-a-1/src
\tprintf 'prefix=%s\\n' '$(O)/host' > $(O)/host/etc/a.conf
\tprintf 'bin\\000%s' '$(O)' > $(O)/host/bin/a
\tln -s $(O)/host/etc/a.conf $(O)/host/link
\ttouch $(O)/build/host-a-1/.stamp_host_installed $(O)/build/host-a-1/src/big.o
";

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store");
        Store::open(&store).expect("store");
        Self {
            _dir: dir,
            store,
            root,
        }
    }

    fn store(&self) -> String {
        self.store.display().to_string()
    }

    fn source(&self) -> Tree {
        let dir = self.root.join("buildroot");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("Makefile"), MAKEFILE).expect("write");
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("stored").into(),
            entries: Vec::new(),
        }
    }

    fn host(&self) -> BuildrootHost {
        BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: String::new(),
            cpus: String::new(),
            jobs: 1,
            download_dir: self.root.join("dl").display().to_string(),
            config_only: false,
        }
    }

    fn path(&self, tree: &Tree) -> PathBuf {
        let store = Store::open(&self.store).expect("store");
        store.path(&tree.object.object_ref().expect("stored"))
    }

    /// How many times the step `name` ran its body so far.
    fn made(&self, name: &str) -> usize {
        let store = Store::open(&self.store).expect("store");
        step_records(&logs_dir(&store))
            .into_iter()
            .filter(|record| record.step == name && record.made)
            .count()
    }

    fn seed(&self, lines: &str) -> Tree {
        build_host(
            &self.store(),
            self.source(),
            &config(lines),
            &[],
            &self.host(),
            &Ctx::new(),
        )
        .expect("the host tools")
    }

    /// A build with `seed`: its `images` directory.
    fn images(&self, lines: &str, seed: &Tree) -> PathBuf {
        let built = build_os(
            &self.store(),
            self.source(),
            &config(lines),
            &[],
            seed,
            &self.host(),
            &Ctx::new(),
        )
        .expect("the build");
        let store = Store::open(&self.store).expect("store");
        store.path(&built.5.object.object_ref().expect("stored"))
    }
}

fn config(lines: &str) -> OsConfig {
    OsConfig {
        fragments: vec![ConfigFragment {
            name: "test".into(),
            text: lines.into(),
        }],
        ..OsConfig::default()
    }
}

#[test]
fn the_goals_are_the_host_packages_that_need_only_host_packages() {
    let info = r#"{"host-a":{"dependencies":["host-c"]},"host-b":{"dependencies":["libc"]},
        "host-c":{"dependencies":[]},"host-d":{"dependencies":["host-b"]},
        "libc":{"dependencies":[]},"busybox":{"dependencies":["host-a"]}}"#;
    assert_eq!(host_goals(info).expect("goals"), ["host-a", "host-c"]);
    assert!(host_goals("not json").is_err());
}

#[test]
fn the_seed_has_the_host_tools_without_the_path_they_were_built_at() {
    let f = Fixture::new();
    let seed = f.seed("BR2_X=y\n");
    let dir = f.path(&seed);
    let read = |name: &str| fs::read_to_string(dir.join(name)).expect(name);
    assert_eq!(
        read("host/etc/a.conf"),
        "prefix=@GAIA_BUILDROOT_OUT@/host\n"
    );
    assert_eq!(read("relocs"), "host/etc/a.conf");
    assert_eq!(
        read("links"),
        "host/link\t@GAIA_BUILDROOT_OUT@/host/etc/a.conf"
    );
    assert_eq!(read("binaries"), "host/bin/a");
    assert!(
        fs::symlink_metadata(dir.join("host/link")).is_err(),
        "the link is made when the seed is planted"
    );
    // The package's marks, and nothing it compiled.
    assert!(dir.join("build/host-a-1/.stamp_host_installed").is_file());
    assert!(!dir.join("build/host-a-1/src").exists());
}

#[test]
fn a_build_with_the_seed_finds_the_host_tools_installed_at_its_own_path() {
    let f = Fixture::new();
    let seed = f.seed("BR2_X=y\n");
    let images = f.images("BR2_X=y\n", &seed);
    let read = |name: &str| fs::read_to_string(images.join(name)).expect(name);
    let conf = read("a.conf");
    println!("a.conf: {conf}link: {}", read("link"));
    assert!(
        conf.starts_with("prefix=/") && conf.ends_with("/out/host\n"),
        "{conf}"
    );
    assert!(!conf.contains("GAIA_BUILDROOT_OUT"));
    assert!(read("link").trim().ends_with("/out/host/etc/a.conf"));
    assert_eq!(
        read("build-dir").trim(),
        "",
        "the package was not built here: its build directory has only its mark, a dot file"
    );

    // Without the seed the same build makes the package itself.
    let alone = f.images("BR2_X=y\n", &no_seed(&f.store()).expect("no seed"));
    assert_eq!(
        fs::read_to_string(alone.join("build-dir"))
            .expect("build-dir")
            .trim(),
        "src",
        "built here: its build directory has what it compiled"
    );
}

#[test]
fn only_what_changes_a_host_tool_builds_the_host_tools_again() {
    let f = Fixture::new();
    let made = |f: &Fixture| {
        (
            f.made("gaia.os.buildroot.host-plan"),
            f.made("gaia.os.buildroot.host"),
        )
    };
    let first = f.seed("BR2_X=y\n");
    assert_eq!(made(&f), (1, 1));
    // Another config line: Buildroot is asked again, and answers the same.
    let second = f.seed("BR2_X=y\nBR2_PACKAGE_BUSYBOX_SHOW_OTHERS=y\n");
    assert_eq!(made(&f), (2, 1));
    assert_eq!(second, first, "the same seed");
    // A line that changes how the host package is configured.
    f.seed("BR2_X=y\nBR2_A_FAST=y\n");
    assert_eq!(made(&f), (3, 2));
}
