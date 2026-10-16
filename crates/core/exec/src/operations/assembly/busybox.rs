use super::runtime_closure::{RuntimeLinkage, copy_runtime_closure};
use super::*;

pub(super) struct BusyboxInitramfsSummary {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) applets: Vec<String>,
    pub(super) runtime_linkage: RuntimeLinkage,
    pub(super) runtime_libraries: Vec<PathBuf>,
}

pub(super) fn execute_busybox_initramfs(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    initramfs: &gaia_spec::AssemblyBusyboxInitramfsSpec,
) -> Result<BusyboxInitramfsSummary, AssemblyError> {
    let tree = roots.tree_path(&initramfs.tree)?;
    let src = roots.resolve_path(spec, &initramfs.busybox)?;
    if !src.is_file() {
        return Err(format!(
            "busybox initramfs source '{}' does not exist or is not a file",
            src.display()
        )
        .into());
    }
    let dest = tree.join("bin/busybox");
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create busybox initramfs bin dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    std_fs::copy(&src, &dest).map_err(|error| {
        format!(
            "failed to copy busybox '{}' to '{}': {error}",
            src.display(),
            dest.display()
        )
    })?;
    apply_mode(
        &dest,
        Some(
            "0755"
                .parse()
                .map_err(|error: gaia_spec::FileModeParseError| error.to_string())?,
        ),
    )?;

    for applet in &initramfs.applets {
        create_busybox_applet_symlink(tree, applet)?;
    }

    let (runtime_linkage, runtime_libraries) = if initramfs.include_runtime_libs {
        let sysroot = initramfs
            .sysroot
            .as_ref()
            .map(|template| roots.resolve_path(spec, template))
            .transpose()?;
        let closure = gaia_plan::resolve_runtime_closure(&src, sysroot.as_deref())?;
        if closure.dynamic {
            let copied = copy_runtime_closure(tree, &closure)?;
            (RuntimeLinkage::Dynamic, copied)
        } else {
            (RuntimeLinkage::Static, Vec::new())
        }
    } else {
        (RuntimeLinkage::NotRequested, Vec::new())
    };

    Ok(BusyboxInitramfsSummary {
        src,
        bytes: file_len(&dest)?,
        sha256: file_sha256(&dest)?,
        dest,
        applets: initramfs.applets.clone(),
        runtime_linkage,
        runtime_libraries,
    })
}

pub(super) fn create_busybox_applet_symlink(tree: &Path, applet: &str) -> Result<(), String> {
    if applet.trim().is_empty() || applet.contains('/') || applet.contains('\\') {
        return Err(format!(
            "busybox applet '{applet}' must be a simple file name"
        ));
    }
    let applet_path = tree.join("bin").join(applet);
    if applet_path.exists() || applet_path.symlink_metadata().is_ok() {
        std_fs::remove_file(&applet_path).map_err(|error| {
            format!(
                "failed to replace busybox applet symlink '{}': {error}",
                applet_path.display()
            )
        })?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("busybox", &applet_path).map_err(|error| {
            format!(
                "failed to create busybox applet symlink '{}' -> busybox: {error}",
                applet_path.display()
            )
        })
    }
    #[cfg(not(unix))]
    {
        std_fs::copy(tree.join("bin/busybox"), &applet_path)
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "failed to copy busybox applet '{}' on this platform: {error}",
                    applet_path.display()
                )
            })
    }
}
