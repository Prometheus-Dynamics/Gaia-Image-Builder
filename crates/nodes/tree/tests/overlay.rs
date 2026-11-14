//! `gaia.devicetree.overlay`, with this machine's `dtc` and `fdtoverlay` as
//! the host tools. Skipped where they are not installed.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::{devicetree_overlay, overlay_compiler, overlay_version_of};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const BASE: &str = r#"/dts-v1/;
/ {
    #address-cells = <1>;
    #size-cells = <1>;
    compatible = "test,board";
    bus: bus@1000 {
        compatible = "test,bus";
        reg = <0x1000 0x100>;
        status = "disabled";
    };
};
"#;

/// An overlay that turns the bus on and leaves a marker at the root.
const OVERLAY: &str = r#"/dts-v1/;
/plugin/;
/ {
    fragment@0 {
        target = <&bus>;
        __overlay__ { status = "okay"; };
    };
    fragment@1 {
        target-path = "/";
        __overlay__ {
            marker { test,board = "demo"; test,digest = "0123abcd"; };
        };
    };
};
"#;

const CONTRACT: &str = r#"version = 1
minimum_dtc = "1.6.0"
flags = ["-@", "-I", "dts", "-O", "dtb"]
error_classes = ["unit_address_vs_reg", "alias_paths"]
"#;

fn tools_here() -> Option<(PathBuf, PathBuf)> {
    let found = |name: &str| {
        ["/usr/bin", "/usr/local/bin", "/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|path| path.is_file())
    };
    let pair = found("dtc").zip(found("fdtoverlay"));
    if pair.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_DTC").is_none(),
            "GAIA_REQUIRE_DTC is set, and there is no dtc and fdtoverlay here"
        );
        println!("SKIPPED: no dtc and fdtoverlay on this machine");
    }
    pair
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
    dtc: PathBuf,
    fdtoverlay: PathBuf,
}

impl Fixture {
    fn new() -> Option<Self> {
        let (dtc, fdtoverlay) = tools_here()?;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store").display().to_string();
        Some(Self {
            _dir: dir,
            root,
            store,
            dtc,
            fdtoverlay,
        })
    }

    fn tree(&self, name: &str, files: &[(&str, &[u8])]) -> Tree {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (path, bytes) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, bytes).expect("write");
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    /// The host tools: this machine's two programs under `bin/`.
    fn tools(&self) -> Tree {
        let dir = self.root.join("tools/bin");
        if !dir.exists() {
            std::fs::create_dir_all(&dir).expect("mkdir");
            for (from, name) in [(&self.dtc, "dtc"), (&self.fdtoverlay, "fdtoverlay")] {
                std::fs::copy(from, dir.join(name)).expect("copy");
                std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store
                .put_tree(&self.root.join("tools"))
                .expect("put")
                .into(),
            entries: Vec::new(),
        }
    }

    /// The base tree compiled with symbols, as a kernel's build makes it.
    fn base(&self, source: &str) -> Artifact {
        let dir = self.root.join("base");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("board.dts"), source).expect("write");
        let ok = std::process::Command::new(&self.dtc)
            .args(["-@", "-I", "dts", "-O", "dtb", "-o"])
            .arg(dir.join("board.dtb"))
            .arg(dir.join("board.dts"))
            .status()
            .expect("dtc")
            .success();
        assert!(ok, "the base tree compiles");
        std::fs::remove_file(dir.join("board.dts")).expect("rm");
        let store = Store::open(&self.store).expect("store");
        Artifact {
            kind: ArtifactKind::DeviceTree,
            object: store.put_tree(&dir).expect("put").into(),
        }
    }

    fn run(
        &self,
        name: &str,
        contract: &str,
        overlay: &str,
        stated: Option<&str>,
        base: &str,
        extra: &str,
    ) -> Result<PathBuf, String> {
        let mut files: Vec<(&str, &[u8])> = vec![
            ("overlay.dts", overlay.as_bytes()),
            ("other.txt", extra.as_bytes()),
        ];
        if let Some(stated) = stated {
            files.push(("manifest-boot.json", stated.as_bytes()));
        }
        let made = devicetree_overlay(
            &self.store,
            self.tools(),
            self.tree(
                &format!("contract-{name}"),
                &[("contracts/dtc.toml", contract.as_bytes())],
            ),
            "contracts/dtc.toml",
            self.tree(&format!("generated-{name}"), &files),
            "overlay.dts",
            if stated.is_some() {
                "manifest-boot.json"
            } else {
                ""
            },
            &self.base(base),
            "board.dtb",
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
            .filter(|r| r.step == "gaia.devicetree.overlay" && r.made)
            .count()
    }
}

const STATED: &str = r#"{"overlay":{"board":"demo","sha256":"0123abcd"}}"#;

#[test]
fn the_overlay_compiles_applies_and_holds_what_the_generator_states() {
    let Some(f) = Fixture::new() else { return };
    let out = f
        .run("ok", CONTRACT, OVERLAY, Some(STATED), BASE, "a")
        .expect("compiled");
    let bytes = std::fs::read(&out).expect("read");
    assert_eq!(&bytes[..4], &[0xd0, 0x0d, 0xfe, 0xed], "a device tree blob");
    // Another file of the generator's output: found in the store.
    f.run("ok", CONTRACT, OVERLAY, Some(STATED), BASE, "b")
        .expect("again");
    assert_eq!(f.made(), 1);
    // `{}` states nothing, and the overlay still compiles and applies.
    f.run("none", CONTRACT, OVERLAY, Some("{}"), BASE, "a")
        .expect("nothing stated");
}

#[test]
fn an_overlay_that_does_not_apply_to_the_base_fails() {
    let Some(f) = Fixture::new() else { return };
    // The base has no node labelled `bus`.
    let other = BASE.replace("bus: bus@1000", "port: bus@1000");
    let why = f
        .run("no-apply", CONTRACT, OVERLAY, Some(STATED), &other, "a")
        .expect_err("the overlay names a label the base lacks");
    println!("{why}");
    assert!(why.contains("gaia.devicetree.overlay: exited"), "{why}");
}

#[test]
fn a_marker_the_compile_lost_and_a_listed_warning_fail() {
    let Some(f) = Fixture::new() else { return };
    // The generator states a digest the source does not carry.
    let stated = r#"{"overlay":{"board":"demo","sha256":"ffffffff"}}"#;
    let why = f
        .run("lost", CONTRACT, OVERLAY, Some(stated), BASE, "a")
        .expect_err("the stated digest is not in the overlay");
    assert!(
        why.contains("the compiled overlay does not hold what the generator states about it"),
        "{why}"
    );
    assert!(
        why.contains("ffffffff") && !why.contains("\n  demo"),
        "{why}"
    );

    // A node with a unit address and no reg: a warning class the contract
    // lists as an error.
    let warns = OVERLAY.replace("marker {", "marker@5 {");
    let why = f
        .run("warn", CONTRACT, &warns, None, BASE, "a")
        .expect_err("a listed warning is an error");
    assert!(why.contains("gaia.devicetree.overlay: exited"), "{why}");
    // The same source with that class not listed compiles.
    let lax = CONTRACT.replace("\"unit_address_vs_reg\", ", "");
    f.run("lax", &lax, &warns, None, BASE, "a")
        .expect("not listed, so a warning only");
}

#[test]
fn a_compiler_older_than_the_minimum_and_a_bad_contract_are_refused() {
    let Some(f) = Fixture::new() else { return };
    let newer = CONTRACT.replace("minimum_dtc = \"1.6.0\"", "minimum_dtc = \"99.0.0\"");
    let why = f
        .run("old", &newer, OVERLAY, None, BASE, "a")
        .expect_err("this machine's dtc is older than 99");
    assert!(
        why.contains("older than the 99.0.0 the overlay's owner checks with"),
        "{why}"
    );

    let read = |text: &str| overlay_compiler(text, "dtc.toml");
    assert_eq!(
        read(&CONTRACT.replace("version = 1", "version = 2")),
        Err("dtc.toml: version 2; this build reads version 1".into())
    );
    let err = read(&format!("{CONTRACT}strict = true\n")).expect_err("unknown key");
    assert!(err.contains("unknown field `strict`"), "{err}");
    let err = read(&CONTRACT.replace("\"alias_paths\"", "\"-o /etc/x\"")).expect_err("class");
    assert!(err.contains("is not a warning class"), "{err}");
    let (args, minimum) = read(CONTRACT).expect("reads");
    assert_eq!(
        args,
        [
            "-@",
            "-I",
            "dts",
            "-O",
            "dtb",
            "-E",
            "unit_address_vs_reg",
            "-E",
            "alias_paths"
        ]
    );
    assert_eq!(minimum, [1, 6, 0]);
    assert_eq!(
        overlay_version_of("Version: DTC 1.7.2"),
        Some(vec![1, 7, 2])
    );
    assert_eq!(
        overlay_version_of("Version: DTC 1.8.1-g1234"),
        Some(vec![1, 8, 1])
    );
    assert_eq!(overlay_version_of("no version here"), None);
    assert!(vec![1, 7, 2] < vec![1, 8, 0] && vec![1, 10, 0] > vec![1, 9, 9]);
}
