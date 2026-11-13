//! The root filesystem's owners and modes.
//!
//! Buildroot's `target/` directory is written by the build's own user, with
//! plain modes. The owners and modes an image must have (`/etc/shadow` at
//! 0600, a setuid busybox, `/root` at 0700, a package's own users) are
//! applied only when Buildroot packs a filesystem image, under fakeroot,
//! from its device and permission tables. So the step has Buildroot pack a
//! tar as well, and reads each path's owner and mode from the tar's headers
//! into the tree's entries, which every filesystem node writes.
//!
//! Device nodes are in the tar too and are left out: the tree holds files,
//! directories and links, and a running system's `/dev` is devtmpfs.

use gaia_types::FileEntry;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The option that has Buildroot pack the tar.
pub const FRAGMENT: &str = "BR2_TARGET_ROOTFS_TAR=y\n";
/// The tar, in Buildroot's `images/`.
pub const TAR: &str = "rootfs.tar";

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
