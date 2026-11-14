//! `gaia sizes`: a run's sizes read from its record and the store.

use gaia_store::Store;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, bytes).expect("write");
}

#[test]
fn the_sizes_of_a_run_are_read_from_its_record_and_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let store = Store::open(root.join("store")).expect("store");
    // What a build with a kernel, a merged root and a boot partition leaves.
    write(&root.join("kernel/Image"), &[7u8; 5000]);
    write(&root.join("modules/6.1/kernel/a.ko"), &[1u8; 300]);
    write(&root.join("modules/6.1/kernel/b.ko.xz"), &[1u8; 200]);
    write(&root.join("modules/6.1/modules.dep"), &[1u8; 50]);
    write(&root.join("bootloader/u-boot.bin"), &[2u8; 900]);
    write(&root.join("merged/usr/bin/app"), &[3u8; 4000]);
    write(&root.join("merged/etc/app.conf"), &[3u8; 10]);
    write(&root.join("merged/bin/sh"), &[3u8; 700]);
    write(
        &root.join("owners.json"),
        br#"[
  {"path": "usr", "mode": 493, "uid": 0, "gid": 0, "owners": ["os", "app"]},
  {"path": "usr/bin/app", "mode": 493, "uid": 0, "gid": 0, "owners": ["app"]},
  {"path": "etc/app.conf", "mode": 420, "uid": 0, "gid": 0, "owners": ["app"]},
  {"path": "bin/sh", "mode": 493, "uid": 0, "gid": 0, "owners": ["os"]}
]"#,
    );
    write(&root.join("boot.img"), &[0u8; 8192]);
    write(
        &root.join("report.txt"),
        b"kernel 5000 8192\ncmdline 12 4096\n",
    );
    write(&root.join("erofs.img"), &[4u8; 4096]);
    let tree = |name: &str| store.put_tree(&root.join(name)).expect("put").to_string();
    let file = |name: &str| store.put_file(&root.join(name)).expect("put").to_string();
    let record = serde_json::json!({
        "product": "product.toml",
        "start_ns": 1,
        "end_ns": 2,
        "outcome": "done",
        "outputs": {
            "img.slot": {"ty": "gaia:image", "object": file("erofs.img"), "from": []},
        },
        "steps": [
            {"step": "gaia.os.buildroot", "key": "sha256:01", "made": true, "ms": 1, "outputs": {
                "kernel": tree("kernel"), "modules": tree("modules"), "bootloader": tree("bootloader"),
            }},
            {"step": "gaia.tree.merge", "key": "sha256:02", "made": true, "ms": 1, "outputs": {
                "tree": tree("merged"), "owners": file("owners.json"),
            }},
            {"step": "gaia.fs.erofs", "key": "sha256:03", "made": true, "ms": 1, "outputs": {
                "image": file("erofs.img"),
            }},
            // Looked up a second time in the same run: one thing, once.
            {"step": "gaia.fs.erofs", "key": "sha256:03", "made": false, "ms": 0, "outputs": {
                "image": file("erofs.img"),
            }},
            {"step": "gaia.generate.image", "key": "sha256:04", "made": true, "ms": 1, "outputs": {
                "image": file("boot.img"), "report": file("report.txt"),
            }},
        ],
    });
    let run = root.join("run.json");
    std::fs::write(&run, serde_json::to_vec(&record).expect("json")).expect("write");

    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(["sizes", "--json", "--store"])
        .arg(root.join("store"))
        .arg(&run)
        .output()
        .expect("gaia runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let rows: Vec<(String, u64, String)> = shown["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            (
                r["what"].as_str().expect("what").to_string(),
                r["bytes"].as_u64().expect("bytes"),
                r["detail"].as_str().expect("detail").to_string(),
            )
        })
        .collect();
    let find = |what: &str, detail: &str| {
        rows.iter()
            .find(|(w, _, d)| w == what && d.starts_with(detail))
            .unwrap_or_else(|| panic!("no row {what} / {detail} in {rows:#?}"))
            .1
    };
    assert_eq!(find("output img.slot", ""), 4096);
    assert_eq!(find("kernel Image", "raw"), 5000);
    // Five thousand equal bytes gzip to a few dozen.
    assert!(find("kernel Image", "gzip") < 100);
    assert_eq!(find("kernel modules", "2 modules"), 500);
    assert_eq!(find("kernel modules tree", "3 files"), 550);
    assert_eq!(find("bootloader u-boot.bin", ""), 900);
    // Each component's bytes in the merged tree, the largest first; a
    // directory is nobody's bytes.
    let merged: Vec<&(String, u64, String)> =
        rows.iter().filter(|(w, _, _)| w.contains(" in ")).collect();
    assert_eq!(merged.len(), 2, "{rows:#?}");
    assert!(merged[0].0.ends_with(": app") && merged[0].1 == 4010 && merged[0].2 == "2 files");
    assert!(merged[1].0.ends_with(": os") && merged[1].1 == 700);
    assert_eq!(
        rows.iter()
            .filter(|(w, _, _)| w == "filesystem erofs")
            .count(),
        1
    );
    assert_eq!(find("generated image (boot partition)", ""), 8192);
    assert_eq!(find("payload kernel", "of 8192 in its bay"), 5000);
    assert_eq!(find("payload cmdline", "of 4096 in its bay"), 12);

    // The text form: one row a line, and the run it read.
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(["sizes", "--store"])
        .arg(root.join("store"))
        .arg(&run)
        .output()
        .expect("gaia runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("run      "), "{text}");
    assert!(
        text.lines()
            .any(|line| line.starts_with("kernel modules ") && line.contains("  2 modules")),
        "{text}"
    );
}
