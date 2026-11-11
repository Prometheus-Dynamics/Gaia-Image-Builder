//! Importing a raw GPT image (FAT boot, ext4 root) and a rootfs archive.

use gaia_os_import::{Owners, os_import};
use gaia_step::{Ctx, logs_dir, step_records};
use gaia_store::{Store, tree_digest};
use gaia_types::{Artifact, ArtifactKind, FileEntry, Tree};
use std::io::Write;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The host tools the image is built with, or `None` (skipped) when one
/// is missing. With GAIA_REQUIRE_SANDBOX=1 a missing tool fails instead.
fn tools() -> Option<[PathBuf; 3]> {
    let find = |name: &str| {
        ["/usr/sbin", "/sbin", "/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(name))
            .find(|p| p.exists())
    };
    match (find("mkfs.ext4"), find("debugfs"), find("sfdisk")) {
        (Some(a), Some(b), Some(c)) => Some([a, b, c]),
        missing => {
            let msg = format!("SKIPPED: mkfs.ext4, debugfs or sfdisk missing: {missing:?}");
            assert!(
                std::env::var("GAIA_REQUIRE_SANDBOX").as_deref() != Ok("1"),
                "{msg}"
            );
            println!("{msg}");
            None
        }
    }
}

fn run(cmd: &mut Command, input: Option<&str>) {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    if let Some(text) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(text.as_bytes())
            .expect("stdin");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{cmd:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const MIB: u64 = 1024 * 1024;

/// A 24 MiB disk: GPT, partition 1 a 4 MiB FAT "boot", partition 2 an
/// 16 MiB ext4 "rootfs" with a uid-1000 home, a setuid binary, a symlink
/// and a device node.
fn disk(dir: &Path, [mkfs, debugfs, sfdisk]: &[PathBuf; 3]) -> PathBuf {
    let src = dir.join("src");
    for (path, text) in [
        ("etc/hostname", "imported\n"),
        ("usr/bin/tool", "#!/bin/sh\n"),
        ("usr/bin/su", "su\n"),
        ("home/user/notes", "mine\n"),
        ("boot/extlinux.conf", "label linux\n"),
    ] {
        let at = src.join(path);
        std::fs::create_dir_all(at.parent().expect("parent")).expect("mkdir");
        std::fs::write(&at, text).expect("write");
    }
    std::fs::create_dir_all(src.join("dev")).expect("mkdir");
    std::fs::set_permissions(
        src.join("usr/bin/tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    std::os::unix::fs::symlink("usr/bin", src.join("bin")).expect("symlink");
    let root = dir.join("root.ext4");
    std::fs::File::create(&root)
        .expect("create")
        .set_len(16 * MIB)
        .expect("size");
    run(
        Command::new(mkfs)
            .args(["-q", "-F", "-L", "rootfs", "-d"])
            .arg(&src)
            .arg(&root),
        None,
    );
    // mkfs copies the source's owner (whoever runs the test): root first.
    let mut edits = String::new();
    for path in [
        "/",
        "/etc",
        "/etc/hostname",
        "/usr",
        "/usr/bin",
        "/usr/bin/tool",
        "/usr/bin/su",
        "/home",
        "/home/user",
        "/home/user/notes",
        "/boot",
        "/boot/extlinux.conf",
        "/dev",
        "/bin",
        "/lost+found",
    ] {
        edits.push_str(&format!("sif {path} uid 0\nsif {path} gid 0\n"));
    }
    edits += "sif /home/user uid 1000\nsif /home/user gid 1000\nsif /home/user/notes uid 1000\nsif /home/user/notes gid 100\nsif /usr/bin/su mode 0104755\ncd /dev\nmknod null c 1 3\nsif /dev/null mode 020666\n";
    run(
        Command::new(debugfs)
            .arg("-w")
            .arg("-f")
            .arg("-")
            .arg(&root),
        Some(&edits),
    );

    let fat = dir.join("boot.fat");
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&fat)
        .expect("fat");
    f.set_len(4 * MIB).expect("size");
    fatfs::format_volume(
        &f,
        fatfs::FormatVolumeOptions::new().volume_label(*b"BOOT       "),
    )
    .expect("format");
    {
        let fs = fatfs::FileSystem::new(&f, fatfs::FsOptions::new()).expect("fat");
        let efi = fs
            .root_dir()
            .create_dir("EFI")
            .expect("dir")
            .create_dir("BOOT")
            .expect("dir");
        efi.create_file("BOOTAA64.EFI")
            .expect("file")
            .write_all(b"MZ efi")
            .expect("write");
        fs.root_dir()
            .create_file("config.txt")
            .expect("file")
            .write_all(b"arm_64bit=1\n")
            .expect("write");
    }

    let img = dir.join("disk.img");
    std::fs::File::create(&img)
        .expect("create")
        .set_len(24 * MIB)
        .expect("size");
    let table = "label: gpt\nfirst-lba: 2048\nstart=2048, size=8192, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=boot\nstart=10240, size=32768, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=rootfs\n";
    run(
        Command::new(sfdisk)
            .args(["--no-reread", "--no-tell-kernel", "-q"])
            .arg(&img),
        Some(table),
    );
    let out = std::fs::OpenOptions::new()
        .write(true)
        .open(&img)
        .expect("img");
    out.write_all_at(&std::fs::read(&fat).expect("read"), 2048 * 512)
        .expect("write");
    out.write_all_at(&std::fs::read(&root).expect("read"), 10240 * 512)
        .expect("write");
    img
}

fn stored(store: &Path, file: &Path) -> Artifact {
    let s = Store::open(store).expect("store");
    Artifact {
        kind: ArtifactKind::parse("disk-image"),
        object: s.put_file(file).expect("put").into(),
    }
}

fn digest(store: &Path, tree: &Tree) -> String {
    let s = Store::open(store).expect("store");
    tree_digest(&tree.object.path_in(&s).expect("in store"))
        .expect("digest")
        .to_string()
}

#[test]
fn a_gpt_image_imports_to_a_stable_tree_with_owners() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let img = disk(dir.path(), &tools);
    let mut seen = Vec::new();
    for name in ["store-a", "store-b"] {
        let store = dir.path().join(name).join("store");
        let image = stored(&store, &img);
        let (rootfs, boot, owners) = os_import(
            &store.display().to_string(),
            image,
            "auto",
            "auto",
            &Ctx::new(),
        )
        .expect("imports");
        let (r, b) = (digest(&store, &rootfs), digest(&store, &boot));
        println!("{name}: rootfs {r}\n{name}: boot   {b}");
        seen.push((r, b, rootfs.entries.clone()));

        let s = Store::open(&store).expect("store");
        let root = rootfs.object.path_in(&s).expect("in");
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname")).expect("read"),
            "imported\n"
        );
        let mode = |p: &str| {
            std::fs::symlink_metadata(root.join(p))
                .expect(p)
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("usr/bin/su"), 0o4755);
        assert_eq!(mode("usr/bin/tool"), 0o755);
        assert_eq!(
            std::fs::read_link(root.join("bin")).expect("link"),
            Path::new("usr/bin")
        );
        assert!(
            !root
                .join("lost+found")
                .read_dir()
                .expect("dir")
                .any(|_| true)
        );
        let boot_dir = boot.object.path_in(&s).expect("in");
        assert_eq!(
            std::fs::read(boot_dir.join("EFI/BOOT/BOOTAA64.EFI")).expect("efi"),
            b"MZ efi"
        );
        assert!(boot_dir.join("config.txt").is_file());

        let owners: Owners = serde_json::from_slice(
            &std::fs::read(owners.object.path_in(&s).expect("in")).expect("read"),
        )
        .expect("json");
        assert_eq!(owners.specials.len(), 1);
        let null = &owners.specials[0];
        assert_eq!(
            (null.path.as_str(), null.kind.as_str(), null.mode),
            ("dev/null", "char", 0o666)
        );
        if name == "store-a" {
            for e in &rootfs.entries {
                println!("owner {:o} {}:{} {}", e.mode, e.uid, e.gid, e.path);
            }
            println!("special {} {} {:o}", null.path, null.kind, null.mode);
        }
    }
    assert_eq!(seen[0], seen[1], "two stores, one digest");
    let entries = &seen[0].2;
    assert!(entries.contains(&FileEntry {
        path: "home/user".into(),
        mode: 0o755,
        uid: 1000,
        gid: 1000
    }));
    assert!(entries.contains(&FileEntry {
        path: "home/user/notes".into(),
        mode: 0o644,
        uid: 1000,
        gid: 100
    }));
    assert_eq!(entries.len(), 2, "only non-root paths are listed");

    // The same partitions chosen by name and number: the same trees; a
    // second import is found, not made.
    let store = dir.path().join("store-a").join("store");
    let s = store.display().to_string();
    let image = stored(&store, &img);
    let (rootfs, boot, _) =
        os_import(&s, image.clone(), "label:rootfs", "1", &Ctx::new()).expect("by name");
    assert_eq!(
        (digest(&store, &rootfs), digest(&store, &boot)),
        (seen[0].0.clone(), seen[0].1.clone())
    );
    os_import(&s, image.clone(), "auto", "auto", &Ctx::new()).expect("again");
    let made: Vec<bool> = step_records(&logs_dir(&Store::open(&store).expect("store")))
        .iter()
        .map(|r| r.made)
        .collect();
    assert_eq!(made, [true, true, false]);

    // The root's /boot instead of the FAT partition.
    let (_, boot, _) =
        os_import(&s, image.clone(), "auto", "rootfs", &Ctx::new()).expect("rootfs boot");
    let b = Store::open(&store).expect("store");
    assert!(
        boot.object
            .path_in(&b)
            .expect("in")
            .join("extlinux.conf")
            .is_file()
    );

    // A FAT partition is not a root.
    let error = os_import(&s, image, "1", "none", &Ctx::new()).expect_err("fat root");
    println!("{error}");
    assert!(error.to_string().contains("is not ext2/3/4"));
}

fn tar_gz(entries: &[(&str, tar::EntryType, &str, u32, u64)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, kind, data, mode, uid) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_mode(*mode);
        header.set_uid(*uid);
        header.set_gid(*uid);
        header.set_mtime(1);
        if *kind == tar::EntryType::Symlink {
            header.set_link_name(data).expect("link");
            header.set_size(0);
        } else {
            header.set_size(data.len() as u64);
        }
        // Raw names, so a bad one reaches the importer.
        let name = header.as_old_mut().name.as_mut();
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        let body: &[u8] = if *kind == tar::EntryType::Symlink {
            b""
        } else {
            data.as_bytes()
        };
        builder.append(&header, body).expect("append");
    }
    builder.into_inner().expect("tar").finish().expect("gz")
}

#[test]
fn a_rootfs_archive_imports_with_owners_and_refuses_escapes() {
    use tar::EntryType::{Char, Directory, Regular, Symlink};
    let dir = tempfile::tempdir().expect("tempdir");
    let good = tar_gz(&[
        ("./", Directory, "", 0o755, 0),
        ("./etc/", Directory, "", 0o755, 0),
        ("./etc/hostname", Regular, "tarball\n", 0o644, 0),
        ("./srv/", Directory, "", 0o750, 33),
        ("./srv/index", Regular, "hi\n", 0o640, 33),
        ("./boot/", Directory, "", 0o755, 0),
        ("./boot/Image", Regular, "kernel", 0o644, 0),
        ("./sbin", Symlink, "usr/sbin", 0o777, 0),
        ("./dev/", Directory, "", 0o755, 0),
        ("./dev/console", Char, "", 0o600, 0),
    ]);
    let file = dir.path().join("rootfs.tar.gz");
    std::fs::write(&file, &good).expect("write");
    let store = dir.path().join("store");
    let s = store.display().to_string();
    let (rootfs, boot, owners) =
        os_import(&s, stored(&store, &file), "auto", "auto", &Ctx::new()).expect("imports");
    println!("archive rootfs {}", digest(&store, &rootfs));
    assert_eq!(
        rootfs.entries,
        [
            FileEntry {
                path: "srv".into(),
                mode: 0o750,
                uid: 33,
                gid: 33
            },
            FileEntry {
                path: "srv/index".into(),
                mode: 0o640,
                uid: 33,
                gid: 33
            },
        ]
    );
    let st = Store::open(&store).expect("store");
    assert!(
        boot.object
            .path_in(&st)
            .expect("in")
            .join("Image")
            .is_file()
    );
    let owners: Owners = serde_json::from_slice(
        &std::fs::read(owners.object.path_in(&st).expect("in")).expect("read"),
    )
    .expect("json");
    assert_eq!(owners.specials[0].path, "dev/console");

    for (bad, why) in [
        (
            tar_gz(&[("../escape", Regular, "x", 0o644, 0)]),
            "absolute path or `..`",
        ),
        (
            tar_gz(&[("/etc/passwd", Regular, "x", 0o644, 0)]),
            "absolute path or `..`",
        ),
        (
            tar_gz(&[
                ("link", Symlink, "/etc", 0o777, 0),
                ("link/passwd", Regular, "x", 0o644, 0),
            ]),
            "through a symlink",
        ),
    ] {
        let file = dir.path().join("bad.tar.gz");
        std::fs::write(&file, bad).expect("write");
        let error =
            os_import(&s, stored(&store, &file), "auto", "auto", &Ctx::new()).expect_err("refused");
        println!("{error}");
        assert!(error.to_string().contains(why), "{error}");
    }
}
