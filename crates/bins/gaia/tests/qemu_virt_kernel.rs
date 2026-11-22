//! scripts/qemu-virt-kernel.sh's edit of a board's copy: the virt fragment
//! last among its fragments, the virt patches last among its kernel
//! patches, and a board that would drop its family's patches refused. The
//! build itself is stubbed (`GAIA=false`): only the copy it leaves is read.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn have(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A description tree: a family, a board extending it and a product.
fn description(dir: &Path, board_kernel: &str, family_kernel: &str) -> PathBuf {
    let devices = dir.join("devices");
    std::fs::create_dir_all(devices.join("families/f")).expect("mkdir");
    std::fs::create_dir_all(devices.join("boards/b")).expect("mkdir");
    std::fs::create_dir_all(devices.join("products/p")).expect("mkdir");
    std::fs::write(
        devices.join("families/f/family.toml"),
        format!("version = 1\nname = \"f\"\n\n[kernel]\nsource = \"linux\"\n{family_kernel}"),
    )
    .expect("family");
    std::fs::write(
        devices.join("boards/b/board.toml"),
        format!(
            "version = 1\nextends = \"../../families/f/family.toml\"\nname = \"b\"\n\n[kernel]\n{board_kernel}\n[bootloader]\ndefconfig = \"x\"\n"
        ),
    )
    .expect("board");
    std::fs::write(
        devices.join("products/p/product.toml"),
        "version = 1\n[[images]]\nname = \"img\"\n",
    )
    .expect("product");
    devices
}

/// Runs the script; returns its success, its output and the board copy's
/// [kernel] table (or None when there is no copy).
fn run(dir: &Path, devices: &Path) -> (bool, String, Option<serde_json::Value>) {
    let out = dir.join("out");
    let result = Command::new(repo().join("scripts/qemu-virt-kernel.sh"))
        .args([
            devices.to_str().expect("utf-8"),
            "products/p/product.toml",
            "boards/b/board.toml",
            out.to_str().expect("utf-8"),
        ])
        .env("GAIA", "false")
        .env("GAIA_STORE", dir.join("store"))
        .output()
        .expect("the script runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let copy = out.join("devices/boards/b/board.toml");
    let kernel = copy.exists().then(|| {
        // Read as TOML by Python's tomllib, the reader the script uses.
        let json = Command::new("python3")
            .args([
                "-I",
                "-c",
                "import json,sys,tomllib; print(json.dumps(tomllib.load(open(sys.argv[1],'rb'))['kernel']))",
            ])
            .arg(&copy)
            .output()
            .expect("python3");
        assert!(json.status.success(), "the copy is TOML: {}", String::from_utf8_lossy(&json.stderr));
        serde_json::from_slice(&json.stdout).expect("json")
    });
    (result.status.success(), text, kernel)
}

fn patch_files(kernel: &serde_json::Value) -> Vec<String> {
    kernel["patches"]
        .as_array()
        .expect("patches")
        .iter()
        .map(|p| p["file"].as_str().expect("file").to_string())
        .collect()
}

/// The virt patches as the board's copy names them, in name order.
fn virt_patches() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo().join("scripts/qemu-virt-kernel"))
        .expect("the patches")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".patch"))
        .map(|n| format!("qemu-virt-kernel/{n}"))
        .collect();
    names.sort();
    assert!(
        names.len() >= 4,
        "the watchdog's, the UDC's, virtio_blk's and the mailbox's patches: {names:?}"
    );
    names
}

#[test]
fn the_virt_fragment_and_patches_go_last_on_the_boards_copy() {
    if !have("python3") {
        return;
    }
    // No patches on the board or its family: the board gets the virt ones.
    let dir = tempfile::tempdir().expect("tempdir");
    let devices = description(dir.path(), "fragments = [\"a.config\"]\n", "");
    let (ok, out, kernel) = run(dir.path(), &devices);
    assert!(!ok && out.contains("the kernel build failed"), "{out}");
    let kernel = kernel.expect("a copy");
    let fragments: Vec<&str> = kernel["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|f| f.as_str().expect("str"))
        .collect();
    assert_eq!(fragments, ["a.config", "qemu-virt-kernel.config"]);
    assert_eq!(patch_files(&kernel), virt_patches());
    for patch in virt_patches() {
        let copied = dir.path().join("out/devices/boards/b").join(&patch);
        assert_eq!(
            std::fs::read(&copied).expect("the patch is copied beside the board"),
            std::fs::read(repo().join("scripts").join(&patch)).expect("the patch")
        );
    }
    // The original is not written.
    assert!(
        !std::fs::read_to_string(devices.join("boards/b/board.toml"))
            .expect("board")
            .contains("qemu-virt")
    );

    // The board's own patches stay first.
    let dir = tempfile::tempdir().expect("tempdir");
    let devices = description(
        dir.path(),
        "fragments = [\"a.config\"]\npatches = [\n  { file = \"own.patch\" },\n]\n",
        "",
    );
    let (_, out, kernel) = run(dir.path(), &devices);
    let mut want = vec!["own.patch".to_string()];
    want.extend(virt_patches());
    assert_eq!(patch_files(&kernel.expect("a copy")), want, "{out}");

    // A board with none while its family has some: its list would replace
    // the family's, so it is refused.
    let dir = tempfile::tempdir().expect("tempdir");
    let devices = description(
        dir.path(),
        "fragments = [\"a.config\"]\n",
        "patches = [{ file = \"family.patch\" }]\n",
    );
    let (ok, out, _) = run(dir.path(), &devices);
    assert!(
        !ok && out.contains("names no kernel `patches` while its family does"),
        "{out}"
    );
}
