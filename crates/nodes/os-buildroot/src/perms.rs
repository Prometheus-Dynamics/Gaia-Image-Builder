//! The root filesystem: what Buildroot's fakeroot step made.
//!
//! Buildroot's `target/` directory is written by the build's own user, with
//! plain modes, and is not the root filesystem: Buildroot copies it and,
//! under fakeroot, applies its device and permission tables, creates the
//! packages' users (mkusers), runs the post-fakeroot scripts and the
//! systemd presets, and only then packs a filesystem image. A file such a
//! script removes or changes is still as it was in `target/`. So the step
//! has Buildroot pack a tar as well and takes the root filesystem from it
//! ([`unpack`]): its files, directories and links as the tar has them, and
//! each path's owner and mode from the tar's headers into the tree's
//! entries ([`entries`]), which every filesystem node writes.
//!
//! Device nodes are in the tar too. The tree holds files, directories and
//! links, so [`unpack`] lists them and does not write them. Those under
//! `/dev` are left out, since a running system mounts devtmpfs over `/dev`;
//! one anywhere else fails the step by name ([`outside_dev`]).

use gaia_types::FileEntry;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The option that has Buildroot pack the tar.
pub const FRAGMENT: &str = "BR2_TARGET_ROOTFS_TAR=y\n";
/// The tar, in Buildroot's `images/`.
pub const TAR: &str = "rootfs.tar";

/// What [`unpack`] found besides files, directories and links: each device
/// node or fifo, as `<path> <kind> <major>:<minor>`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    pub devices: Vec<String>,
}

/// Why device nodes under `/dev` are left out of the root filesystem.
pub const DEVICES_LEFT_OUT: &str = "devtmpfs is mounted over /dev at boot";

/// The device nodes of `devices` (as [`Unpacked`] lists them) that are not
/// under `/dev`.
pub fn outside_dev(devices: &[String]) -> Vec<&str> {
    devices
        .iter()
        .map(String::as_str)
        .filter(|device| !device.starts_with("dev/"))
        .collect()
}

/// Writes the tar `tar` out as the new directory `into`: each file with its
/// contents and permission bits, each directory (its mode applied last, so
/// a read-only one is still filled), each symbolic and hard link. Device
/// nodes and fifos are listed, not written. A path that leaves `into`
/// (absolute, or with `..`) and an entry of another kind are errors.
pub fn unpack(tar: &Path, into: &Path) -> io::Result<Unpacked> {
    let invalid = |what: String| io::Error::new(io::ErrorKind::InvalidData, what);
    std::fs::create_dir(into)?;
    let mut archive = tar::Archive::new(io::BufReader::new(std::fs::File::open(tar)?));
    let mut unpacked = Unpacked::default();
    let mut dirs: Vec<(std::path::PathBuf, u32)> = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = relative(&entry.path()?).ok_or_else(|| {
            invalid(format!(
                "the root filesystem's tar names a path outside it: {}",
                String::from_utf8_lossy(&entry.path_bytes())
            ))
        })?;
        let path = into.join(&name);
        let header = entry.header();
        let kind = header.entry_type();
        // Only files and directories have a mode of their own.
        let mode = || header.mode().map(|mode| mode & 0o777);
        if name.as_os_str().is_empty() {
            dirs.push((path, mode()?));
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if kind.is_dir() {
            match std::fs::create_dir(&path) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
                _ => {}
            }
            dirs.push((path, mode()?));
        } else if kind.is_file() || kind == tar::EntryType::Continuous {
            let mode = mode()?;
            let mut file = std::fs::File::create(&path)?;
            io::copy(&mut entry, &mut file)?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        } else if kind.is_symlink() {
            let target = entry
                .link_name()?
                .ok_or_else(|| invalid(format!("a link with no target: {}", name.display())))?;
            std::os::unix::fs::symlink(target, &path)?;
        } else if kind.is_hard_link() {
            let target = entry
                .link_name()?
                .as_deref()
                .and_then(relative)
                .ok_or_else(|| {
                    invalid(format!(
                        "a hard link to outside the tar: {}",
                        name.display()
                    ))
                })?;
            std::fs::hard_link(into.join(target), &path)?;
        } else if kind.is_character_special() || kind.is_block_special() || kind.is_fifo() {
            let what = if kind.is_character_special() {
                "char"
            } else if kind.is_block_special() {
                "block"
            } else {
                "fifo"
            };
            let major = header.device_major()?.unwrap_or(0);
            let minor = header.device_minor()?.unwrap_or(0);
            unpacked
                .devices
                .push(format!("{} {what} {major}:{minor}", name.display()));
        } else {
            return Err(invalid(format!(
                "the root filesystem's tar has an entry of kind {kind:?}: {}",
                name.display()
            )));
        }
    }
    // Deepest first, so a directory made read-only is already filled.
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (dir, mode) in dirs {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
    }
    unpacked.devices.sort();
    Ok(unpacked)
}

/// `path` without its leading `./`, if it stays inside the tree: no root,
/// no `..`.
fn relative(path: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) => out.push(name),
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// An entry for each path of `tree` whose owner or mode in `tar` is not
/// root and the mode the tree stores, sorted by path.
pub fn entries(tar: &Path, tree: &Path) -> io::Result<Vec<FileEntry>> {
    let mut archive = tar::Archive::new(io::BufReader::new(std::fs::File::open(tar)?));
    let mut found = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let header = entry.header();
        let kind = header.entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
            continue;
        }
        let path = entry.path()?;
        let name = path
            .to_string_lossy()
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_string();
        if name.is_empty() || name == "." {
            continue;
        }
        let Ok(stored) = std::fs::symlink_metadata(tree.join(&name)) else {
            continue;
        };
        let (uid, gid) = (header.uid()? as u32, header.gid()? as u32);
        if stored.file_type().is_symlink() {
            // A link has no mode of its own; only its owner can differ.
            if uid != 0 || gid != 0 {
                found.push(FileEntry {
                    path: name,
                    mode: 0o777,
                    uid,
                    gid,
                });
            }
            continue;
        }
        let mode = header.mode()? & 0o7777;
        let same = uid == 0 && gid == 0 && mode == stored.permissions().mode() & 0o7777;
        // A set-id or sticky bit is always listed: a store need not keep it.
        if !same || mode & 0o7000 != 0 {
            found.push(FileEntry {
                path: name,
                mode,
                uid,
                gid,
            });
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(
        builder: &mut tar::Builder<Vec<u8>>,
        path: &str,
        kind: tar::EntryType,
        mode: u32,
        id: u64,
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_uid(id);
        header.set_gid(id);
        header.set_size(0);
        if kind.is_symlink() {
            header.set_link_name("busybox").expect("link");
        }
        builder
            .append_data(&mut header, path, io::empty())
            .expect("append");
    }

    fn add_file(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, text: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_size(text.len() as u64);
        builder
            .append_data(&mut header, path, text.as_bytes())
            .expect("append");
    }

    /// The tree unpacked is the tar's, not the target's: a file a
    /// post-fakeroot script removed is gone, one it changed has the tar's
    /// contents, one it added is there; links and modes are kept, and
    /// device nodes are listed, not written.
    #[test]
    fn the_root_filesystem_is_the_tars_after_fakeroot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut builder = tar::Builder::new(Vec::new());
        use tar::EntryType::{Block, Char, Directory, Fifo, Link, Symlink};
        add(&mut builder, "./", Directory, 0o755, 0);
        add(&mut builder, "./bin/", Directory, 0o755, 0);
        add_file(&mut builder, "./bin/busybox", 0o4755, "busybox");
        add(&mut builder, "./bin/sh", Symlink, 0o777, 0);
        add(&mut builder, "./etc/", Directory, 0o755, 0);
        add_file(
            &mut builder,
            "./etc/passwd",
            0o644,
            "root:x:0:0::/root:/bin/sh\n",
        );
        add_file(&mut builder, "./etc/added-by-script", 0o600, "new");
        add(&mut builder, "./usr/share/ro/", Directory, 0o555, 0);
        add_file(&mut builder, "./usr/share/ro/file", 0o444, "ro");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(Link);
        header.set_mode(0o755);
        header.set_uid(0);
        header.set_gid(0);
        header.set_size(0);
        header.set_link_name("./bin/busybox").expect("link");
        builder
            .append_data(&mut header, "./bin/hard", io::empty())
            .expect("append");
        for (path, kind, major, minor) in [
            ("./dev/console", Char, 5, 1),
            ("./dev/mmcblk0", Block, 179, 0),
            ("./run/fifo", Fifo, 0, 0),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(0o600);
            header.set_uid(0);
            header.set_gid(0);
            header.set_size(0);
            header.set_device_major(major).expect("major");
            header.set_device_minor(minor).expect("minor");
            builder
                .append_data(&mut header, path, io::empty())
                .expect("append");
        }
        let tar_path = dir.path().join("rootfs.tar");
        std::fs::write(&tar_path, builder.into_inner().expect("tar")).expect("write");

        let tree = dir.path().join("rootfs");
        let unpacked = unpack(&tar_path, &tree).expect("unpack");
        assert_eq!(
            unpacked.devices,
            [
                "dev/console char 5:1",
                "dev/mmcblk0 block 179:0",
                "run/fifo fifo 0:0"
            ]
        );
        assert!(
            !tree.join("dev/console").exists(),
            "a device node was written"
        );
        assert_eq!(
            std::fs::read_to_string(tree.join("etc/passwd")).expect("passwd"),
            "root:x:0:0::/root:/bin/sh\n"
        );
        assert_eq!(
            std::fs::read_to_string(tree.join("etc/added-by-script")).expect("added"),
            "new"
        );
        let mode = |path: &str| {
            std::fs::symlink_metadata(tree.join(path))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("etc/added-by-script"), 0o600);
        assert_eq!(
            mode("bin/busybox"),
            0o755,
            "set-id bits are the entries' to say"
        );
        assert_eq!(mode("usr/share/ro"), 0o555);
        assert_eq!(mode("usr/share/ro/file"), 0o444);
        assert_eq!(
            std::fs::read_link(tree.join("bin/sh")).expect("link"),
            Path::new("busybox")
        );
        let ino = |path: &str| {
            std::os::unix::fs::MetadataExt::ino(
                &std::fs::metadata(tree.join(path)).expect("metadata"),
            )
        };
        assert_eq!(
            ino("bin/hard"),
            ino("bin/busybox"),
            "the hard link is one file"
        );
        // The entries are read against the unpacked tree, as before.
        let found = entries(&tar_path, &tree).expect("entries");
        assert!(
            found
                .iter()
                .any(|e| e.path == "bin/busybox" && e.mode == 0o4755)
        );
        assert!(!found.iter().any(|e| e.path == "etc/added-by-script"));
        std::fs::set_permissions(
            tree.join("usr/share/ro"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
    }

    /// Device nodes under /dev are left out; any other is named.
    #[test]
    fn device_nodes_outside_dev_are_named() {
        let devices = [
            "dev/console char 5:1".to_string(),
            "dev/null char 1:3".to_string(),
            "devices/x char 1:3".to_string(),
            "etc/gaia-node char 1:3".to_string(),
            "run/fifo fifo 0:0".to_string(),
        ];
        assert_eq!(
            outside_dev(&devices),
            [
                "devices/x char 1:3",
                "etc/gaia-node char 1:3",
                "run/fifo fifo 0:0"
            ]
        );
        assert!(outside_dev(&devices[..2]).is_empty());
    }

    /// A tar path that leaves the tree is refused, and nothing is written
    /// outside it.
    #[test]
    fn a_path_out_of_the_tree_is_refused() {
        for bad in ["../escape", "/etc/escape", "./a/../../escape"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut builder = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(1);
            // `append_data` refuses such paths itself; the header is set by hand.
            let name = header.as_old_mut().name.as_mut();
            name[..bad.len()].copy_from_slice(bad.as_bytes());
            header.set_cksum();
            builder.append(&header, &b"x"[..]).expect("append");
            let tar_path = dir.path().join("rootfs.tar");
            std::fs::write(&tar_path, builder.into_inner().expect("tar")).expect("write");
            let error = unpack(&tar_path, &dir.path().join("rootfs")).expect_err(bad);
            assert!(error.to_string().contains("outside"), "{bad}: {error}");
            assert!(!dir.path().join("escape").exists(), "{bad}");
        }
    }

    #[test]
    fn only_paths_that_differ_from_root_and_the_stored_mode_are_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tree = dir.path().join("target");
        for sub in ["bin", "etc", "root", "var/lib/app"] {
            std::fs::create_dir_all(tree.join(sub)).expect("mkdir");
        }
        for file in [
            "bin/busybox",
            "etc/shadow",
            "etc/hostname",
            "var/lib/app/db",
        ] {
            std::fs::write(tree.join(file), "x").expect("write");
            std::fs::set_permissions(tree.join(file), std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
        }
        for sub in ["bin", "etc", "root", "var", "var/lib", "var/lib/app"] {
            std::fs::set_permissions(tree.join(sub), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        std::os::unix::fs::symlink("busybox", tree.join("bin/sh")).expect("link");

        let mut builder = tar::Builder::new(Vec::new());
        use tar::EntryType::{Char, Directory, Regular, Symlink};
        add(&mut builder, "./", Directory, 0o755, 0);
        add(&mut builder, "./bin/", Directory, 0o755, 0);
        add(&mut builder, "./bin/busybox", Regular, 0o4755, 0);
        add(&mut builder, "./bin/sh", Symlink, 0o777, 0);
        add(&mut builder, "./etc/", Directory, 0o755, 0);
        add(&mut builder, "./etc/shadow", Regular, 0o600, 0);
        add(&mut builder, "./etc/hostname", Regular, 0o644, 0);
        add(&mut builder, "./root/", Directory, 0o700, 0);
        add(&mut builder, "./var/lib/app/", Directory, 0o755, 100);
        add(&mut builder, "./var/lib/app/db", Regular, 0o644, 100);
        add(&mut builder, "./dev/console", Char, 0o622, 0);
        add(&mut builder, "./not/in/the/tree", Regular, 0o600, 0);
        let tar_path = dir.path().join("rootfs.tar");
        std::fs::write(&tar_path, builder.into_inner().expect("tar")).expect("write");

        let found = entries(&tar_path, &tree).expect("entries");
        let shown: Vec<String> = found
            .iter()
            .map(|e| format!("{} {:o} {} {}", e.path, e.mode, e.uid, e.gid))
            .collect();
        println!("{}", shown.join("\n"));
        assert_eq!(
            shown,
            [
                "bin/busybox 4755 0 0",
                "etc/shadow 600 0 0",
                "root 700 0 0",
                "var/lib/app 755 100 100",
                "var/lib/app/db 644 100 100",
            ]
        );
    }
}
