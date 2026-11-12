//! An image's OS config: what the board gives, by rule, then the image's
//! own fragments.
//!
//! The rules read the board's fields and name Buildroot's own options for
//! them. Nothing here knows a board, an SoC or a vendor: a value that only
//! one board needs is in that board's file or the image's fragments.
//!
//! - **Target:** the architecture's option, and `BR2_<cpu>` for the CPU
//!   (`cortex-a55` is `BR2_cortex_a55`).
//! - **Kernel:** the locked source as Buildroot's custom git repository or
//!   tarball, built from Gaia's stored copy (`LINUX_OVERRIDE_SRCDIR`), so
//!   Buildroot downloads nothing for it; the defconfig (`defconfig` is the
//!   architecture's default), the fragment files, the in-tree device tree
//!   and the board's device tree directory.
//! - **Bootloader:** the same for U-Boot (`UBOOT`), its defconfig and
//!   fragment files; each blob is given to U-Boot's make as
//!   `<NAME>=<path>`, so a blob's name in the board file is the make
//!   variable U-Boot reads; trusted firmware (`ARM_TRUSTED_FIRMWARE`) and
//!   its platform, with U-Boot built with its BL31.
//!
//! A file or directory the config names is an input tree of the step,
//! written `@name@` in the text: a locked source by its lock name, a
//! directory beside a description as `files.<n>`. In an image's own
//! fragment, `@here@` is the fragment's directory.

use crate::Error;
use gaia_describe::{Board, FileRef, OsSpec, RelPath, Source};
use gaia_lock::{Entry, Lock};
use gaia_types::{Arch, ConfigFragment, OsConfig, SourceOverride};
use std::fs;
use std::path::{Path, PathBuf};

/// Where an input tree comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputFrom {
    /// A locked source, written in `file` under `sources.<name>`.
    Source {
        file: PathBuf,
        name: String,
        source: Source,
    },
    /// A directory beside a description.
    Dir(PathBuf),
}

/// The config and its input trees, in port order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OsPlan {
    pub config: OsConfig,
    pub inputs: Vec<(String, InputFrom)>,
}

impl OsPlan {
    fn input(&mut self, name: String, from: InputFrom) -> String {
        if !self.inputs.iter().any(|(n, _)| *n == name) {
            self.inputs.push((name.clone(), from));
            self.config.inputs.push(name.clone());
        }
        name
    }

    /// A directory as an input, `files.<n>` by first use.
    fn dir(&mut self, dir: &Path) -> String {
        if let Some((name, _)) = self
            .inputs
            .iter()
            .find(|(_, from)| *from == InputFrom::Dir(dir.to_path_buf()))
        {
            return name.clone();
        }
        let n = self
            .inputs
            .iter()
            .filter(|(_, from)| matches!(from, InputFrom::Dir(_)))
            .count();
        self.input(format!("files.{n}"), InputFrom::Dir(dir.to_path_buf()))
    }

    /// A file beside a description, as `@files.<n>@/<name>`.
    fn file(&mut self, path: &Path) -> String {
        let dir = path.parent().unwrap_or(Path::new("/"));
        let name = self.dir(dir);
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        format!("@{name}@/{file}")
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
    let mut fragments = vec![ConfigFragment {
        name: "board.target".into(),
        text: target(board)?,
    }];
    if let Some(kernel) = &board.kernel {
        let mut lines = vec!["BR2_LINUX_KERNEL=y".to_string()];
        if let Some(name) = &kernel.source {
            lines.extend(locked_source(
                &mut plan,
                board,
                name,
                "kernel.source",
                lock,
                "BR2_LINUX_KERNEL",
                "LINUX",
            )?);
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
        if let Some(dt) = &kernel.device_tree {
            lines.push(format!("BR2_LINUX_KERNEL_INTREE_DTS_NAME={}", quoted(dt)));
        }
        if let Some(dir) = &kernel.device_tree_dir {
            let name = plan.dir(dir.path());
            lines.push(format!(
                "BR2_LINUX_KERNEL_CUSTOM_DTS_DIR={}",
                quoted(&format!("@{name}@"))
            ));
        }
        if kernel.overlays.as_ref().is_some_and(|o| !o.is_empty()) {
            return Err(Error::at(
                &board.file,
                "kernel.overlays",
                "device tree overlays are not given to an OS build yet",
            ));
        }
        fragments.push(ConfigFragment {
            name: "board.kernel".into(),
            text: text(&lines),
        });
    }
    if let Some(boot) = &board.bootloader {
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
                lock,
                "BR2_TARGET_UBOOT",
                "UBOOT",
            )?);
        }
        if let Some(name) = &boot.defconfig {
            lines.push(format!(
                "BR2_TARGET_UBOOT_BOARD_DEFCONFIG={}",
                quoted(name.strip_suffix("_defconfig").unwrap_or(name))
            ));
        }
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
            let here = plan.dir(dir);
            body.replace("@here@", &format!("@{here}@"))
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
        let symbol = cpu.replace(['-', '.'], "_");
        if !symbol.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
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
    let source = board.sources.get(name).ok_or_else(|| {
        Error::at(&board.file, key, format!("no source named `{name}`"))
    })?;
    Ok(plan.input(
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
fn locked_source(
    plan: &mut OsPlan,
    board: &Board,
    name: &str,
    key: &str,
    lock: &Lock,
    prefix: &str,
    package: &str,
) -> Result<Vec<String>, Error> {
    let input = board_source_input(plan, board, name, key)?;
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
    plan.override_with(package, &input);
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
