//! A product with no OS: a microcontroller board whose one image is a
//! `.bin` its component builds. The OS provider gives the empty tree, so
//! the `.bin` is the image's only output, and the report says so.

use gaia_describe::{Product, Source};
use gaia_os_none::os_none;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Tool, logs_dir, step_records};
use gaia_store::{Store, tree_digest};
use gaia_types::{Artifact, ArtifactKind, Os, Target};
use std::path::{Path, PathBuf};

const BOARD: &str = r#"
version = 1
name = "mcu-board"
[target]
arch = "arm"
cpu = "cortex-m33"
os = "none"
"#;

const COMPONENT: &str = r#"
version = 1
name = "blink"
[sources.blink]
path = "src"
[build]
builder = "command"
source = "blink"
[build.settings]
run = "cat \"$SRC/vectors.bin\" \"$SRC/text.bin\" > \"$OUT/blink.bin\""
output = "blink.bin"
kind = "bin"
"#;

const PRODUCT: &str = r#"
version = 1
name = "blink-firmware"
[components]
blink = "components/blink/component.toml"
[[images]]
name = "blink"
board = "boards/mcu-board/board.toml"
components = ["blink"]
recipe = "firmware"
"#;

fn write(path: &Path, text: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// The description files, with the component's source beside it.
fn describe(dir: &Path) -> PathBuf {
    write(&dir.join("boards/mcu-board/board.toml"), BOARD);
    write(&dir.join("components/blink/component.toml"), COMPONENT);
    write(
        &dir.join("components/blink/src/vectors.bin"),
        [0x00, 0x20, 0x00, 0x20, 0x09, 0x01, 0x00, 0x00],
    );
    write(&dir.join("components/blink/src/text.bin"), [0xfe, 0xe7]);
    write(&dir.join("product.toml"), PRODUCT);
    dir.join("product.toml")
}

/// What C6's command builder does, in the least that shows the shape: the
/// declared command over the source tree, its declared output kept.
fn build_command(store: &Store, src: &Path, run: &str, output: &str) -> Result<PathBuf, StepError> {
    const NAME: &str = "test.build.command";
    let digest = tree_digest(src).expect("digest");
    let key = KeyBuilder::new(NAME, 1)
        .config("run", run)
        .input("src", digest)
        .finish();
    let made = Step::new(store, &Ctx::new(), NAME, key).look_up_or_make(
        &[(output, Path::new(output))],
        |work| {
            let out = work.dir().to_path_buf();
            let tool = Tool::new("/bin/sh")
                .args(["-c", run])
                .env("SRC", src)
                .env("OUT", &out);
            work.run(&tool).map(drop)
        },
    )?;
    Ok(store.path(&made.outputs[output]))
}

#[test]
fn a_product_with_no_os_is_one_bin_and_a_report() {
    let root = tempfile::tempdir().expect("tempdir");
    let product = describe(&root.path().join("product"));
    let loaded = Product::load(&product).expect("loads");
    let image = &loaded.product.images[0];
    let board = &loaded.boards[image.board.path()];
    assert_eq!(board.target.os, Os::None);
    let store_root = root.path().join("store");
    let store_str = store_root.display().to_string();

    // The OS provider: the empty tree, made once.
    let target: Target = board.target.clone();
    let rootfs = os_none(&store_str, target.clone(), &Ctx::new()).expect("os none");
    let store = Store::open(&store_root).expect("store");
    let rootfs_dir = rootfs.object.path_in(&store).expect("rootfs in store");
    assert_eq!(std::fs::read_dir(&rootfs_dir).expect("read").count(), 0);
    os_none(&store_str, target.clone(), &Ctx::new()).expect("again");

    // The component's build: one `.bin`, a kind Gaia does not list.
    let component = &loaded.components["blink"];
    let build = component.build.as_ref().expect("build");
    let Source::Path { path } = &component.sources[&build.source] else {
        panic!("a path source");
    };
    let setting = |name: &str| build.settings[name].as_str().expect(name).to_string();
    let bin =
        build_command(&store, path.path(), &setting("run"), &setting("output")).expect("builds");
    let artifact = Artifact {
        kind: ArtifactKind::parse(&setting("kind")),
        object: store.put_file(&bin).expect("put").into(),
    };
    assert_eq!(artifact.kind, ArtifactKind::Other("bin".into()));

    // The image's outputs: what the OS gives (nothing) and what was built.
    let mut outputs = Vec::new();
    if std::fs::read_dir(&rootfs_dir)
        .expect("read")
        .next()
        .is_some()
    {
        outputs.push(("rootfs".to_string(), "tree".to_string(), rootfs_dir.clone()));
    }
    outputs.push((setting("output"), artifact.kind.to_string(), bin.clone()));
    assert_eq!(outputs.len(), 1, "one output: the .bin");

    let report = serde_json::json!({
        "product": loaded.product.name,
        "image": image.name,
        "target": { "arch": target.arch.as_str(), "cpu": target.cpu, "os": target.os.as_str() },
        "os": { "provider": "gaia.os.none", "rootfs": tree_digest(&rootfs_dir).expect("digest").to_string() },
        "outputs": outputs.iter().map(|(name, kind, path)| serde_json::json!({
            "name": name,
            "kind": kind,
            "size": std::fs::metadata(path).expect("size").len(),
            "digest": gaia_store::file_digest(path).expect("digest").to_string(),
        })).collect::<Vec<_>>(),
    });
    let report_path = root.path().join("report.json");
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report).expect("json"),
    )
    .expect("write");
    println!("{}", std::fs::read_to_string(&report_path).expect("read"));
    assert_eq!(
        std::fs::read(&bin).expect("read"),
        [0x00, 0x20, 0x00, 0x20, 0x09, 0x01, 0x00, 0x00, 0xfe, 0xe7]
    );
    assert_eq!(report["outputs"][0]["size"], 10);

    // The provider ran once; the second ask found it.
    let made: Vec<(String, bool)> = step_records(&logs_dir(&store))
        .into_iter()
        .map(|r| (r.step, r.made))
        .collect();
    println!("steps: {made:?}");
    assert_eq!(
        made[..2],
        [
            ("gaia.os.none".into(), true),
            ("gaia.os.none".into(), false)
        ]
    );
}

#[test]
fn each_target_has_its_own_key() {
    let root = tempfile::tempdir().expect("tempdir");
    let store_str = root.path().join("store").display().to_string();
    let m33 = Target {
        cpu: Some("cortex-m33".into()),
        ..Target::from_triple("thumbv8m.main-none-eabihf")
    };
    let rv = Target::from_triple("riscv32imac-unknown-none-elf");
    os_none(&store_str, m33.clone(), &Ctx::new()).expect("m33");
    os_none(&store_str, rv, &Ctx::new()).expect("riscv");
    os_none(&store_str, m33, &Ctx::new()).expect("m33 again");
    let store = Store::open(root.path().join("store")).expect("store");
    let records = step_records(&logs_dir(&store));
    let made: Vec<bool> = records.iter().map(|r| r.made).collect();
    assert_eq!(made, [true, true, false]);
    assert_ne!(records[0].key, records[1].key);
    // Every target's rootfs is the same empty tree.
    assert_eq!(records[0].outputs, records[1].outputs);
}
