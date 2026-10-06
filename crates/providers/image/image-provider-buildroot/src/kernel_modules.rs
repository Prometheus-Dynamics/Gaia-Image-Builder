//! Catches an image that ships fewer kernel modules than the kernel built.
//!
//! Buildroot's `linux.mk` does not fail when `modules_install` stops part
//! way (for example when its output pipe closes after a docker client was
//! killed), so a build can "succeed" with most modules missing. The kernel's
//! `modules.order` lists every module the current configuration built;
//! comparing it with the modules installed under `target/lib/modules`
//! exposes the gap. `[providers.buildroot] kernel_modules_check` decides
//! whether a gap fails the image operation (default), warns, or is ignored.

use super::*;
use gaia_spec::BuildrootOverrideCheckSpec;

pub(crate) const KERNEL_MODULES_WARNING_PREFIX: &str = "warning: kernel modules: ";

const MODULE_SUFFIXES: &[&str] = &[".ko", ".ko.gz", ".ko.xz", ".ko.zst"];

/// Modules listed in the kernel build's `modules.order`, or `None` when the
/// image does not build a modular Linux kernel.
fn built_module_count(output_dir: &Path) -> Option<usize> {
    let build_dir = output_dir.join("build");
    let mut counts = fs::read_dir(build_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // `linux-<version>`, not `linux-firmware-*` or `linux-headers-*`.
            name.starts_with("linux-")
                && !name.starts_with("linux-firmware")
                && !name.starts_with("linux-headers")
                && !name.starts_with("linux-tools")
                && !name.starts_with("linux-backports")
        })
        .filter_map(|entry| fs::read_to_string(entry.path().join("modules.order")).ok())
        .map(|order| order.lines().filter(|line| !line.trim().is_empty()).count())
        .collect::<Vec<_>>();
    counts.sort_unstable();
    counts.pop()
}

fn installed_module_count(target_dir: &Path) -> usize {
    fn count(dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    count(&path)
                } else {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    usize::from(MODULE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)))
                }
            })
            .sum()
    }
    count(&target_dir.join("lib/modules"))
}

/// Compares built and installed kernel modules after `make`.
pub(crate) fn check_kernel_modules_installed(
    output_dir: &Path,
    mode: BuildrootOverrideCheckSpec,
) -> Result<Vec<String>, ImageProviderError> {
    if mode == BuildrootOverrideCheckSpec::Off {
        return Ok(Vec::new());
    }
    let Some(built) = built_module_count(output_dir) else {
        return Ok(Vec::new());
    };
    let installed = installed_module_count(&output_dir.join("target"));
    if installed >= built {
        return Ok(Vec::new());
    }
    let detail = format!(
        "the image holds {installed} of the {built} kernel modules the kernel build produced \
         (modules.order); Buildroot does not fail when `modules_install` stops part way, so \
         the build log around linux modules_install shows the cause"
    );
    if mode == BuildrootOverrideCheckSpec::Error {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::OutputMissing,
            format!(
                "incomplete kernel module install: {detail}\nif a post-build script removes \
                 modules on purpose, set [providers.buildroot] kernel_modules_check = \"warn\""
            ),
        ));
    }
    Ok(vec![format!("{KERNEL_MODULES_WARNING_PREFIX}{detail}")])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(installed: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gaia-kernel-modules-{installed}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let linux = dir.join("build/linux-6.12.20");
        fs::create_dir_all(&linux).expect("linux build dir");
        fs::create_dir_all(dir.join("build/linux-firmware-2025")).expect("firmware dir");
        fs::write(
            linux.join("modules.order"),
            "drivers/a.o\ndrivers/b.o\ndrivers/c.o\n",
        )
        .expect("modules.order");
        let modules = dir.join("target/lib/modules/6.12.20/kernel/drivers");
        fs::create_dir_all(&modules).expect("modules dir");
        for index in 0..installed {
            fs::write(modules.join(format!("m{index}.ko.xz")), "").expect("module");
        }
        dir
    }

    #[test]
    fn partial_module_install_fails_by_default() {
        let dir = tree(1);
        let error = check_kernel_modules_installed(&dir, BuildrootOverrideCheckSpec::Error)
            .expect_err("partial install must fail");
        assert!(
            error.message.contains("1 of the 3 kernel modules"),
            "{error:?}"
        );
        let warnings = check_kernel_modules_installed(&dir, BuildrootOverrideCheckSpec::Warn)
            .expect("warn mode");
        assert_eq!(warnings.len(), 1);
        assert!(
            check_kernel_modules_installed(&dir, BuildrootOverrideCheckSpec::Off)
                .expect("off")
                .is_empty()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn complete_install_or_no_modular_kernel_passes() {
        let dir = tree(3);
        assert!(
            check_kernel_modules_installed(&dir, BuildrootOverrideCheckSpec::Error)
                .expect("complete")
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
        let empty = std::env::temp_dir().join(format!("gaia-no-kernel-{}", std::process::id()));
        fs::create_dir_all(empty.join("build")).expect("build dir");
        assert!(
            check_kernel_modules_installed(&empty, BuildrootOverrideCheckSpec::Error)
                .expect("no kernel")
                .is_empty()
        );
        let _ = fs::remove_dir_all(empty);
    }
}
