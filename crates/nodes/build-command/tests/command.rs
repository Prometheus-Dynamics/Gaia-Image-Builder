//! The command builder: a declared `.bin` made, an undeclared output
//! refused, an undeclared input not there.
//!
//! The image is a small root filesystem built here from this host's shell,
//! a few tools and the libraries they load. Where bubblewrap or user
//! namespaces are missing the tests say SKIPPED, unless
//! `GAIA_REQUIRE_SANDBOX=1`, when they fail.

use gaia_build_command::{Command, CommandInput, CommandOutput, build_command};
use gaia_step::{Ctx, logs_dir, step_records};
use gaia_store::{Store, file_digest};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const BWRAP: &str = "/usr/bin/bwrap";
const TOOLS: [&str; 8] = ["sh", "cat", "ls", "cp", "printf", "mkdir", "head", "id"];

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

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
    image: Tree,
}

/// A store and an image in it: the host's tools and their libraries under
/// `usr`, the merged-/usr links, an `etc`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let image = root.join("image");
    for sub in ["usr/bin", "usr/lib64", "etc"] {
        std::fs::create_dir_all(image.join(sub)).expect("mkdir");
    }
    for (link, target) in [
        ("bin", "usr/bin"),
        ("sbin", "usr/bin"),
        ("lib64", "usr/lib64"),
        ("lib", "usr/lib64"),
    ] {
        std::os::unix::fs::symlink(target, image.join(link)).expect("symlink");
    }
    let mut libs = std::collections::BTreeSet::new();
    for tool in TOOLS {
        let host = which(tool);
        std::fs::copy(&host, image.join("usr/bin").join(tool)).expect("copy tool");
        let ldd = std::process::Command::new("ldd")
            .arg(&host)
            .output()
            .expect("ldd");
        for line in String::from_utf8_lossy(&ldd.stdout).lines() {
            let path = line
                .split("=>")
                .last()
                .and_then(|rest| rest.split_whitespace().next())
                .filter(|p| p.starts_with('/'));
            if let Some(path) = path {
                libs.insert(PathBuf::from(path));
            }
        }
    }
    for lib in libs {
        let name = lib.file_name().expect("name");
        std::fs::copy(&lib, image.join("usr/lib64").join(name)).expect("copy lib");
    }
    std::fs::write(image.join("etc/image-file"), "from the image\n").expect("write");
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&image).expect("put").into(),
        entries: Vec::new(),
    };
    Fixture {
        store: store_path.display().to_string(),
        root,
        _dir: dir,
        image: tree,
    }
}

fn which(tool: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(tool))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no {tool} on this host"))
}

fn tree(f: &Fixture, name: &str, files: &[(&str, &[u8])]) -> Tree {
    let dir = f.root.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (path, bytes) in files {
        std::fs::write(dir.join(path), bytes).expect("write");
    }
    let store = Store::open(&f.store).expect("store");
    Tree {
        object: store.put_tree(&dir).expect("put").into(),
        entries: Vec::new(),
    }
}

fn script(text: &str) -> Command {
    Command {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), text.into()],
        env: vec!["NAME=blink".into()],
        cwd: String::new(),
        network: false,
        jobs: 0,
    }
}

fn output(path: &str, kind: &str) -> CommandOutput {
    CommandOutput {
        path: path.into(),
        kind: kind.into(),
    }
}

#[test]
fn a_declared_bin_is_made_and_found_again() {
    if !available() {
        return;
    }
    let f = fixture();
    let src = tree(
        &f,
        "src",
        &[
            ("vectors.bin", &[0, 0x20, 0, 0x20]),
            ("text.bin", &[0xfe, 0xe7]),
        ],
    );
    let inputs = vec![CommandInput {
        tree: src,
        at: "/src".into(),
    }];
    let command = script(
        "cat /src/vectors.bin /src/text.bin > /out/$NAME.bin && mkdir -p /out/map && printf 'ok\\n' > /out/map/sections",
    );
    let outputs = vec![output("blink.bin", "bin"), output("map", "tree")];
    let built = build_command(
        &f.store,
        BWRAP,
        f.image.clone(),
        inputs.clone(),
        command.clone(),
        outputs.clone(),
        &Ctx::new(),
    )
    .expect("builds");
    let store = Store::open(&f.store).expect("store");
    let bin = built[0].object.path_in(&store).expect("in store");
    println!(
        "{} {} -> {}",
        built[0].kind,
        file_digest(&bin).expect("digest"),
        bin.display()
    );
    assert_eq!(built[0].kind, ArtifactKind::Other("bin".into()));
    assert_eq!(
        std::fs::read(&bin).expect("read"),
        [0, 0x20, 0, 0x20, 0xfe, 0xe7]
    );
    assert_eq!(built[1].kind, ArtifactKind::Tree);
    let map = built[1].object.path_in(&store).expect("in store");
    assert_eq!(
        std::fs::read_to_string(map.join("sections")).expect("read"),
        "ok\n"
    );
    let again = build_command(
        &f.store,
        BWRAP,
        f.image.clone(),
        inputs,
        command,
        outputs,
        &Ctx::new(),
    )
    .expect("again");
    assert_eq!(again, built);
    let made: Vec<bool> = step_records(&logs_dir(&store))
        .iter()
        .map(|r| r.made)
        .collect();
    println!("made: {made:?}");
    assert_eq!(made, [true, false]);
}

#[test]
fn an_undeclared_output_is_an_error_naming_it() {
    if !available() {
        return;
    }
    let f = fixture();
    let fails = |text: &str, outputs: Vec<CommandOutput>| -> String {
        let error = build_command(
            &f.store,
            BWRAP,
            f.image.clone(),
            Vec::new(),
            script(text),
            outputs,
            &Ctx::new(),
        )
        .expect_err("refused");
        println!("{error}");
        error.to_string()
    };
    let error = fails(
        "printf a > /out/blink.bin; printf b > /out/stray.o; mkdir /out/obj; printf c > /out/obj/x.o",
        vec![output("blink.bin", "bin")],
    );
    assert!(
        error.contains("undeclared output /out/obj, /out/stray.o"),
        "{error}"
    );
    let error = fails(
        "printf a > /out/other.bin",
        vec![output("blink.bin", "bin")],
    );
    assert!(
        error.contains("declared output /out/blink.bin was not made"),
        "{error}"
    );
    let error = fails("mkdir /out/blink.bin", vec![output("blink.bin", "bin")]);
    assert!(
        error.contains("declared output /out/blink.bin is not a file"),
        "{error}"
    );
    // Nothing of a refused run is kept.
    let store = Store::open(&f.store).expect("store");
    assert!(step_records(&logs_dir(&store)).iter().all(|r| !r.made));
}

#[test]
fn an_undeclared_input_is_not_visible() {
    if !available() {
        return;
    }
    let f = fixture();
    // A host file next to the declared input, and the store itself.
    let secret = f.root.join("secret.txt");
    std::fs::write(&secret, "host only\n").expect("write");
    let src = tree(&f, "src", &[("declared.txt", b"declared\n")]);
    let probe = format!(
        "{{ ls /; ls /src; cat /etc/image-file; cat /src/declared.txt; \
         cat {secret} 2>&1 || echo 'secret: unreadable'; \
         ls {store} 2>&1 || echo 'store: unreadable'; \
         ls /home 2>&1 || echo 'home: unreadable'; id -u; }} > /out/seen.txt",
        secret = secret.display(),
        store = f.store,
    );
    let built: Vec<Artifact> = build_command(
        &f.store,
        BWRAP,
        f.image.clone(),
        vec![CommandInput {
            tree: src,
            at: "/src".into(),
        }],
        script(&probe),
        vec![output("seen.txt", "text")],
        &Ctx::new(),
    )
    .expect("runs");
    let store = Store::open(&f.store).expect("store");
    let seen = std::fs::read_to_string(built[0].object.path_in(&store).expect("in")).expect("read");
    println!("{seen}");
    let lines: Vec<&str> = seen.lines().collect();
    assert!(seen.contains("from the image") && seen.contains("declared\n"));
    assert!(!seen.contains("host only"), "the host file was read");
    assert!(seen.contains("secret: unreadable") && seen.contains("store: unreadable"));
    assert!(seen.contains("home: unreadable"));
    // The root holds the image's entries, the input, /out and the
    // sandbox's own /proc, /dev, /tmp; nothing of the host.
    let root: Vec<&str> = lines
        .iter()
        .take_while(|l| **l != "declared.txt")
        .copied()
        .collect();
    assert_eq!(
        root,
        [
            "bin", "dev", "etc", "lib", "lib64", "out", "proc", "sbin", "src", "tmp", "usr"
        ]
    );
}
