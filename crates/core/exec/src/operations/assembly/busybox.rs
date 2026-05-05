use super::*;

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
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
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
        let dependencies = resolve_busybox_runtime_libraries(spec, &src, cancel_check)?;
        if dependencies.is_empty() {
            (BusyboxRuntimeLinkage::Static, Vec::new())
        } else {
            let copied = copy_busybox_runtime_libraries(tree, &dependencies)?;
            (BusyboxRuntimeLinkage::Dynamic, copied)
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

fn resolve_busybox_runtime_libraries(
    spec: &ResolvedBuildSpec,
    busybox: &Path,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<Vec<PathBuf>, AssemblyError> {
    resolve_busybox_runtime_libraries_with_program(spec, busybox, Path::new("ldd"), cancel_check)
}

pub(super) fn resolve_busybox_runtime_libraries_with_program(
    spec: &ResolvedBuildSpec,
    busybox: &Path,
    ldd_program: &Path,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> Result<Vec<PathBuf>, AssemblyError> {
    let mut command = Command::new(ldd_program);
    command.arg(busybox);
    let output = run_command_capture_tail(
        spec,
        &mut command,
        process_output_retention(spec),
        cancel_check,
    )
    .map_err(|error| AssemblyError {
        kind: error.kind,
        message: format!(
            "failed to resolve busybox runtime libraries for '{}': {}",
            busybox.display(),
            error.message
        ),
    })?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(parse_busybox_runtime_libraries_from_ldd(
        &combined,
        output.status.success(),
        busybox,
    )?)
}

pub(super) fn parse_busybox_runtime_libraries_from_ldd(
    combined: &str,
    success: bool,
    busybox: &Path,
) -> Result<Vec<PathBuf>, String> {
    let lowered = combined.to_ascii_lowercase();
    if lowered.contains("not a dynamic executable") || lowered.contains("statically linked") {
        return Ok(Vec::new());
    }
    if !success {
        return Err(format!(
            "failed to resolve busybox runtime libraries for '{}': {}",
            busybox.display(),
            combined.trim()
        ));
    }
    let mut libraries = Vec::new();
    for line in combined.lines() {
        if let Some(path) = parse_ldd_library_path(line)
            && !libraries.iter().any(|existing| existing == &path)
        {
            libraries.push(path);
        }
    }
    if libraries.is_empty() {
        return Err(format!(
            "busybox runtime library resolver produced no libraries for '{}'",
            busybox.display()
        ));
    }
    Ok(libraries)
}

pub(super) fn parse_ldd_library_path(line: &str) -> Option<PathBuf> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("linux-vdso") {
        return None;
    }
    if let Some((_, right)) = trimmed.split_once("=>") {
        let path = right.split_whitespace().next()?;
        if path.starts_with('/') {
            return Some(PathBuf::from(path));
        }
        return None;
    }
    let path = trimmed.split_whitespace().next()?;
    path.starts_with('/').then(|| PathBuf::from(path))
}

fn copy_busybox_runtime_libraries(
    tree: &Path,
    libraries: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let mut copied = Vec::new();
    for library in libraries {
        if !library.is_file() {
            return Err(format!(
                "busybox runtime library '{}' does not exist or is not a file",
                library.display()
            ));
        }
        let relative = library.strip_prefix("/").unwrap_or(library);
        let dest = tree.join(relative);
        if let Some(parent) = dest.parent() {
            std_fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create busybox runtime library dir '{}': {error}",
                    parent.display()
                )
            })?;
        }
        std_fs::copy(library, &dest).map_err(|error| {
            format!(
                "failed to copy busybox runtime library '{}' to '{}': {error}",
                library.display(),
                dest.display()
            )
        })?;
        copied.push(dest);
    }
    Ok(copied)
}
