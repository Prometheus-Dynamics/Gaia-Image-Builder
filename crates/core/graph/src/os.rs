//! An image's OS config: what the board gives, by rule, then the image's
//! own fragments.
//!
//! The rules read the board's fields and name Buildroot's own options for
//! them. Nothing here knows a board, an SoC or a vendor: a value that only
//! one board needs is in that board's file or the image's fragments.
//!
//! - **Target:** the architecture's option, and `BR2_<cpu>` for the CPU
//!   (`cortex-a55` is `BR2_cortex_a55`; a big.LITTLE pair
//!   `cortex-a76.cortex-a55` is `BR2_cortex_a76_a55`).
//! - **Kernel:** the locked source as Buildroot's custom git repository or
//!   tarball, built from Gaia's stored copy (`LINUX_OVERRIDE_SRCDIR`), so
//!   Buildroot downloads nothing for it; the defconfig (`defconfig` is the
//!   architecture's default), the fragment files, the in-tree device tree,
//!   the board's device tree directory, and the overlays by name (built by
//!   the kernel build; they come out with the device trees). The board's
//!   extra in-tree device trees follow its own in the same option, which
//!   Buildroot reads as a list whose first name is the one booted.
//!   `toolchain = "llvm"` is `BR2_LINUX_KERNEL_LLVM` (the kernel built
//!   with `LLVM=1`, the pinned LLVM and Rust; an option of Gaia's Buildroot
//!   patch set).
//! - **Bootloader:** the same for U-Boot (`UBOOT`), its defconfig and
//!   fragment files; each blob is given to U-Boot's make as
//!   `<NAME>=<path>`, so a blob's name in the board file is the make
//!   variable U-Boot reads; trusted firmware (`ARM_TRUSTED_FIRMWARE`) and
//!   its platform, with U-Boot built with its BL31.
//!
//! The config is then divided into up to three builds, each keyed on its own
//! part, so a change to one does not rebuild the others ([`split`]):
//!
//! - **kernel**: the board's kernel fragment and the image's
//!   `BR2_LINUX_KERNEL...` lines.
//! - **bootloader**: the board's bootloader fragment and the image's
//!   `BR2_TARGET_...` lines that are a bootloader's (every `BR2_TARGET_`
//!   option but the system's own: `GENERIC`, `ROOTFS`, root login, time
//!   zone, optimisation).
//! - **root filesystem**: everything else.
//!
//! The target fragment and the image's toolchain and build options go to
//! all three; `BR2_PACKAGE_HOST_...` to the kernel and the bootloader, whose
//! builds are what need host tools. A board with no kernel, or no
//! bootloader, has no such build, and those lines stay in the root
//! filesystem's.
//!
//! A file or directory the config names is an input tree of the step,
//! written `@name@` in the text: a locked source by its lock name, a
//! file or a directory beside a description as `files.<n>` (a file as a
//! tree with that file alone, so a build is keyed on what it names and not
//! on its neighbours). In an image's own fragment, `@here@/<path>` is that
//! file or directory beside the fragment, and a bare `@here@` the
//! fragment's whole directory.

use crate::Error;
use gaia_describe::{Board, FileRef, OsSpec, RelPath, Source};
use gaia_lock::{Entry, Lock};
use gaia_types::{Arch, ConfigFragment, OsConfig, SourceOverride};
use std::fs;
use std::path::{Path, PathBuf};

/// Where an input tree comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputFrom {
    /// A source with patch series applied (`gaia.source.patch`): the source
    /// by its input name, and each patch tree by a name of its own with the
    /// path in it to apply (empty: the whole tree).
    Patched {
        base: Box<(String, InputFrom)>,
        patches: Vec<(String, InputFrom, String)>,
    },
    /// A locked source, written in `file` under `sources.<name>`.
    Source {
        file: PathBuf,
        name: String,
        source: Source,
    },
    /// A directory beside a description.
    Dir(PathBuf),
    /// One file beside a description, as a tree with that file alone: the
    /// step that reads it does not depend on what else is in its directory
    /// (a README, a lock, another build's fragment).
    File(PathBuf),
    /// One file of a component's source, as a tree with that file alone:
    /// the step that reads it does not depend on the rest of the source.
    Picked {
        /// The component's file, where the source is written.
        file: PathBuf,
        /// The source's name in that file.
        name: String,
        source: Source,
        /// The source's name in the lock.
        lock_name: String,
        /// The file in the source.
        path: String,
    },
}

/// The config and its input trees, in port order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OsPlan {
    pub config: OsConfig,
    pub inputs: Vec<(String, InputFrom)>,
}

impl OsPlan {
    /// Adds a contract the kernel's final config must meet: `from` is the
    /// tree that holds its file, under the input name of the contract.
    pub(crate) fn require(&mut self, contract: gaia_types::KernelContract, from: InputFrom) {
        self.input(contract.input.clone(), from);
        self.config.kernel_contracts.push(contract);
    }

    fn input(&mut self, name: String, from: InputFrom) -> String {
        if !self.inputs.iter().any(|(n, _)| *n == name) {
            self.inputs.push((name.clone(), from));
            self.config.inputs.push(name.clone());
        }
        name
    }

    /// A new `files.<n>`, by first use.
    fn next_files(&mut self, from: InputFrom) -> String {
        if let Some((name, _)) = self.inputs.iter().find(|(_, have)| *have == from) {
            return name.clone();
        }
        let n = self
            .inputs
            .iter()
            .filter(|(_, from)| matches!(from, InputFrom::Dir(_) | InputFrom::File(_)))
            .count();
        self.input(format!("files.{n}"), from)
    }

    /// A whole directory as an input, `files.<n>` by first use.
    fn dir(&mut self, dir: &Path) -> String {
        // The directory of a file named with no directory is this one: an
        // empty path is no directory to a step.
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        self.next_files(InputFrom::Dir(dir.to_path_buf()))
    }

    /// A file beside a description, as `@files.<n>@/<name>`: an input with
    /// that file alone.
    fn file(&mut self, path: &Path) -> String {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let name = self.next_files(InputFrom::File(path.to_path_buf()));
        format!("@{name}@/{file}")
    }

    /// `text` with each `@here@` (the directory `dir` of the file the text
    /// is from) as an input: `@here@/<path>` is that file, or that
    /// directory, alone; `@here@` by itself, or before a path that is not
    /// there, is the whole of `dir`.
    fn here(&mut self, text: &str, dir: &Path) -> String {
        const HERE: &str = "@here@";
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find(HERE) {
            out.push_str(&rest[..at]);
            rest = &rest[at + HERE.len()..];
            let path: &str = match rest.strip_prefix('/') {
                Some(after) => {
                    let end = after
                        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                        .unwrap_or(after.len());
                    &after[..end]
                }
                None => "",
            };
            let named = dir.join(path);
            let plain = !path.is_empty()
                && path
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..");
            if plain && named.is_file() {
                out.push_str(&self.file(&named));
                rest = &rest[1 + path.len()..];
            } else if plain && named.is_dir() {
                out.push_str(&format!("@{}@", self.dir(&named)));
                rest = &rest[1 + path.len()..];
            } else {
                out.push_str(&format!("@{}@", self.dir(dir)));
            }
        }
        out.push_str(rest);
        out
    }

    fn override_with(&mut self, package: &str, input: &str) {
        self.config.overrides.push(SourceOverride {
            package: package.into(),
            input: input.into(),
        });
    }
}

/// The OS plan for one image on `board`.
pub(crate) fn plan(
    board: &Board,
    index: usize,
    os: &OsSpec,
    product_file: &Path,
    lock: &Lock,
) -> Result<OsPlan, Error> {
    let mut plan = OsPlan {
        config: OsConfig::default(),
        inputs: Vec::new(),
    };
    let mut fragments = defaults();
    fragments.push(ConfigFragment {
        name: "board.target".into(),
        text: target(board)?,
    });
    if let Some(kernel) = &board.kernel {
        let mut lines = vec!["BR2_LINUX_KERNEL=y".to_string()];
        if let Some(name) = &kernel.source {
            lines.extend(locked_source(
                &mut plan,
                board,
                name,
                "kernel.source",
                kernel.patches.as_deref(),
                lock,
                "BR2_LINUX_KERNEL",
                "LINUX",
            )?);
        }
        if kernel.source.is_none() && kernel.patches.as_ref().is_some_and(|p| !p.is_empty()) {
            return Err(Error::at(
                &board.file,
                "kernel.patches",
                "patches are applied to the board's own kernel source, and it names none (`source`)",
            ));
        }
        if kernel.config.is_none() && kernel.defconfig.is_none() {
            return Err(Error::at(
                &board.file,
                "kernel",
                format!(
                    "the board {} states no configuration for its kernel to start from: `defconfig` (an in-tree one, or \"defconfig\" for the architecture's own) or `config` (a complete one); Buildroot stops without it",
                    board.name
                ),
            ));
        }
        if let (Some(_), Some(_)) = (&kernel.config, &kernel.defconfig) {
            return Err(Error::at(
                &board.file,
                "kernel.config",
                "a complete configuration and a `defconfig` are two starting points; the board (with its family) states one",
            ));
        }
        if let Some(config) = &kernel.config {
            lines.push("BR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y".into());
            lines.push(format!(
                "BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE={}",
                quoted(&plan.file(config.path()))
            ));
        }
        match kernel.defconfig.as_deref() {
            None => {}
            Some("defconfig") => lines.push("BR2_LINUX_KERNEL_USE_ARCH_DEFAULT_CONFIG=y".into()),
            Some(name) => {
                lines.push("BR2_LINUX_KERNEL_USE_DEFCONFIG=y".into());
                lines.push(format!(
                    "BR2_LINUX_KERNEL_DEFCONFIG={}",
                    quoted(name.strip_suffix("_defconfig").unwrap_or(name))
                ));
            }
        }
        if let Some(files) = &kernel.fragments {
            lines.push(format!(
                "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES={}",
                quoted(&files_list(&mut plan, files))
            ));
        }
        if kernel.device_tree.is_some() || kernel.device_tree_dir.is_some() {
            lines.push("BR2_LINUX_KERNEL_DTS_SUPPORT=y".into());
        }
        // The image form the board's firmware loads, when the board says.
        if let Some(image) = &kernel.image
            && let Some((_, option)) = gaia_describe::KERNEL_IMAGES
                .iter()
                .find(|(name, _)| name == image)
        {
            lines.push(format!("{option}=y"));
        }
        // The kernel built with LLVM and Rust (Gaia's Buildroot patch set
        // adds the option, the pinned LLVM and the Rust tools). Buildroot
        // refusing the line fails the build: a line it did not take is an
        // error, so no board asks for LLVM and gets gcc.
        if kernel.toolchain.as_deref() == Some("llvm") {
            lines.push("BR2_LINUX_KERNEL_LLVM=y".into());
        }
        let extra = kernel
            .extra_device_trees
            .as_deref()
            .filter(|names| !names.is_empty());
        if let Some(names) = extra {
            if kernel.device_tree.is_none() {
                return Err(Error::at(
                    &board.file,
                    "kernel.extra_device_trees",
                    "extra device trees are built beside the board's own: the board names no `device_tree`",
                ));
            }
            for (n, name) in names.iter().enumerate() {
                if !device_tree_name(name) {
                    return Err(Error::at(
                        &board.file,
                        format!("kernel.extra_device_trees[{n}]"),
                        format!(
                            "`{name}` is not a device tree's name: a path under the kernel's device tree directory, with no extension"
                        ),
                    ));
                }
            }
        }
        if let Some(dt) = &kernel.device_tree {
            // The board's own first: Buildroot takes the first name as the
            // one booted.
            let mut all = vec![dt.as_str()];
            all.extend(extra.unwrap_or_default().iter().map(String::as_str));
            lines.push(format!(
                "BR2_LINUX_KERNEL_INTREE_DTS_NAME={}",
                quoted(&all.join(" "))
            ));
        }
        if let Some(dir) = &kernel.device_tree_dir {
            let name = plan.dir(dir.path());
            lines.push(format!(
                "BR2_LINUX_KERNEL_CUSTOM_DTS_DIR={}",
                quoted(&format!("@{name}@"))
            ));
        }
        // Overlays the kernel build makes, by the names it takes (a path
        // under its device tree directory, no extension). A board's own
        // overlay source sits in its device tree directory and is named here
        // like an in-tree one.
        if let Some(overlays) = kernel.overlays.as_ref().filter(|o| !o.is_empty()) {
            for (n, name) in overlays.iter().enumerate() {
                if !device_tree_name(name) {
                    return Err(Error::at(
                        &board.file,
                        format!("kernel.overlays[{n}]"),
                        format!(
                            "`{name}` is not an overlay's name: a path under the kernel's device tree directory, with no extension"
                        ),
                    ));
                }
            }
            if !lines.iter().any(|l| l == "BR2_LINUX_KERNEL_DTS_SUPPORT=y") {
                lines.push("BR2_LINUX_KERNEL_DTS_SUPPORT=y".into());
            }
            lines.push(format!(
                "BR2_LINUX_KERNEL_INTREE_DTSO_NAMES={}",
                quoted(&overlays.join(" "))
            ));
        }
        fragments.push(ConfigFragment {
            name: "board.kernel".into(),
            text: text(&lines),
        });
    }
    if let Some(boot) = &board.bootloader {
        let tf = boot.trusted_firmware.as_ref();
        for (key, source, patches) in [
            (
                "bootloader.patches",
                boot.source.as_ref(),
                boot.patches.as_ref(),
            ),
            (
                "bootloader.trusted_firmware.patches",
                tf.and_then(|t| t.source.as_ref()),
                tf.and_then(|t| t.patches.as_ref()),
            ),
        ] {
            if source.is_none() && patches.is_some_and(|p| !p.is_empty()) {
                return Err(Error::at(
                    &board.file,
                    key,
                    "patches are applied to the board's own source here, and it names none (`source`)",
                ));
            }
        }
        let mut lines = vec![
            "BR2_TARGET_UBOOT=y".to_string(),
            "BR2_TARGET_UBOOT_BUILD_SYSTEM_KCONFIG=y".to_string(),
        ];
        if let Some(name) = &boot.source {
            lines.extend(locked_source(
                &mut plan,
                board,
                name,
                "bootloader.source",
                boot.patches.as_deref(),
                lock,
                "BR2_TARGET_UBOOT",
                "UBOOT",
            )?);
        }
        let Some(name) = &boot.defconfig else {
            return Err(Error::at(
                &board.file,
                "bootloader.defconfig",
                format!(
                    "the board {} names no U-Boot defconfig: it carries the board's pin and voltage setup, so the board states its own (a family gives none)",
                    board.name
                ),
            ));
        };
        lines.push(format!(
            "BR2_TARGET_UBOOT_BOARD_DEFCONFIG={}",
            quoted(name.strip_suffix("_defconfig").unwrap_or(name))
        ));
        if let Some(files) = &boot.fragments {
            lines.push(format!(
                "BR2_TARGET_UBOOT_CONFIG_FRAGMENT_FILES={}",
                quoted(&files_list(&mut plan, files))
            ));
        }
        let mut opts = Vec::new();
        for (name, file) in &boot.blobs {
            let valid = !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if !valid {
                return Err(Error::at(
                    &board.file,
                    format!("bootloader.blobs.{name}"),
                    format!(
                        "a blob is given to U-Boot's make by its name, and `{name}` is not a make variable: capitals, digits and _"
                    ),
                ));
            }
            let path = match file {
                FileRef::InSource { source, path } => {
                    let input = board_source_input(
                        &mut plan,
                        board,
                        source,
                        &format!("bootloader.blobs.{name}"),
                    )?;
                    format!("@{input}@/{path}")
                }
                FileRef::Local { file } => plan.file(file.path()),
                FileRef::Input { .. } | FileRef::Description { .. } | FileRef::Generated { .. } => {
                    return Err(Error::at(
                        &board.file,
                        format!("bootloader.blobs.{name}"),
                        "a named input, a board description or a generated file is only for a component's install entry",
                    ));
                }
            };
            opts.push(format!("{name}={path}"));
        }
        if !opts.is_empty() {
            lines.push(format!(
                "BR2_TARGET_UBOOT_CUSTOM_MAKEOPTS={}",
                quoted(&opts.join(" "))
            ));
        }
        if let Some(tf) = &boot.trusted_firmware {
            lines.push("BR2_TARGET_ARM_TRUSTED_FIRMWARE=y".into());
            if let Some(name) = &tf.source {
                lines.extend(locked_source(
                    &mut plan,
                    board,
                    name,
                    "bootloader.trusted_firmware.source",
                    tf.patches.as_deref(),
                    lock,
                    "BR2_TARGET_ARM_TRUSTED_FIRMWARE",
                    "ARM_TRUSTED_FIRMWARE",
                )?);
            }
            if let Some(platform) = &tf.platform {
                lines.push(format!(
                    "BR2_TARGET_ARM_TRUSTED_FIRMWARE_PLATFORM={}",
                    quoted(platform)
                ));
            }
            lines.push("BR2_TARGET_UBOOT_NEEDS_ATF_BL31=y".into());
        }
        fragments.push(ConfigFragment {
            name: "board.bootloader".into(),
            text: text(&lines),
        });
    }
    // Before the descriptions, so a description can turn a default off.
    fragments.push(parallel_defaults());
    for (j, file) in os.fragments.iter().enumerate() {
        let key = format!("images[{index}].os.fragments[{j}]");
        let body = fs::read_to_string(file.path()).map_err(|e| {
            Error::at(
                product_file,
                key.clone(),
                format!("{}: {e}", file.path().display()),
            )
        })?;
        let body = if body.contains("@here@") {
            let dir = file.path().parent().unwrap_or(Path::new("/"));
            plan.here(&body, dir)
        } else {
            body
        };
        let shown = file
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        fragments.push(ConfigFragment {
            name: format!("image.{j}.{shown}"),
            text: body,
        });
    }
    plan.config.fragments = fragments;
    plan.config
        .check()
        .map_err(|e| Error::at(product_file, format!("images[{index}].os"), e))?;
    Ok(plan)
}

/// The default that makes Buildroot build with per-package directories, so
/// its packages build at once under `make -j`. A description turns it off
/// with `# BR2_PER_PACKAGE_DIRECTORIES is not set`, which comes later and
/// wins. Its own fragment, so another default fragment merges beside it.
fn parallel_defaults() -> ConfigFragment {
    ConfigFragment {
        name: "gaia.parallel".into(),
        text: "BR2_PER_PACKAGE_DIRECTORIES=y\n".into(),
    }
}

/// The fragments every OS build starts from, before the board's and the
/// image's own: none. Buildroot's compiler cache is not among them: in a
/// build from nothing every compile misses, each configure probe costs about
/// twice as much through the cache (measured: 100 small compiles 1.3 s plain,
/// 2.6 s through a cold cache), and the cache itself is five host packages.
/// A build that reruns a step on nearly the same sources turns it on with
/// `BR2_CCACHE=y` in its fragment; the cache's directory is then the store's
/// (given to make by the host, so no key holds it).
fn defaults() -> Vec<ConfigFragment> {
    Vec::new()
}

/// Whether `name` names a device tree or overlay the kernel build makes: a
/// relative path under its device tree directory, of plain characters, with
/// no extension.
fn device_tree_name(name: &str) -> bool {
    let plain = !name.is_empty()
        && !name.starts_with('/')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b'.' | b','));
    plain
        && !name.split('/').any(|part| part.is_empty() || part == "..")
        && ![".dtbo", ".dtb", ".dts", ".dtso"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

/// The architecture's and the CPU's options.
fn target(board: &Board) -> Result<String, Error> {
    let arch = match &board.target.arch {
        Arch::AArch64 => vec!["BR2_aarch64=y"],
        Arch::X86_64 => vec!["BR2_x86_64=y"],
        Arch::Arm => vec!["BR2_arm=y"],
        Arch::RiscV64 => vec!["BR2_riscv=y", "BR2_RISCV_64=y"],
        Arch::RiscV32 => vec!["BR2_riscv=y", "BR2_RISCV_32=y"],
        other => {
            return Err(Error::at(
                &board.file,
                "target.arch",
                format!("no Buildroot architecture is known for {}", other.as_str()),
            ));
        }
    };
    let mut lines: Vec<String> = arch.into_iter().map(str::to_string).collect();
    if let Some(cpu) = &board.target.cpu {
        // A big.LITTLE pair is written as the compiler takes it,
        // `cortex-a76.cortex-a55`; Buildroot's option names the second core
        // by its number alone, `BR2_cortex_a76_a55`.
        let symbol = match cpu.split_once('.') {
            Some((big, little)) => {
                let family = big.split_once('-').map_or("", |(family, _)| family);
                let little = little
                    .strip_prefix(family)
                    .and_then(|rest| rest.strip_prefix('-'))
                    .unwrap_or(little);
                format!("{big}_{little}")
            }
            None => cpu.clone(),
        }
        .replace(['-', '.'], "_");
        if !symbol
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::at(
                &board.file,
                "target.cpu",
                format!("`{cpu}` is not a CPU name Buildroot has an option for"),
            ));
        }
        lines.push(format!("BR2_{symbol}=y"));
    }
    Ok(text(&lines))
}

/// A board source as an input, by its lock name.
fn board_source_input(
    plan: &mut OsPlan,
    board: &Board,
    name: &str,
    key: &str,
) -> Result<String, Error> {
    let (input, from) = board_source(board, name, key)?;
    Ok(plan.input(input, from))
}

/// The board's source `name` as an input: its name and where it is from.
fn board_source(board: &Board, name: &str, key: &str) -> Result<(String, InputFrom), Error> {
    let source = board
        .sources
        .get(name)
        .ok_or_else(|| Error::at(&board.file, key, format!("no source named `{name}`")))?;
    Ok((
        format!("{}.{name}", board.name),
        InputFrom::Source {
            file: board.file.clone(),
            name: name.to_string(),
            source: source.clone(),
        },
    ))
}

/// A package's source: Buildroot's custom git or tarball options with the
/// locked commit or URL, so the config says what it is built from, and an
/// override to the stored copy, so Buildroot fetches nothing for it.
#[allow(clippy::too_many_arguments)]
fn locked_source(
    plan: &mut OsPlan,
    board: &Board,
    name: &str,
    key: &str,
    patches: Option<&[FileRef]>,
    lock: &Lock,
    prefix: &str,
    package: &str,
) -> Result<Vec<String>, Error> {
    let patches = patches.unwrap_or_default();
    let (input, from) = board_source(board, name, key)?;
    // With patches, the build takes the source as they leave it: a tree of
    // its own, under a name of its own; the lock still names the source.
    let input_used = if patches.is_empty() {
        plan.input(input.clone(), from)
    } else {
        let field = key.strip_suffix(".source").unwrap_or(key);
        let mut trees = Vec::new();
        for (n, patch) in patches.iter().enumerate() {
            let at = format!("{field}.patches[{n}]");
            let (tree_name, tree, path) = match patch {
                FileRef::Local { file } => {
                    let path = file.path();
                    if path.is_dir() {
                        (
                            format!("{input}.patch{n}"),
                            InputFrom::Dir(path.to_path_buf()),
                            String::new(),
                        )
                    } else {
                        (
                            format!("{input}.patch{n}"),
                            InputFrom::File(path.to_path_buf()),
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        )
                    }
                }
                FileRef::InSource { source, path } => {
                    let (tree_name, tree) = board_source(board, source, &format!("{at}.source"))?;
                    if lock.get(&tree_name).is_none() {
                        return Err(Error::at(
                            &board.file,
                            format!("sources.{source}"),
                            format!("{tree_name} is not in the lock; lock it first"),
                        ));
                    }
                    (tree_name, tree, path.clone())
                }
                _ => {
                    return Err(Error::at(
                        &board.file,
                        at,
                        "a patch is a file beside the board (`file`) or in one of its sources (`source` and `path`)",
                    ));
                }
            };
            trees.push((tree_name, tree, path));
        }
        plan.input(
            format!("{input}.patched"),
            InputFrom::Patched {
                base: Box::new((input.clone(), from)),
                patches: trees,
            },
        )
    };
    let lines = match lock.get(&input) {
        Some(Entry::Git { url, commit, .. }) => vec![
            format!("{prefix}_CUSTOM_GIT=y"),
            format!("{prefix}_CUSTOM_REPO_URL={}", quoted(url)),
            format!("{prefix}_CUSTOM_REPO_VERSION={}", quoted(commit)),
        ],
        Some(Entry::Download { url, .. }) => vec![
            format!("{prefix}_CUSTOM_TARBALL=y"),
            format!("{prefix}_CUSTOM_TARBALL_LOCATION={}", quoted(url)),
        ],
        Some(Entry::Tree { .. }) => Vec::new(),
        Some(Entry::Image { .. }) => {
            return Err(Error::at(
                &board.file,
                key,
                format!("`{name}` is a container image, not a source tree"),
            ));
        }
        None => {
            return Err(Error::at(
                &board.file,
                format!("sources.{name}"),
                format!("{input} is not in the lock; lock it first"),
            ));
        }
    };
    plan.override_with(package, &input_used);
    Ok(lines)
}

fn files_list(plan: &mut OsPlan, files: &[RelPath]) -> String {
    files
        .iter()
        .map(|f| plan.file(f.path()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A Kconfig string value.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn text(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// What the kernel's and the bootloader's builds start from: no init system,
/// no `/bin/sh` and no BusyBox. Buildroot's allnoconfig picks BusyBox init,
/// which builds busybox, initscripts and the SysV skeleton into a root those
/// builds never use (measured: busybox 272 s of package time in a kernel
/// build). The root's own build is not given this; its init lines are its
/// own (an init line always goes to the root part).
fn no_root() -> ConfigFragment {
    ConfigFragment {
        name: "gaia.no-root".into(),
        text: "BR2_INIT_NONE=y\nBR2_SYSTEM_BIN_SH_NONE=y\n# BR2_PACKAGE_BUSYBOX is not set\n"
            .into(),
    }
}

/// The parts of one image's OS, each a build of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Parts {
    pub kernel: Option<OsPlan>,
    pub bootloader: Option<OsPlan>,
    pub rootfs: OsPlan,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Kernel,
    Bootloader,
    Rootfs,
}

/// Where a config symbol's line goes: one part, or several.
fn parts_of(symbol: &str, kernel: bool, bootloader: bool) -> Vec<Part> {
    const SYSTEM: [&str; 7] = [
        "BR2_TARGET_GENERIC_",
        "BR2_TARGET_ROOTFS_",
        "BR2_TARGET_ENABLE_ROOT_LOGIN",
        "BR2_TARGET_TZ",
        "BR2_TARGET_LOCALTIME",
        "BR2_TARGET_OPTIMIZATION",
        "BR2_TARGET_LDFLAGS",
    ];
    const EVERY: [&str; 9] = [
        "BR2_TOOLCHAIN",
        "BR2_KERNEL_HEADERS",
        "BR2_CCACHE",
        "BR2_JLEVEL",
        "BR2_GLOBAL_PATCH_DIR",
        "BR2_DOWNLOAD_FORCE_CHECK_HASHES",
        "BR2_PER_PACKAGE_DIRECTORIES",
        "BR2_OPTIMIZE_",
        "BR2_ENABLE_",
    ];
    let mut every = vec![Part::Rootfs];
    if kernel {
        every.push(Part::Kernel);
    }
    if bootloader {
        every.push(Part::Bootloader);
    }
    if EVERY.iter().any(|prefix| symbol.starts_with(prefix)) {
        return every;
    }
    if symbol.starts_with("BR2_PACKAGE_HOST_") {
        let built: Vec<Part> = every.into_iter().filter(|p| *p != Part::Rootfs).collect();
        return if built.is_empty() {
            vec![Part::Rootfs]
        } else {
            built
        };
    }
    if kernel && symbol.starts_with("BR2_LINUX_KERNEL") {
        return vec![Part::Kernel];
    }
    if bootloader
        && symbol.starts_with("BR2_TARGET_")
        && !SYSTEM.iter().any(|prefix| symbol.starts_with(prefix))
    {
        return vec![Part::Bootloader];
    }
    vec![Part::Rootfs]
}

/// The symbol a config line sets or unsets.
fn symbol_of(line: &str) -> Option<&str> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("# ") {
        return rest.strip_suffix(" is not set");
    }
    if line.starts_with('#') {
        return None;
    }
    line.split_once('=').map(|(name, _)| name)
}

/// `plan` as its parts. Each has the fragments' lines that are its own,
/// under the same fragment names, and only the input trees those lines and
/// its overrides name.
pub(crate) fn split(plan: &OsPlan, board: &Board) -> Parts {
    let (has_kernel, has_bootloader) = (board.kernel.is_some(), board.bootloader.is_some());
    let part = |which: Part| -> OsPlan {
        let mut fragments = Vec::new();
        if which != Part::Rootfs {
            fragments.push(no_root());
        }
        for fragment in &plan.config.fragments {
            let whole = match fragment.name.as_str() {
                "board.target" => Some(true),
                "board.kernel" => Some(which == Part::Kernel),
                "board.bootloader" => Some(which == Part::Bootloader),
                _ => None,
            };
            let text: String = match whole {
                Some(true) => fragment.text.clone(),
                Some(false) => String::new(),
                None => fragment
                    .text
                    .lines()
                    .filter(|line| {
                        symbol_of(line).is_some_and(|symbol| {
                            parts_of(symbol, has_kernel, has_bootloader).contains(&which)
                        })
                    })
                    .map(|line| format!("{}\n", line.trim()))
                    .collect(),
            };
            if !text.is_empty() {
                fragments.push(ConfigFragment {
                    name: fragment.name.clone(),
                    text,
                });
            }
        }
        let overrides: Vec<SourceOverride> = plan
            .config
            .overrides
            .iter()
            .filter(|o| match which {
                Part::Kernel => o.package == "LINUX",
                Part::Bootloader => o.package != "LINUX",
                Part::Rootfs => false,
            })
            .cloned()
            .collect();
        let used = |name: &str| {
            let marker = format!("@{name}@");
            fragments.iter().any(|f| f.text.contains(&marker))
                || overrides.iter().any(|o| o.input == name)
        };
        let inputs: Vec<(String, InputFrom)> = plan
            .inputs
            .iter()
            .filter(|(name, _)| used(name))
            .cloned()
            .collect();
        OsPlan {
            config: OsConfig {
                fragments,
                inputs: inputs.iter().map(|(name, _)| name.clone()).collect(),
                overrides,
                externals: Vec::new(),
                kernel_contracts: Vec::new(),
            },
            inputs,
        }
    };
    Parts {
        kernel: has_kernel.then(|| part(Part::Kernel)),
        bootloader: has_bootloader.then(|| part(Part::Bootloader)),
        rootfs: part(Part::Rootfs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_goes_to_the_part_that_builds_it() {
        let to = |symbol: &str| parts_of(symbol, true, true);
        assert!(to("BR2_LINUX_KERNEL_INSTALL_TARGET") == vec![Part::Kernel]);
        assert!(to("BR2_TARGET_UBOOT_NEEDS_DTC") == vec![Part::Bootloader]);
        assert!(to("BR2_TARGET_ARM_TRUSTED_FIRMWARE_IMAGES") == vec![Part::Bootloader]);
        assert!(to("BR2_TARGET_GENERIC_HOSTNAME") == vec![Part::Rootfs]);
        assert!(to("BR2_TARGET_ROOTFS_EXT2") == vec![Part::Rootfs]);
        assert!(to("BR2_TARGET_ENABLE_ROOT_LOGIN") == vec![Part::Rootfs]);
        assert!(to("BR2_PACKAGE_BUSYBOX") == vec![Part::Rootfs]);
        assert!(to("BR2_INIT_SYSTEMD") == vec![Part::Rootfs]);
        assert!(to("BR2_PACKAGE_HOST_DTC") == vec![Part::Kernel, Part::Bootloader]);
        assert!(to("BR2_TOOLCHAIN_EXTERNAL").len() == 3);
        // With no kernel or bootloader build, their lines stay where one
        // build has everything.
        assert!(parts_of("BR2_LINUX_KERNEL", false, false) == vec![Part::Rootfs]);
        assert!(parts_of("BR2_TARGET_UBOOT", false, false) == vec![Part::Rootfs]);
        assert!(parts_of("BR2_PACKAGE_HOST_DTC", false, false) == vec![Part::Rootfs]);
    }

    /// Per-package directories are on by default, and the default is one
    /// fragment that a description can turn off after it.
    #[test]
    fn per_package_directories_are_on_by_default() {
        let fragment = parallel_defaults();
        assert_eq!(fragment.name, "gaia.parallel");
        assert_eq!(fragment.text, "BR2_PER_PACKAGE_DIRECTORIES=y\n");
        assert_eq!(
            symbol_of(fragment.text.trim_end()),
            Some("BR2_PER_PACKAGE_DIRECTORIES")
        );
    }

    /// The compiler cache is off unless a description turns it on: no
    /// default fragment names it.
    #[test]
    fn the_compiler_cache_is_not_a_default() {
        assert!(defaults().is_empty());
    }

    #[test]
    fn a_symbol_is_read_from_a_set_or_an_unset_line() {
        assert_eq!(symbol_of("BR2_X=y"), Some("BR2_X"));
        assert_eq!(symbol_of("  # BR2_X is not set "), Some("BR2_X"));
        assert_eq!(symbol_of("# a comment"), None);
        assert_eq!(symbol_of(""), None);
    }
}
