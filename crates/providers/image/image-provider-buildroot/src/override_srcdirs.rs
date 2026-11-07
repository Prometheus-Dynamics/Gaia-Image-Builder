//! `[image] override_srcdirs`: Buildroot packages built from a directory
//! instead of their download, written to the output tree's `local.mk` as
//! `<PKG>_OVERRIDE_SRCDIR`. For a vendor kernel or U-Boot that lives inside
//! an SDK checkout (the Luckfox RV1106 SDK's `sysdrv/source/kernel`), which
//! Buildroot's git and tarball methods cannot fetch a subdirectory of.
//! Buildroot rsyncs such a package's tree on every build, and the package
//! cache never stores it (see `package_keys`).
use super::*;
use crate::host_tools::write_local_mk_section;
use crate::package_keys::upper_name;

const BEGIN: &str = "# BEGIN gaia override_srcdirs (generated; do not edit)";
const END: &str = "# END gaia override_srcdirs";

/// Replaces Gaia's `override_srcdirs` block in `<output>/local.mk` (removing
/// it when the build has none). Each directory resolves like other
/// workspace paths (relative to the workspace root, or through an alias) and
/// must exist.
pub(crate) fn write_override_srcdirs(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    output_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let entries = match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.override_srcdirs.as_slice(),
        _ => &[][..],
    };
    let mut block = String::new();
    let mut messages = Vec::new();
    for (package, dir) in entries {
        let resolved = resolve_workspace_path(spec, dir)?;
        if !resolved.is_dir() {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                format!(
                    "override_srcdirs: '{package}' names '{}', which is not a directory",
                    resolved.display()
                ),
            ));
        }
        // Only `local.mk` names it, so a container would not mount it.
        gaia_process::register_read_only_docker_mount(&resolved);
        block.push_str(&format!(
            "{}_OVERRIDE_SRCDIR = {}\n",
            upper_name(package),
            resolved.display()
        ));
        messages.push(format!(
            "buildroot package '{package}' builds from '{}'",
            resolved.display()
        ));
    }
    write_local_mk_section(output_dir, BEGIN, END, &block)?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::BuildrootImageSpec;
    use gaia_testkit::TempDir;

    fn image_with(entries: Vec<(String, String)>) -> ImageSpec {
        ImageSpec {
            signing: None,
            layers: None,
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                override_srcdirs: entries,
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: gaia_spec::ImageOutputSpec::default(),
            assembly: None,
            checks: gaia_spec::ImageChecksSpec::default(),
            hardware: Vec::new(),
        }
    }

    #[test]
    fn packages_build_from_their_dirs_and_the_block_goes_with_the_entries() {
        let scratch = TempDir::new("gaia-override-srcdirs");
        let root = scratch.join("workspace");
        let kernel = root.join("sdk/sysdrv/source/kernel");
        let uboot = root.join("sdk/sysdrv/source/uboot/u-boot");
        fs::create_dir_all(&kernel).expect("kernel dir");
        fs::create_dir_all(&uboot).expect("u-boot dir");
        let output = scratch.join("output");
        fs::create_dir_all(&output).expect("output dir");
        fs::write(output.join("local.mk"), "FOO_OVERRIDE_SRCDIR = /src/foo\n").expect("local.mk");
        let mut spec = ResolvedBuildSpec::new("override-srcdirs");
        spec.workspace.root_dir = root.display().to_string();

        let image = image_with(vec![
            ("linux".into(), "sdk/sysdrv/source/kernel".into()),
            ("uboot".into(), uboot.display().to_string()),
        ]);
        let messages = write_override_srcdirs(&spec, &image, &output).expect("written");
        assert_eq!(messages.len(), 2);
        let local = fs::read_to_string(output.join("local.mk")).expect("local.mk");
        assert!(
            local.starts_with("FOO_OVERRIDE_SRCDIR = /src/foo\n"),
            "{local}"
        );
        assert!(
            local.contains(&format!("LINUX_OVERRIDE_SRCDIR = {}\n", kernel.display())),
            "{local}"
        );
        assert!(
            local.contains(&format!("UBOOT_OVERRIDE_SRCDIR = {}\n", uboot.display())),
            "{local}"
        );

        write_override_srcdirs(&spec, &image_with(Vec::new()), &output).expect("removed");
        let local = fs::read_to_string(output.join("local.mk")).expect("local.mk");
        assert_eq!(local, "FOO_OVERRIDE_SRCDIR = /src/foo\n");
    }

    #[test]
    fn a_missing_dir_fails_before_buildroot_runs() {
        let scratch = TempDir::new("gaia-override-srcdirs-missing");
        let output = scratch.join("output");
        fs::create_dir_all(&output).expect("output dir");
        let mut spec = ResolvedBuildSpec::new("override-srcdirs");
        spec.workspace.root_dir = scratch.path().display().to_string();
        let image = image_with(vec![("linux".into(), "no/such/kernel".into())]);
        let error = write_override_srcdirs(&spec, &image, &output).expect_err("missing");
        assert!(
            error.message.contains("not a directory"),
            "{}",
            error.message
        );
    }
}
