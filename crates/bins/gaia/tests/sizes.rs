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
    // This record's erofs step has no read-back output: its row says so.
    assert!(
        rows.iter()
            .any(|(w, _, d)| w == "filesystem erofs"
                && d.ends_with("NOT read back after it was made")),
        "{rows:#?}"
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

/// A FAT12 volume of 64 sectors made by hand: `A.TXT` (5 bytes), a long
/// name, a gzip file, and a file in a directory.
fn fat_volume() -> Vec<u8> {
    let mut v = vec![0u8; 64 * 512];
    v[11..13].copy_from_slice(&512u16.to_le_bytes());
    v[13] = 1; // sectors a cluster
    v[14..16].copy_from_slice(&1u16.to_le_bytes()); // reserved
    v[16] = 1; // FATs
    v[17..19].copy_from_slice(&16u16.to_le_bytes()); // root entries
    v[19..21].copy_from_slice(&64u16.to_le_bytes()); // sectors
    v[22..24].copy_from_slice(&1u16.to_le_bytes()); // sectors a FAT
    v[54..59].copy_from_slice(b"FAT12");
    v[510] = 0x55;
    v[511] = 0xAA;
    // Clusters 2 to 5, each the end of its own chain: twelve bits each.
    let fat = 512;
    let mut set = |cluster: usize, value: u16| {
        let at = fat + cluster * 3 / 2;
        let pair = u16::from_le_bytes([v[at], v[at + 1]]);
        let pair = if cluster.is_multiple_of(2) {
            (pair & 0xF000) | value
        } else {
            (pair & 0x000F) | (value << 4)
        };
        v[at..at + 2].copy_from_slice(&pair.to_le_bytes());
    };
    for cluster in 2..6 {
        set(cluster, 0xFFF);
    }
    let (root, data) = (2 * 512, 3 * 512);
    let short = |name: &[u8; 11], attr: u8, cluster: u16, size: u32| {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = attr;
        e[26..28].copy_from_slice(&cluster.to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    };
    let mut gz = Vec::new();
    {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::best());
        encoder.write_all(&[0u8; 3000]).expect("gzip");
        encoder.finish().expect("gzip");
    }
    assert!(gz.len() < 512);
    // The long name `settings.conf`, one entry before its short one.
    let mut long = [0xFFu8; 32];
    long[0] = 0x41;
    long[11] = 0x0F;
    long[12] = 0;
    long[26] = 0;
    long[27] = 0;
    let name: Vec<u16> = "settings.conf".encode_utf16().chain([0]).collect();
    for (unit, at) in name
        .iter()
        .zip([1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30])
    {
        long[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    let entries = [
        short(b"A       TXT", 0x20, 2, 5),
        long,
        short(b"SETTIN~1CON", 0x20, 3, 7),
        short(b"KERNEL  IMG", 0x20, 4, gz.len() as u32),
        short(b"SUB        ", 0x10, 5, 0),
    ];
    for (n, entry) in entries.iter().enumerate() {
        v[root + n * 32..root + (n + 1) * 32].copy_from_slice(entry);
    }
    v[data..data + 5].copy_from_slice(b"hello");
    v[data + 512..data + 512 + 7].copy_from_slice(b"a=1\nb=2");
    v[data + 1024..data + 1024 + gz.len()].copy_from_slice(&gz);
    // The directory's one file is empty: no cluster of its own.
    let inner = short(b"INNER   BIN", 0x20, 0, 0);
    v[data + 1536..data + 1536 + 32].copy_from_slice(&inner);
    v
}

#[test]
fn an_image_this_builder_did_not_make_is_measured_from_its_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A disk: an MBR, the FAT volume at sector 1, an unknown partition.
    let volume = fat_volume();
    let mut disk = vec![0u8; 512];
    let mut entry = |n: usize, kind: u8, start: u32, sectors: u32| {
        let at = 446 + n * 16;
        disk[at + 4] = kind;
        disk[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
        disk[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
    };
    entry(0, 0x0C, 1, 64);
    entry(1, 0x83, 65, 8);
    disk[510] = 0x55;
    disk[511] = 0xAA;
    disk.extend(&volume);
    // The second partition holds an ext filesystem's magic and nothing else.
    let mut ext = vec![0u8; 8 * 512];
    ext[1080..1082].copy_from_slice(&0xEF53u16.to_le_bytes());
    disk.extend(&ext);
    let raw = dir.path().join("disk.img");
    std::fs::write(&raw, &disk).expect("write");
    let packed = dir.path().join("disk.img.xz");
    {
        use std::io::Write;
        let mut encoder =
            xz2::write::XzEncoder::new(std::fs::File::create(&packed).expect("create"), 1);
        encoder.write_all(&disk).expect("xz");
        encoder.finish().expect("xz");
    }
    for image in [&raw, &packed] {
        let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
            .args(["sizes", "--json", "--image"])
            .arg(image)
            .env("TMPDIR", dir.path())
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
                    r["what"].as_str().expect("what").trim().to_string(),
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
        assert_eq!(find("image raw", ""), disk.len() as u64);
        assert_eq!(
            rows.iter().any(|(w, _, _)| w == "image xz"),
            image == &packed
        );
        assert_eq!(find("partition 1 (type 0x0c)", "fat"), 64 * 512);
        assert_eq!(find("partition 2 (type 0x83)", "ext2/3/4"), 8 * 512);
        assert_eq!(find("A.TXT", ""), 5);
        assert_eq!(find("settings.conf", ""), 7);
        // The gzip file as stored, and what it unpacks to.
        assert!(find("KERNEL.IMG", "") < 512);
        assert_eq!(find("KERNEL.IMG", "unpacked (gzip)"), 3000);
        assert_eq!(find("SUB/INNER.BIN", ""), 0);
        // What it did not read is a row, not a silence.
        assert!(
            rows.iter()
                .any(|(w, _, d)| w == "not measured"
                    && d.contains("partition 2 (type 0x83), ext2/3/4")),
            "{rows:#?}"
        );
    }
    // The volume alone, with no disk around it.
    let alone = dir.path().join("boot.img");
    std::fs::write(&alone, &volume).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(["sizes", "--image"])
        .arg(&alone)
        .output()
        .expect("gaia runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("volume") && text.contains("settings.conf"),
        "{text}"
    );
}
