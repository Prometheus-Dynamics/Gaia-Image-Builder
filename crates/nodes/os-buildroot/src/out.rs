//! Where Buildroot's outputs go: one directory per artifact kind, moved (not
//! copied) out of Buildroot's output directory.

use std::fs;
use std::io;
use std::path::Path;

/// What a file in Buildroot's `images/` directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Kernel,
    DeviceTrees,
    Bootloader,
    Images,
}

impl Bucket {
    /// The directory the bucket's files are moved into.
    pub fn dir(self) -> &'static str {
        match self {
            Bucket::Kernel => "kernel",
            Bucket::DeviceTrees => "device-trees",
            Bucket::Bootloader => "bootloader",
            Bucket::Images => "images",
        }
    }
}

const BOOTLOADER_PREFIXES: [&str; 6] = ["u-boot", "idbloader", "MLO", "spl", "bl31", "tf-a"];
const KERNEL_PREFIXES: [&str; 5] = ["Image", "zImage", "bzImage", "uImage", "vmlinux"];

/// The bucket of a file named `name`. Device trees by extension; bootloader
/// names before kernel names, so `u-boot.itb` is a bootloader and any other
/// `.itb` (a FIT kernel image) is a kernel. Everything else, directories
/// included, is an image.
pub fn bucket(name: &str, is_file: bool) -> Bucket {
    if !is_file {
        return Bucket::Images;
    }
    if name.ends_with(".dtb") || name.ends_with(".dtbo") {
        Bucket::DeviceTrees
    } else if BOOTLOADER_PREFIXES.iter().any(|p| name.starts_with(p)) {
        Bucket::Bootloader
    } else if KERNEL_PREFIXES.iter().any(|p| name.starts_with(p)) || name.ends_with(".itb") {
        Bucket::Kernel
    } else {
        Bucket::Images
    }
}

/// Moves each entry of `images` into `out/<bucket>/`, creating the four
/// bucket directories even when they stay empty.
pub fn sort_images(images: &Path, out: &Path) -> io::Result<()> {
    for bucket in [
        Bucket::Kernel,
        Bucket::DeviceTrees,
        Bucket::Bootloader,
        Bucket::Images,
    ] {
        fs::create_dir_all(out.join(bucket.dir()))?;
    }
    if !images.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(images)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_file = entry.file_type()?.is_file();
        let target = out
            .join(bucket(&name, is_file).dir())
            .join(entry.file_name());
        fs::rename(entry.path(), target)?;
    }
    Ok(())
}

/// Copies `from` to `to` as a tree: directories, files with their modes, and
/// symlinks as links. `to` must not exist. A missing `from` gives an empty
/// `to`.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return fs::create_dir_all(to);
    }
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_go_to_the_bucket_their_name_says() {
        assert_eq!(bucket("Image", true), Bucket::Kernel);
        assert_eq!(bucket("zImage", true), Bucket::Kernel);
        assert_eq!(bucket("fit.itb", true), Bucket::Kernel);
        assert_eq!(bucket("board.dtb", true), Bucket::DeviceTrees);
        assert_eq!(bucket("board.dtbo", true), Bucket::DeviceTrees);
        assert_eq!(bucket("u-boot.itb", true), Bucket::Bootloader);
        assert_eq!(bucket("idbloader.img", true), Bucket::Bootloader);
        assert_eq!(bucket("rootfs.cpio.gz", true), Bucket::Images);
        assert_eq!(bucket("sdcard.img", true), Bucket::Images);
        assert_eq!(bucket("images-subdir", false), Bucket::Images);
    }

    #[test]
    fn sorting_moves_each_file_and_makes_every_bucket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let images = dir.path().join("images");
        let out = dir.path().join("out");
        fs::create_dir_all(images.join("sub")).expect("mkdir");
        for name in ["Image", "board.dtb", "u-boot.bin", "rootfs.cpio.gz"] {
            fs::write(images.join(name), name).expect("write");
        }
        sort_images(&images, &out).expect("sort");
        assert_eq!(fs::read(out.join("kernel/Image")).expect("read"), b"Image");
        assert!(out.join("device-trees/board.dtb").is_file());
        assert!(out.join("bootloader/u-boot.bin").is_file());
        assert!(out.join("images/rootfs.cpio.gz").is_file());
        assert!(out.join("images/sub").is_dir());
        assert!(!images.join("Image").exists());
    }

    #[test]
    fn a_missing_images_directory_still_gives_empty_buckets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("out");
        sort_images(&dir.path().join("nothing"), &out).expect("sort");
        assert!(out.join("kernel").is_dir());
        assert!(out.join("images").is_dir());
    }

    #[test]
    fn copy_tree_keeps_links_and_modes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = dir.path().join("from");
        fs::create_dir_all(from.join("bin")).expect("mkdir");
        fs::write(from.join("bin/tool"), "x").expect("write");
        fs::set_permissions(
            from.join("bin/tool"),
            std::os::unix::fs::PermissionsExt::from_mode(0o750),
        )
        .expect("chmod");
        std::os::unix::fs::symlink("tool", from.join("bin/alias")).expect("link");
        let to = dir.path().join("to");
        copy_tree(&from, &to).expect("copy");
        assert_eq!(
            fs::read_link(to.join("bin/alias")).expect("link"),
            Path::new("tool")
        );
        let mode = fs::metadata(to.join("bin/tool"))
            .expect("meta")
            .permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o750
        );
    }
}
