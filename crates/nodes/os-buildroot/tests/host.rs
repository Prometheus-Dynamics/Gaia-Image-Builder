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
    BuildrootHost, ConfigFragment, OsConfig, build_host, build_os, build_plan, host_goals, no_seed,
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
\t@printf 'HOST_CFLAGS=%s\\n' \"$$(grep '^BR2_A_DATA=' $(O)/.config || true)\"

host-a:
\tmkdir -p $(O)/host/etc $(O)/host/bin $(O)/build/host-a-1/src
\tprintf 'prefix=%s\\n' '$(O)/host' > $(O)/host/etc/a.conf
\tprintf 'bin\\000%s' '$(O)' > $(O)/host/bin/a
\tln -s $(O)/host/etc/a.conf $(O)/host/link
\ttouch $(O)/build/host-a-1/.stamp_host_installed $(O)/build/host-a-1/src/big.o
\tmkdir -p $(O)/host/lib/py
\tprintf 'int a(void) { return 1; }\\n' > $(O)/build/host-a-1/src/a.c
\tcc -c -o $(O)/build/host-a-1/src/a.o $(O)/build/host-a-1/src/a.c
\tar rcs $(O)/host/lib/liba.a $(O)/build/host-a-1/src/a.o
\t/usr/bin/ar rcsU $(O)/host/lib/libplain.a $(O)/build/host-a-1/src/a.o
\tprintf 'X = 1\\n' > $(O)/host/lib/py/m.py
\tpython3 -c 'import sys; sys.path.insert(0, \"$(O)/host/lib/py\"); import m'
\tln -s \"$$(command -v python3)\" $(O)/host/bin/python3
\tmkdir -p $(O)/host/lib/python3.x/site-packages
\tprintf 'def f(v):\\n    return v in {\"a\", \"b\", \"c\", \"d\", \"e\", \"f\"}\\n' > $(O)/host/lib/python3.x/site-packages/s.py
\tprintf '%s/package/a/0001-fix.patch\\n' '$(CURDIR)' > $(O)/build/host-a-1/.applied_patches_list
\tprintf 'host-a,./lib/liba.a\\nhost-a,./etc/a.conf\\n' > $(O)/build/host-a-1/.files-list-host.txt
";

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    root: PathBuf,
    /// The sandbox every make runs in, as a real build's do: it sees its
    /// output directory at one fixed path. Empty: none.
    bwrap: String,
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
            bwrap: String::new(),
        }
    }

    /// A fixture whose makes run in the sandbox, at the fixed path, as a
    /// real build's do; `None` where the machine has no bwrap.
    fn sandboxed() -> Option<Self> {
        let bwrap = "/usr/bin/bwrap";
        std::path::Path::new(bwrap).exists().then(|| Self {
            bwrap: bwrap.into(),
            ..Self::new()
        })
    }

    fn store(&self) -> String {
        self.store.display().to_string()
    }

    fn source(&self) -> Tree {
        self.source_with("")
    }

    /// The Buildroot tree, with one more file `extra`: a stand-in for a
    /// patch to Buildroot.
    fn source_with(&self, extra: &str) -> Tree {
        let dir = self.root.join(format!("buildroot-{}", extra.len()));
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("Makefile"), MAKEFILE).expect("write");
        fs::write(dir.join("gaia-extra"), extra).expect("write");
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("stored").into(),
            entries: Vec::new(),
        }
    }

    /// An input tree holding one file `text`, stored.
    fn tree(&self, text: &str) -> Tree {
        let dir = self.root.join(format!("tree-{text}"));
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("file"), text).expect("write");
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
            download_pool: String::new(),
            ccache_dir: String::new(),
            mirror: String::new(),
            config_only: false,
            dev_reuse: String::new(),
            bwrap: self.bwrap.clone(),
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
        self.seed_with(&config(lines), &[], self.source())
    }

    /// The plan node, then the host stage with its tree. Both are given the
    /// input trees the plan reads or needs, as the graph gives them.
    fn seed_with(&self, config: &OsConfig, inputs: &[Tree], source: Tree) -> Tree {
        let plan = build_plan(
            &self.store(),
            source.clone(),
            config,
            &plan_trees(config, inputs),
            &self.host(),
            &Ctx::new(),
        )
        .expect("the plan");
        build_host(
            &self.store(),
            source,
            config,
            &host_trees(config, inputs),
            &plan,
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
    assert_eq!(
        read("relocs"),
        "build/host-a-1/.applied_patches_list\nhost/etc/a.conf"
    );
    assert_eq!(
        read("links"),
        "host/link\t@GAIA_BUILDROOT_OUT@/host/etc/a.conf"
    );
    // The bytecode names its source where builds see it, as a binary does.
    let binaries = read("binaries");
    let binaries: Vec<&str> = binaries.lines().collect();
    assert_eq!(binaries.len(), 2, "{binaries:?}");
    assert_eq!(binaries[0], "host/bin/a");
    assert!(
        binaries[1].starts_with("host/lib/python3.x/site-packages/__pycache__/s.")
            && binaries[1].ends_with(".pyc"),
        "{binaries:?}"
    );
    assert!(
        fs::symlink_metadata(dir.join("host/link")).is_err(),
        "the link is made when the seed is planted"
    );
    // The package's marks, and nothing it compiled.
    assert!(dir.join("build/host-a-1/.stamp_host_installed").is_file());
    assert!(!dir.join("build/host-a-1/src").exists());
}

/// The same host tools made in two stores, a second apart, are one tree:
/// the source's store path is a marker, the file lists are sorted, the
/// archives carry no date (the one `ar` on the build's `PATH` made, and the
/// one an archiver run by its own path made, rewritten), and the module the
/// build imported left no bytecode with its source's time in it.
#[test]
fn the_same_host_tools_made_in_two_stores_are_the_same_bytes() {
    // In the sandbox, as a real build: the stand-in's binary names the
    // output directory, and binaries keep the path they were built at.
    let (Some(one), Some(two)) = (Fixture::sandboxed(), Fixture::sandboxed()) else {
        eprintln!("skipped: no bwrap");
        return;
    };
    let first = one.seed("BR2_X=y\n");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = two.seed("BR2_X=y\n");
    let dir = one.path(&first);
    let read = |name: &str| fs::read_to_string(dir.join(name)).expect(name);
    assert_eq!(
        read("build/host-a-1/.applied_patches_list"),
        "@GAIA_BUILDROOT_SOURCE@/package/a/0001-fix.patch\n"
    );
    assert_eq!(
        read("build/host-a-1/.files-list-host.txt"),
        "host-a,./etc/a.conf\nhost-a,./lib/liba.a\n"
    );
    for archive in ["host/lib/liba.a", "host/lib/libplain.a"] {
        let bytes = fs::read(dir.join(archive)).expect(archive);
        let date = String::from_utf8_lossy(&bytes[8 + 16..8 + 28]).into_owned();
        assert_eq!(date.trim(), "0", "{archive}: the symbol table's date");
    }
    assert!(
        !dir.join("host/lib/py/__pycache__").exists(),
        "no bytecode written on import"
    );
    // The installed packages compiled once, hash-based (flags 3: checked
    // hash), as the standard library is: the same bytes in any store.
    let cache = dir.join("host/lib/python3.x/site-packages/__pycache__");
    let compiled: Vec<_> = fs::read_dir(&cache)
        .expect("the packages compiled")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(compiled.len(), 1, "{compiled:?}");
    assert_eq!(
        fs::read(&compiled[0]).expect("the bytecode")[4..8],
        [3, 0, 0, 0]
    );
    // The wrapper made `liba.a` as it is kept; only the archive made by an
    // archiver run by its own path was rewritten.
    let store = Store::open(&one.store).expect("store");
    let rewritten: Vec<serde_json::Value> = step_records(&logs_dir(&store))
        .into_iter()
        .filter(|record| record.step == "gaia.os.buildroot.host" && record.made)
        .map(|record| record.report.details["archives-rewritten"].clone())
        .collect();
    assert_eq!(
        rewritten,
        [serde_json::json!({ "seed": 1, "first": ["host/lib/libplain.a"] })]
    );
    assert_eq!(
        first.object,
        second.object,
        "the two seeds: {} and {}",
        dir.display(),
        two.path(&second).display()
    );
}

/// The stored config names an input tree, not the store path it was at:
/// the same config from any store.
#[test]
fn the_stored_config_names_its_input_trees() {
    let f = Fixture::new();
    let data = f.tree("data");
    let built = build_os(
        &f.store(),
        f.source(),
        &config_with("BR2_A_DATA=\"@data@\"\n", &["data"]),
        std::slice::from_ref(&data),
        &no_seed(&f.store()).expect("no seed"),
        &f.host(),
        &Ctx::new(),
    )
    .expect("the build");
    let store = Store::open(&f.store).expect("store");
    let config = fs::read_to_string(store.path(&built.6.object.object_ref().expect("stored")))
        .expect("the config");
    assert!(config.contains("BR2_A_DATA=\"@data@\""), "{config}");
    let at = f.path(&data).display().to_string();
    assert!(!config.contains(&at), "{config}");
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

/// The input trees the plan is given: those `OsConfig::plan_inputs` names,
/// in order, as the graph gives them.
fn plan_trees(config: &OsConfig, inputs: &[Tree]) -> Vec<Tree> {
    config
        .plan_inputs()
        .iter()
        .map(|name| {
            let at = config.inputs.iter().position(|n| n == name).expect("named");
            inputs[at].clone()
        })
        .collect()
}

/// The input trees the host tools are given: those `OsConfig::host_inputs`
/// names, from `inputs` in the config's order.
fn host_trees(config: &OsConfig, inputs: &[Tree]) -> Vec<Tree> {
    config
        .host_inputs()
        .iter()
        .map(|name| {
            let at = config.inputs.iter().position(|n| n == name).expect("named");
            inputs[at].clone()
        })
        .collect()
}

/// A config with one fragment, and the names of its input trees.
fn config_with(lines: &str, inputs: &[&str]) -> OsConfig {
    OsConfig {
        inputs: inputs.iter().map(|name| (*name).to_string()).collect(),
        ..config(lines)
    }
}

/// The plan does not wait for a tree it does not read: a kernel's tree is
/// not given to it, and it is made once. A change to the config or to the
/// Buildroot tree makes it again.
#[test]
fn the_plan_key_is_the_buildroot_tree_and_the_config_only() {
    let f = Fixture::new();
    let plan = |f: &Fixture, config: &OsConfig, source: Tree| {
        build_plan(&f.store(), source, config, &[], &f.host(), &Ctx::new()).expect("the plan");
    };
    let kernel = config_with("BR2_X=y\n", &["kernel"]);
    plan(&f, &kernel, f.source());
    assert_eq!(f.made("gaia.os.buildroot.host-plan"), 1);
    plan(&f, &kernel, f.source());
    assert_eq!(
        f.made("gaia.os.buildroot.host-plan"),
        1,
        "the plan ran again"
    );
    plan(
        &f,
        &config_with("BR2_X=y\nBR2_Y=y\n", &["kernel"]),
        f.source(),
    );
    assert_eq!(
        f.made("gaia.os.buildroot.host-plan"),
        2,
        "a config change did not plan again"
    );
    plan(&f, &kernel, f.source_with("patched"));
    assert_eq!(
        f.made("gaia.os.buildroot.host-plan"),
        3,
        "a Buildroot change did not plan again"
    );
}

/// A tree the plan needs (here, the patch directory the config names) is in
/// the plan's key: the plan runs again when it changes.
#[test]
fn a_tree_the_plan_needs_is_in_its_key() {
    let f = Fixture::new();
    let patched = |f: &Fixture, tree: Tree| {
        let config = OsConfig {
            fragments: vec![ConfigFragment {
                name: "f".into(),
                text: "BR2_GLOBAL_PATCH_DIR=\"@patches@\"\n".into(),
            }],
            inputs: vec!["patches".into()],
            ..OsConfig::default()
        };
        build_plan(
            &f.store(),
            f.source(),
            &config,
            &[tree],
            &f.host(),
            &Ctx::new(),
        )
        .expect("the plan");
    };
    patched(&f, f.tree("one"));
    patched(&f, f.tree("one"));
    assert_eq!(f.made("gaia.os.buildroot.host-plan"), 1);
    patched(&f, f.tree("two"));
    assert_eq!(f.made("gaia.os.buildroot.host-plan"), 2);
}

/// The host stage keeps its key on an input tree that the shared variables
/// of the seed name: a change to that tree builds the host tools again. (A
/// package that names a tree is a board package, built by each build, and
/// is not in the seed: its variables are not in the key.)
#[test]
fn the_host_stage_builds_again_when_a_tree_it_builds_from_changes() {
    let f = Fixture::new();
    let with = |f: &Fixture, data: Tree| {
        let config = config_with("BR2_A_DATA=\"@data@\"\n", &["data"]);
        f.seed_with(&config, &[data], f.source())
    };
    let first = with(&f, f.tree("one"));
    assert_eq!(f.made("gaia.os.buildroot.host"), 1);
    let again = with(&f, f.tree("one"));
    assert_eq!(
        f.made("gaia.os.buildroot.host"),
        1,
        "the same tree built again"
    );
    assert_eq!(again, first);
    with(&f, f.tree("two"));
    assert_eq!(
        f.made("gaia.os.buildroot.host"),
        2,
        "a changed tree did not rebuild"
    );
}

/// The opposite case: the kernel's tree is not one the host tools are built
/// from. They are not given it, and a change to it does not build them
/// again; a change to the tree they are built from does.
#[test]
fn a_change_to_the_kernels_tree_does_not_build_the_host_tools_again() {
    let f = Fixture::new();
    let config = config_with(
        "BR2_A_DATA=\"@data@\"\nBR2_LINUX_KERNEL_CUSTOM_DTS_PATH=\"@kernel@/a.dts\"\n",
        &["data", "kernel"],
    );
    assert_eq!(config.host_inputs(), ["data"]);
    let first = f.seed_with(&config, &[f.tree("one"), f.tree("k1")], f.source());
    let again = f.seed_with(&config, &[f.tree("one"), f.tree("k2")], f.source());
    assert_eq!(
        f.made("gaia.os.buildroot.host"),
        1,
        "a kernel change rebuilt"
    );
    assert_eq!(again, first);
    f.seed_with(&config, &[f.tree("two"), f.tree("k2")], f.source());
    assert_eq!(f.made("gaia.os.buildroot.host"), 2);
}

/// Host tools built from a tree they are not given fail, naming it, rather
/// than build from its placeholder path.
#[test]
fn host_tools_not_given_a_tree_they_are_built_from_fail_naming_it() {
    let f = Fixture::new();
    let config = config_with("BR2_A_DATA=\"@data@\"\n", &["data"]);
    let plan =
        build_plan(&f.store(), f.source(), &config, &[], &f.host(), &Ctx::new()).expect("the plan");
    // The same tree named by a symbol the host tools do not take from, so
    // they are given nothing: as a wiring that left the tree out would be,
    // with the plan's values naming it.
    let without = config_with("BR2_ROOTFS_A_DATA=\"@data@\"\n", &["data"]);
    assert!(without.host_inputs().is_empty());
    let err = build_host(
        &f.store(),
        f.source(),
        &without,
        &[],
        &plan,
        &f.host(),
        &Ctx::new(),
    )
    .expect_err("built from a placeholder");
    assert!(err.to_string().contains("the input \"data\""), "{err}");
}
