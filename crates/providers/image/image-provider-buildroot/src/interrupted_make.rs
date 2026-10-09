//! Keeping the work of a Buildroot `make` that did not finish.
//!
//! A `make` that fails or is cancelled leaves every finished package built
//! (Buildroot writes a step's stamp only after the step succeeded), so the
//! next run continues from there; those packages are also stored in the
//! package cache before the error is returned.
//!
//! A `make` that was killed (cancelled, timed out, or Gaia itself stopped)
//! can leave the steps it was running half done: half-applied patches, a
//! half-configured tree, a partial install into the package's per-package
//! directory. Redoing such a step on top of what is there can fail or build
//! something different, so the packages that were in progress are built
//! again from the start. A marker written before `make` and removed when it
//! exits on its own (successfully or with a build error) tells the two
//! apart.
use super::*;

/// Present while a `make` runs, and after one that was killed.
const MAKE_RUNNING: &str = ".gaia-make-running";

pub(crate) fn mark_make_running(output_dir: &Path) -> Result<(), ImageProviderError> {
    fs::write(output_dir.join(MAKE_RUNNING), "").map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write '{}': {error}",
            output_dir.join(MAKE_RUNNING).display()
        ))
    })
}

/// After `make` stopped: clears the marker unless it was killed, and on an
/// error stores the packages it finished in the package cache.
pub(crate) fn finish_make(
    output_dir: &Path,
    result: Result<Vec<String>, ImageProviderError>,
    cached: Option<&CachedPackages>,
) -> Result<Vec<String>, ImageProviderError> {
    let killed = matches!(
        &result,
        Err(error) if matches!(
            error.kind,
            ImageProviderErrorKind::Cancelled | ImageProviderErrorKind::Timeout
        )
    );
    if !killed {
        let _ = fs::remove_file(output_dir.join(MAKE_RUNNING));
    }
    if result.is_err()
        && let Some(cached) = cached
    {
        for message in cached.store(output_dir) {
            tracing::info!(provider_domain = "image.buildroot", "{message}");
        }
    }
    result
}

/// After a killed `make`, removes the packages it was in the middle of
/// (their build directory and per-package directory) so they build again
/// from the start. Packages with no step done yet and finished packages are
/// left alone.
pub(crate) fn redo_interrupted_packages(
    output_dir: &Path,
    graphs: &[Option<&PackageGraph>],
) -> Result<Vec<String>, ImageProviderError> {
    let marker = output_dir.join(MAKE_RUNNING);
    if !marker.is_file() {
        return Ok(Vec::new());
    }
    let names = graphs
        .iter()
        .flatten()
        .flat_map(|graph| graph.packages.iter())
        .filter_map(|(name, package)| Some((package.stamp_dir.clone()?, name.clone())))
        .collect::<BTreeMap<_, _>>();
    let mut redone = Vec::new();
    for entry in fs::read_dir(output_dir.join("build"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let dir = entry.path();
        if !dir.is_dir() || !in_progress(&dir) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let name = names.get(&format!("build/{dir_name}")).cloned();
        let mut paths = vec![dir];
        if let Some(name) = &name {
            paths.push(output_dir.join("per-package").join(name));
        }
        for path in paths {
            gaia_process::discard(&path).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to remove '{}': {error}",
                    path.display()
                ))
            })?;
        }
        redone.push(name.unwrap_or(dir_name));
    }
    let _ = fs::remove_file(&marker);
    redone.sort();
    Ok(vec![if redone.is_empty() {
        "resuming an interrupted Buildroot make: no package was half built".to_string()
    } else {
        format!(
            "resuming an interrupted Buildroot make: building {} again from the start: {}",
            redone.len(),
            redone.join(", ")
        )
    }])
}

/// A package build directory with some steps done but not the last.
fn in_progress(dir: &Path) -> bool {
    let has_stamps = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".stamp_"))
        });
    has_stamps && !dir.join(".stamp_installed").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gaia-interrupted-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn package(output: &Path, dir: &str, stamps: &[&str]) {
        let build = output.join("build").join(dir);
        fs::create_dir_all(&build).expect("build dir");
        for stamp in stamps {
            fs::write(build.join(stamp), "").expect("stamp");
        }
    }

    #[test]
    fn a_killed_make_redoes_only_the_packages_in_progress() {
        let output = temp("killed");
        package(&output, "zlib-1.3", &[".stamp_built", ".stamp_installed"]);
        package(
            &output,
            "mesa3d-25.0",
            &[".stamp_extracted", ".stamp_patched"],
        );
        package(&output, "busybox-1.37", &[]);
        // Buildroot 2026.08 `pkg-generic.mk`: `.stamp_installed` follows
        // the build and every install step that applies, for host, target,
        // staging-only and virtual packages alike.
        let installed = |kind: &'static str| [".stamp_built", kind, ".stamp_installed"];
        package(
            &output,
            "host-cmake-3.31",
            &installed(".stamp_host_installed"),
        );
        package(
            &output,
            "linux-headers-6.12",
            &installed(".stamp_staging_installed"),
        );
        package(
            &output,
            "rpi-firmware-1.2",
            &installed(".stamp_images_installed"),
        );
        package(&output, "toolchain", &[".stamp_installed"]);
        // Killed after the host install, before the final stamp.
        package(
            &output,
            "host-meson-1.8",
            &[".stamp_built", ".stamp_host_installed"],
        );
        fs::create_dir_all(output.join("per-package/mesa3d/target")).expect("ppd");
        fs::create_dir_all(output.join("per-package/zlib/target")).expect("ppd");
        let mut graph = PackageGraph::default();
        for (name, dir) in [("zlib", "zlib-1.3"), ("mesa3d", "mesa3d-25.0")] {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    stamp_dir: Some(format!("build/{dir}")),
                    ..PackageInfo::default()
                },
            );
        }

        // A make that exited on its own leaves nothing to redo.
        mark_make_running(&output).expect("marker");
        finish_make(&output, Ok(Vec::new()), None).expect("finished");
        assert!(
            redo_interrupted_packages(&output, &[Some(&graph)])
                .expect("redo")
                .is_empty()
        );
        assert!(output.join("build/mesa3d-25.0").is_dir());

        // A cancelled one keeps the marker; the next run redoes mesa3d only.
        mark_make_running(&output).expect("marker");
        let cancelled = finish_make(
            &output,
            Err(ImageProviderError::new(
                ImageProviderErrorKind::Cancelled,
                "buildroot make cancelled",
            )),
            None,
        );
        assert!(cancelled.is_err());
        let messages = redo_interrupted_packages(&output, &[Some(&graph)]).expect("redo");
        assert_eq!(
            messages,
            [
                "resuming an interrupted Buildroot make: building 2 again from the start: \
              host-meson-1.8, mesa3d"
            ]
        );
        for finished in [
            "host-cmake-3.31",
            "linux-headers-6.12",
            "rpi-firmware-1.2",
            "toolchain",
        ] {
            assert!(output.join("build").join(finished).is_dir(), "{finished}");
        }
        assert!(!output.join("build/mesa3d-25.0").exists());
        assert!(!output.join("per-package/mesa3d").exists());
        assert!(output.join("build/zlib-1.3/.stamp_installed").is_file());
        assert!(output.join("per-package/zlib/target").is_dir());
        assert!(output.join("build/busybox-1.37").is_dir());
        assert!(!output.join(MAKE_RUNNING).exists());
        let _ = fs::remove_dir_all(output);
    }
}
