//! `gaia.build.make`: a Makefile run in a stand-in image (this machine's
//! `make` and `sh` with their libraries), sandboxed. Skipped where no
//! sandbox can run.

use daedalus_cpu_author::FanIn;
use gaia_build_make::build_make;
use gaia_step::{Ctx, Store};
use gaia_types::Tree;
use std::path::{Path, PathBuf};

const BWRAP: &str = "/usr/bin/bwrap";

/// Whether a sandbox can run here.
fn available() -> bool {
    let ok = Path::new(BWRAP).exists()
        && std::process::Command::new(BWRAP)
            .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_SANDBOX").is_none(),
            "GAIA_REQUIRE_SANDBOX is set, and no sandbox runs here"
        );
        println!("SKIPPED: no bubblewrap with user namespaces here");
    }
    ok
}

/// A root filesystem in `dir` with `programs` (each at the path given)
/// and every library they load, each at the path it has here.
fn make_root(dir: &Path, programs: &[(&str, &str)]) {
    let place = |from: &Path, at: &Path| {
        let to = dir.join(at.strip_prefix("/").expect("absolute"));
        if !to.exists() {
            std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
            gaia_store::copy_file(from, &to).expect("copy");
        }
    };
    for (name, at) in programs {
        let program = std::fs::canonicalize(format!("/usr/bin/{name}")).expect("a program");
        place(&program, Path::new(at));
        let listed = std::process::Command::new("ldd")
            .arg(&program)
            .output()
            .expect("ldd");
        for line in String::from_utf8_lossy(&listed.stdout).lines() {
            for word in line.split_whitespace().filter(|w| w.starts_with('/')) {
                let library = Path::new(word);
                place(&std::fs::canonicalize(library).expect("a library"), library);
            }
        }
    }
}

const PROGRAMS: [(&str, &str); 4] = [
    ("make", "/usr/bin/make"),
    ("sh", "/bin/sh"),
    ("cat", "/bin/cat"),
    ("mkdir", "/bin/mkdir"),
];

/// Joins a file of the source `lib` and one of the build source, and
/// writes what the build was told.
const MAKEFILE: &str = "all:\n\tmkdir -p $(OUT)/bin\n\tcat $(DEPS)/lib/a.txt $(SRC)/b.txt > $(OUT)/bin/joined\n\techo \"$(SOURCE_DATE_EPOCH) $(LC_ALL) $$PWD\" > $(OUT)/env\n\nother:\n\tmkdir -p $(OUT)\n\techo other > $(OUT)/other\n\nnothing:\n\t:\n";

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

    fn tree(&self, name: &str, files: &[(&str, &str)]) -> Tree {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, text).expect("write");
        }
        self.put(&dir)
    }

    fn put(&self, dir: &Path) -> Tree {
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    fn image(&self, programs: &[(&str, &str)], name: &str) -> Tree {
        let dir = self.root.join(name);
        if !dir.exists() {
            make_root(&dir, programs);
        }
        self.put(&dir)
    }

    fn source(&self) -> Tree {
        self.tree("src", &[("Makefile", MAKEFILE), ("b.txt", "from src\n")])
    }

    fn build(
        &self,
        source: Tree,
        deps: &[Tree],
        names: &str,
        targets: &str,
    ) -> Result<(PathBuf, usize), String> {
        let deps: Vec<Option<Tree>> = deps.iter().cloned().map(Some).collect();
        let (tree, artifacts) = build_make(
            &self.store,
            BWRAP,
            self.image(&PROGRAMS, "image"),
            source,
            FanIn::values(&deps),
            Some(&names.to_string()),
            Some(&targets.to_string()),
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        let object = tree.object.object_ref().expect("stored");
        Ok((store.path(&object), artifacts.len()))
    }
}

#[test]
fn a_makefile_builds_from_its_sources_by_name_into_out() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let lib = f.tree("lib", &[("a.txt", "from lib\n")]);
    let (out, artifacts) = f.build(f.source(), &[lib], "lib", "").expect("built");
    let read = |p: &str| std::fs::read_to_string(out.join(p)).expect(p);
    assert_eq!(read("bin/joined"), "from lib\nfrom src\n");
    assert_eq!(read("env"), "0 C /work/build\n");
    assert_eq!(artifacts, 2, "each file of $(OUT) is an artifact");
    // The build directory and home are not kept.
    let mut names: Vec<_> = std::fs::read_dir(&out)
        .expect("out")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    names.sort();
    assert_eq!(names, ["bin", "env"]);

    // A target given is the one made.
    let lib = f.tree("lib", &[("a.txt", "from lib\n")]);
    let (out, _) = f.build(f.source(), &[lib], "lib", "other").expect("built");
    assert_eq!(
        std::fs::read_to_string(out.join("other")).expect("other"),
        "other\n"
    );

    // Another source's bytes are another build.
    let lib2 = f.tree("lib2", &[("a.txt", "changed\n")]);
    let (out, _) = f.build(f.source(), &[lib2], "lib", "").expect("built");
    assert_eq!(
        std::fs::read_to_string(out.join("bin/joined")).expect("joined"),
        "changed\nfrom src\n"
    );
}

#[test]
fn a_make_build_is_refused_what_it_cannot_run() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let lib = || f.tree("lib", &[("a.txt", "x\n")]);
    let refused = |result: Result<(PathBuf, usize), String>, why: &str| {
        let error = result.expect_err(why);
        assert!(error.contains(why), "{error}");
    };
    refused(
        f.build(f.source(), &[lib()], "", ""),
        "1 sources are given and 0 names for them",
    );
    refused(
        f.build(f.source(), &[lib()], "../lib", ""),
        "`../lib` is not a source name",
    );
    refused(
        f.build(f.source(), &[lib()], "lib", "-j9"),
        "target `-j9` is a flag",
    );
    refused(
        f.build(f.tree("bare", &[("b.txt", "x")]), &[lib()], "lib", ""),
        "the build source has no Makefile",
    );
    refused(
        f.build(f.source(), &[lib()], "lib", "nothing"),
        "the Makefile put nothing under $(OUT)",
    );
    // An image with no make of its own.
    let deps = [Some(lib())];
    let error = build_make(
        &f.store,
        BWRAP,
        f.image(&PROGRAMS[1..], "no-make"),
        f.source(),
        FanIn::values(&deps),
        Some(&"lib".to_string()),
        None,
        &Ctx::new(),
    )
    .map(|_| ())
    .expect_err("no make");
    assert!(
        error.to_string().contains("the image has no /usr/bin/make"),
        "{error}"
    );
}
