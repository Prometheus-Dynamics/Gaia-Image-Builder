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

/// The `.config` of `package`'s build directory (`build/<package>-<version>`),
/// when there is one. A directory of another package whose name starts the
/// same way (`linux-headers`, `linux-firmware`) is not it: the build
/// directory is the one with a `.config` and a `Makefile` or `Kbuild`.
pub fn package_config(build: &Path, package: &str) -> io::Result<Option<std::path::PathBuf>> {
    if !build.is_dir() {
        return Ok(None);
    }
    let prefix = format!("{package}-");
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    for entry in fs::read_dir(build)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        // `linux-headers-6.1`, `linux-firmware-...`: another package.
        if rest.starts_with(|c: char| c.is_ascii_alphabetic())
            && !rest.starts_with("custom")
            && !rest.starts_with('v')
        {
            continue;
        }
        let config = entry.path().join(".config");
        if config.is_file() {
            found.push(config);
        }
    }
    found.sort();
    Ok(found.into_iter().next())
}

/// The files Buildroot keeps at the top of its target tree for its own use,
/// which its filesystem images leave out (`fs/common.mk` excludes the
/// first, `TARGET_DIR_WARNING_FILE`): no part of a root filesystem.
pub const BOOKKEEPING: [&str; 1] = ["THIS_IS_NOT_YOUR_ROOT_FILESYSTEM"];

/// The names of [`BOOKKEEPING`] that `root` holds at its top.
pub fn bookkeeping_in(root: &Path) -> Vec<&'static str> {
    BOOKKEEPING
        .into_iter()
        .filter(|name| fs::symlink_metadata(root.join(name)).is_ok())
        .collect()
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

/// The kernel build directory Buildroot made in `build` (`linux-<version>`):
/// the one with a `.config` and a `Module.symvers`. `None` when the build has
/// no kernel.
pub fn kernel_build_source(build: &Path) -> io::Result<Option<std::path::PathBuf>> {
    if !build.is_dir() {
        return Ok(None);
    }
    let mut found = Vec::new();
    for entry in fs::read_dir(build)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if name.starts_with("linux-")
            && path.join(".config").is_file()
            && path.join("Module.symvers").is_file()
        {
            found.push(path);
        }
    }
    found.sort();
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err(io::Error::other(format!(
            "more than one kernel build directory in {}",
            build.display()
        ))),
    }
}

/// Copies a kernel build tree to `to`, keeping what an external module build
/// needs after `modules_prepare` and dropping what it does not: object and
/// dependency files, boot images, `vmlinux`, `System.map` and the
/// documentation. The source of the subsystems a module does not build
/// (drivers, filesystems, networking and so on) is dropped, as the distribution
/// `kernel-devel` packages do; their `Makefile`, `Kbuild` and `Kconfig` files
/// stay so the tree still reads as a kernel. Empty directories are not kept.
pub fn copy_kernel_build(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    copy_kept(from, to, "")
}

fn copy_kept(from: &Path, to: &Path, rel: &str) -> io::Result<()> {
    let dir = if rel.is_empty() {
        from.to_path_buf()
    } else {
        from.join(rel)
    };
    let mut entries = fs::read_dir(&dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let dest = to.join(&child);
            // A directory is made only when something in it is kept.
            if !prunes_dir(&child) {
                fs::create_dir(&dest)?;
                copy_kept(from, to, &child)?;
                if fs::read_dir(&dest)?.next().is_none() {
                    fs::remove_dir(&dest)?;
                }
            }
        } else if keep_file(&child, &name) {
            let src = from.join(&child);
            let dest = to.join(&child);
            if kind.is_symlink() {
                std::os::unix::fs::symlink(fs::read_link(&src)?, &dest)?;
            } else {
                fs::copy(&src, &dest)?;
            }
        }
    }
    Ok(())
}

/// Directories no kernel build tree keeps: boot images and documentation.
fn prunes_dir(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    parts[0] == "Documentation" || (parts[0] == "arch" && parts.get(2) == Some(&"boot"))
}

/// The files a kernel build tree keeps, by path (`rel`) and name.
fn keep_file(rel: &str, name: &str) -> bool {
    if is_object_leftover(name) {
        return false;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts[0] == "Documentation" {
        return build_file(name);
    }
    if parts[0] == "tools" && parts.get(1) != Some(&"objtool") {
        return build_file(name);
    }
    if parts[0] == "arch" && parts.get(2) == Some(&"boot") {
        return false;
    }
    const WHOLE: [&str; 5] = ["include", "scripts", "kernel", "lib", "tools/objtool"];
    if WHOLE
        .iter()
        .any(|root| rel == *root || rel.starts_with(&format!("{root}/")))
    {
        return true;
    }
    if parts[0] == "arch" && matches!(parts.get(2), Some(&"include" | &"kernel" | &"tools")) {
        return true;
    }
    build_file(name)
}

/// A file a build leaves behind, not one a module build reads.
fn is_object_leftover(name: &str) -> bool {
    const SUFFIXES: [&str; 10] = [
        ".o", ".a", ".ko", ".mod", ".mod.c", ".cmd", ".d", ".so", ".s", ".dwo",
    ];
    name.starts_with(".tmp")
        || name.starts_with("vmlinux")
        || name == "System.map"
        || SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// The build files of a kernel tree: makefiles, Kconfig, the config and the
/// module symbols and indexes.
fn build_file(name: &str) -> bool {
    name.starts_with("Makefile")
        || name.starts_with("Kbuild")
        || name.starts_with("Kconfig")
        || name.starts_with("localversion")
        || name.starts_with("modules.")
        || matches!(name, ".config" | ".version" | "Module.symvers")
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
    fn buildroots_own_files_are_found_at_the_top_of_a_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("etc")).expect("mkdir");
        assert!(bookkeeping_in(root).is_empty());
        fs::write(root.join("THIS_IS_NOT_YOUR_ROOT_FILESYSTEM"), "Warning!").expect("write");
        // A file of that name deeper down is a package's own business.
        fs::write(root.join("etc/THIS_IS_NOT_YOUR_ROOT_FILESYSTEM"), "x").expect("write");
        assert_eq!(bookkeeping_in(root), ["THIS_IS_NOT_YOUR_ROOT_FILESYSTEM"]);
    }

    #[test]
    fn a_kernel_build_keeps_what_a_module_build_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = dir.path().join("linux-1");
        let write = |rel: &str| {
            fs::create_dir_all(from.join(rel).parent().expect("parent")).expect("mkdir");
            fs::write(from.join(rel), rel).expect("write");
        };
        for kept in [
            ".config",
            "Makefile",
            "Module.symvers",
            "modules.order",
            "include/generated/utsrelease.h",
            "scripts/basic/fixdep",
            "arch/arm64/include/asm/x.h",
            "arch/arm64/kernel/asm-offsets.c",
            "arch/arm64/Makefile",
            "drivers/net/Makefile",
            "drivers/net/Kconfig",
            "tools/objtool/objtool",
        ] {
            write(kept);
        }
        for dropped in [
            "vmlinux",
            "System.map",
            "built-in.a",
            "kernel/sched/core.o",
            "kernel/sched/.core.o.cmd",
            "drivers/net/driver.c",
            "drivers/net/driver.o",
            "arch/arm64/boot/Image",
            "arch/arm64/kernel/asm-offsets.s",
            "Documentation/admin-guide/index.rst",
            "tools/perf/perf.c",
        ] {
            write(dropped);
        }
        let to = dir.path().join("kernel-build");
        copy_kernel_build(&from, &to).expect("copy");
        for kept in [
            "include/generated/utsrelease.h",
            "drivers/net/Makefile",
            "tools/objtool/objtool",
            "arch/arm64/kernel/asm-offsets.c",
        ] {
            assert!(to.join(kept).is_file(), "{kept} is kept");
        }
        for dropped in [
            "vmlinux",
            "built-in.a",
            "kernel/sched",
            "drivers/net/driver.c",
            "arch/arm64/boot",
            "Documentation",
            "tools/perf",
        ] {
            assert!(!to.join(dropped).exists(), "{dropped} is dropped");
        }
    }

    #[test]
    fn the_kernel_build_is_the_one_with_a_config_and_symbols() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("linux-headers-6")).expect("headers");
        assert_eq!(kernel_build_source(dir.path()).expect("scan"), None);
        let linux = dir.path().join("linux-6.18.7");
        fs::create_dir_all(&linux).expect("linux");
        fs::write(linux.join(".config"), "").expect("config");
        fs::write(linux.join("Module.symvers"), "").expect("symvers");
        assert_eq!(kernel_build_source(dir.path()).expect("scan"), Some(linux));
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
