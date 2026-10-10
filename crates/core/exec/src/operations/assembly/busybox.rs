use super::*;
use gaia_plan::RuntimeEntry;

pub(super) struct BusyboxInitramfsSummary {
    pub(super) src: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) applets: Vec<String>,
    pub(super) runtime_linkage: BusyboxRuntimeLinkage,
    pub(super) runtime_libraries: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BusyboxRuntimeLinkage {
    NotRequested,
    Static,
    Dynamic,
}

impl BusyboxRuntimeLinkage {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not-requested",
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
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
            let copied = copy_busybox_runtime_closure(tree, &closure)?;
            (BusyboxRuntimeLinkage::Dynamic, copied)
        } else {
            (BusyboxRuntimeLinkage::Static, Vec::new())
        }
    } else {
        (BusyboxRuntimeLinkage::NotRequested, Vec::new())
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

/// Writes a resolved runtime closure into the tree at its absolute paths:
/// the interpreter and each library as a copy of the sysroot's file, and each
/// symlink crossed on the way as a symlink with the sysroot's target text.
/// Returns the tree paths of the copied files.
pub(super) fn copy_busybox_runtime_closure(
    tree: &Path,
    closure: &gaia_plan::RuntimeClosure,
) -> Result<Vec<PathBuf>, String> {
    let mut copied = Vec::new();
    for entry in &closure.entries {
        let dest = tree.join(entry.guest().trim_start_matches('/'));
        if let Some(parent) = dest.parent() {
            std_fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create busybox runtime dir '{}': {error}",
                    parent.display()
                )
            })?;
        }
        if let Ok(metadata) = dest.symlink_metadata() {
            if metadata.is_dir() {
                return Err(format!(
                    "busybox runtime path '{}' is a directory in the tree, but the sysroot needs it as a link or file; do not create that directory in the tree",
                    dest.display()
                ));
            }
            std_fs::remove_file(&dest).map_err(|error| {
                format!(
                    "failed to replace busybox runtime path '{}': {error}",
                    dest.display()
                )
            })?;
        }
        match entry {
            RuntimeEntry::File { source, .. } => {
                std_fs::copy(source, &dest).map_err(|error| {
                    format!(
                        "failed to copy busybox runtime file '{}' to '{}': {error}",
                        source.display(),
                        dest.display()
                    )
                })?;
                copied.push(dest);
            }
            RuntimeEntry::Symlink { target, .. } => create_runtime_symlink(target, &dest)?,
        }
    }
    Ok(copied)
}

#[cfg(unix)]
fn create_runtime_symlink(target: &str, dest: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, dest).map_err(|error| {
        format!(
            "failed to create busybox runtime link '{}' -> '{target}': {error}",
            dest.display()
        )
    })
}

#[cfg(not(unix))]
fn create_runtime_symlink(_target: &str, dest: &Path) -> Result<(), String> {
    Err(format!(
        "busybox runtime link '{}' needs a Unix host to create symlinks",
        dest.display()
    ))
}
