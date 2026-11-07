//! Changes Gaia makes to the Buildroot source it builds with. Each one is
//! applied only to the exact upstream text it replaces, and also exists as
//! an upstreamable patch under `contrib/buildroot/`.
//!
//! With per-package directories, Buildroot assembles the final `target/`
//! and `host/` by copying every package's tree (one `rsync`, which reads and
//! writes every file). On a filesystem with reflinks (btrfs, XFS) a
//! copy-on-write clone (`cp --reflink`) makes the same files without
//! copying their data, and keeps them independent: later in-place edits
//! (stripping, `patchelf`, post-build scripts) do not reach the per-package
//! trees. The last package's tree still wins where two hold the same path.
//! Without reflinks, `cp` would copy everything on every `make` where
//! `rsync` copies only what changed, so the change is applied only when the
//! output directory supports reflinks.

use super::*;

/// `per-package-rsync`'s copy mode in `package/pkg-utils.mk`, as upstream
/// has it since 2024.08.
pub(crate) const PPD_COPY_UPSTREAM: &str = "\t\tprintf \"%s/$(2)/\\n\" $(1) | tac \\\n\t\t\t| rsync -a --hard-links --files-from=- --no-R -r $(PER_PACKAGE_DIR) $(3))";

/// The same, cloning each package's tree in order (the last one wins, as
/// the reversed list makes it win with `rsync`).
pub(crate) const PPD_COPY_REFLINK: &str = "\t\t$(foreach pkg,$(1),\\\n\t\t\tcp -a --reflink=auto --remove-destination $(PER_PACKAGE_DIR)/$(pkg)/$(2)/. $(3)/$(sep)))";

/// `contents` of a Buildroot file with Gaia's changes put back to the
/// upstream text. They change how the output tree is assembled, never what
/// a package builds, so package cache keys hash this: a Gaia version
/// patching Buildroot differently must not invalidate every cached package.
pub(crate) fn without_gaia_patches(contents: &str) -> String {
    PARSE_PATCHES.iter().fold(
        contents
            .replace(PPD_COPY_REFLINK, PPD_COPY_UPSTREAM)
            .replace(HOST_FINALIZE_SKIP, HOST_FINALIZE_UPSTREAM),
        |contents, patch| contents.replace(patch.gaia, patch.upstream),
    )
}

/// Upstream text that starts a `$(shell)` while Buildroot's Makefiles are
/// read, each with Gaia's replacement, which gives the same values without
/// one. GNU make 4.4 builds the environment of every `$(shell)` from all of
/// its variables (tens of thousands once every package's `.mk` is read),
/// and a `$(shell)` in a recursive, exported variable runs again for each
/// such environment. These two were 98 of the 148 shells, and about 3 of
/// 11.5 CPU seconds, of reading a HeliOS tree's Makefiles, which every
/// `make` does.
pub(crate) const PARSE_PATCHES: &[ParsePatch] = &[
    // `which perl`, once per environment make builds (59 times while the
    // Makefiles are read). The path cannot change during a make.
    ParsePatch {
        file: "package/Makefile.in",
        upstream: "export PERL=$(shell which perl)\n",
        gaia: "export PERL:=$(shell which perl)\n",
    },
    // A luarocks package's directory: its version without the rockspec
    // revision (`-1`), stripped by `sed` (40 shells) where make can do it:
    // the one `-<digit>` suffix a version ends with, or the version as is.
    ParsePatch {
        file: "package/pkg-luarocks.mk",
        upstream: "    $(2)_SUBDIR ?= $$($(3)_NAME_UPSTREAM)-$$(shell echo \"$$($(3)_VERSION)\" | sed -e \"s/-[0-9]$$$$//\")\n",
        gaia: "    $(2)_SUBDIR ?= $$($(3)_NAME_UPSTREAM)-$$(firstword $$(foreach d,0 1 2 3 4 5 6 7 8 9,$$(filter-out $$($(3)_VERSION),$$(patsubst %-$$(d),%,$$($(3)_VERSION)))) $$($(3)_VERSION))\n",
    },
];

/// One of [`PARSE_PATCHES`].
pub(crate) struct ParsePatch {
    pub(crate) file: &'static str,
    pub(crate) upstream: &'static str,
    pub(crate) gaia: &'static str,
}

/// Applies [`PARSE_PATCHES`] to each file that holds its upstream text once.
/// Returns what was done, and a message per file left as is.
pub(crate) fn apply_parse_patches(buildroot_dir: &Path) -> Result<Vec<String>, ImageProviderError> {
    let mut messages = Vec::new();
    let mut applied = 0;
    for patch in PARSE_PATCHES {
        let path = buildroot_dir.join(patch.file);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if !contents.contains(patch.gaia) {
            if contents.matches(patch.upstream).count() != 1 {
                messages.push(format!(
                    "buildroot {} left as is: it differs from the upstream text Gaia patches",
                    patch.file
                ));
                continue;
            }
            fs::write(&path, contents.replace(patch.upstream, patch.gaia)).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to patch '{}': {error}",
                    path.display()
                ))
            })?;
        }
        applied += 1;
    }
    if applied > 0 {
        messages.push(format!(
            "buildroot makefiles: {applied} parse-time shell calls replaced"
        ));
    }
    Ok(messages)
}

/// The rsync of `host-finalize` in the top-level `Makefile`, as upstream has
/// it (`BR2_PER_PACKAGE_DIRECTORIES`).
pub(crate) const HOST_FINALIZE_UPSTREAM: &str =
    "\t$(call per-package-rsync,$(sort $(PACKAGES)),host,$(HOST_DIR),copy)\n";

/// The same rsync, skipped while nothing it would copy has changed since the
/// last one: the `host/.gaia-host-finalized` marker holds the package list
/// that copy covered, and it is newer than every package's installed stamp
/// and than everything under `per-package/` (a file removed and added back
/// changes its directory). The marker is written only after the rsync
/// succeeds, so a killed rsync leaves it older than the change that asked for
/// it. The rest of `host-finalize` (`fix-rpath`, the path fix-up) still runs:
/// it is idempotent and is not what the skip covers.
pub(crate) const HOST_FINALIZE_SKIP: &str = concat!(
    "\t$(if $(shell M=$(HOST_DIR)/.gaia-host-finalized; [ -f \"$$M\" ] && ",
    "printf '%s\\n' $(sort $(PACKAGES)) | cmp -s - \"$$M\" && ",
    "[ -z \"$$(find $(BUILD_DIR)/ -maxdepth 2 -name '.stamp_*installed' -newer \"$$M\" -print -quit)\" ] && ",
    "[ -z \"$$(find $(PER_PACKAGE_DIR) -newer \"$$M\" -print -quit)\" ] && echo unchanged),",
    "@echo \"host directory unchanged since the last finalize (Gaia)\",",
    "$(call per-package-rsync,$(sort $(PACKAGES)),host,$(HOST_DIR),copy))\n",
    "\t$(Q)printf '%s\\n' $(sort $(PACKAGES)) > $(HOST_DIR)/.gaia-host-finalized\n",
);

/// Makes `host-finalize` skip its rsync when nothing it copies changed (see
/// [`HOST_FINALIZE_SKIP`]). Applied to the top-level `Makefile` of the tree
/// the make runs from, like the reflink change. Returns a message saying what
/// was done.
pub(crate) fn apply_host_finalize_skip(
    buildroot_dir: &Path,
) -> Result<Option<String>, ImageProviderError> {
    let makefile = buildroot_dir.join("Makefile");
    let Ok(contents) = fs::read_to_string(&makefile) else {
        return Ok(None);
    };
    if !contents.contains(HOST_FINALIZE_SKIP) {
        if contents.matches(HOST_FINALIZE_UPSTREAM).count() != 1 {
            return Ok(Some(
                "buildroot host finalize left as is: Makefile differs from the upstream text \
                 Gaia patches"
                    .to_string(),
            ));
        }
        fs::write(
            &makefile,
            contents.replace(HOST_FINALIZE_UPSTREAM, HOST_FINALIZE_SKIP),
        )
        .map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to patch '{}': {error}",
                makefile.display()
            ))
        })?;
    }
    Ok(Some(
        "buildroot host finalize skips an unchanged host directory".to_string(),
    ))
}

/// Uses reflink clones for the per-package finalize step when the output
/// directory supports them. Returns a message saying what was done.
pub(crate) fn apply_reflink_finalize(
    buildroot_dir: &Path,
    output_dir: &Path,
) -> Result<Option<String>, ImageProviderError> {
    let makefile = buildroot_dir.join("package/pkg-utils.mk");
    let Ok(contents) = fs::read_to_string(&makefile) else {
        return Ok(None);
    };
    if contents.contains(PPD_COPY_REFLINK) {
        if supports_reflinks(output_dir) {
            return Ok(Some(
                "buildroot per-package finalize uses reflink clones".to_string(),
            ));
        }
        // The tree moved to a filesystem without reflinks (a RAM tree):
        // back to Buildroot's own copy.
        fs::write(&makefile, without_gaia_patches(&contents)).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to patch '{}': {error}",
                makefile.display()
            ))
        })?;
        return Ok(None);
    }
    if !contents.contains(PPD_COPY_UPSTREAM) {
        return Ok(Some(
            "buildroot per-package finalize left as is: package/pkg-utils.mk differs from \
             the upstream text Gaia patches"
                .to_string(),
        ));
    }
    if !supports_reflinks(output_dir) {
        return Ok(None);
    }
    fs::write(
        &makefile,
        contents.replace(PPD_COPY_UPSTREAM, PPD_COPY_REFLINK),
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to patch '{}': {error}",
            makefile.display()
        ))
    })?;
    Ok(Some(
        "buildroot per-package finalize uses reflink clones".to_string(),
    ))
}

/// Puts the files Gaia patches back to upstream's text, as a fresh copy of the
/// source holds them. For a mirror kept without a copy when this run applies
/// no patches.
pub(crate) fn revert_gaia_patches(buildroot_dir: &Path) -> Result<(), ImageProviderError> {
    let parse_patched = PARSE_PATCHES.iter().map(|patch| patch.file);
    for relative in ["Makefile", "package/pkg-utils.mk"]
        .into_iter()
        .chain(parse_patched)
    {
        let path = buildroot_dir.join(relative);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let upstream = without_gaia_patches(&contents);
        if upstream != contents {
            fs::write(&path, upstream).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to patch '{}': {error}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Whether a file in `from` can be cloned into `to` (`cp --reflink=always`):
/// both on one filesystem that supports reflinks.
pub(crate) fn reflinks_between(from: &Path, to: &Path) -> bool {
    if fs::create_dir_all(from).is_err() || fs::create_dir_all(to).is_err() {
        return false;
    }
    let probe = from.join(format!(".gaia-reflink-probe-{}", std::process::id()));
    let clone = to.join(format!(".gaia-reflink-probe-{}.clone", std::process::id()));
    let supported = fs::write(&probe, b"gaia").is_ok()
        && Command::new("cp")
            .arg("--reflink=always")
            .arg(&probe)
            .arg(&clone)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    let _ = fs::remove_file(&probe);
    let _ = fs::remove_file(&clone);
    supported
}

/// Whether `dir`'s filesystem can clone files (`cp --reflink=always`).
pub(crate) fn supports_reflinks(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".gaia-reflink-probe-{}", std::process::id()));
    let clone = dir.join(format!(".gaia-reflink-probe-{}.clone", std::process::id()));
    let supported = fs::write(&probe, b"gaia").is_ok()
        && Command::new("cp")
            .arg("--reflink=always")
            .arg(&probe)
            .arg(&clone)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    let _ = fs::remove_file(&probe);
    let _ = fs::remove_file(&clone);
    supported
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn only_the_upstream_text_is_patched_and_only_with_reflinks() {
        let buildroot_scratch = TempDir::new("gaia-buildroot-patch-source");
        let buildroot = buildroot_scratch.join("buildroot");
        let output_scratch = TempDir::new("gaia-buildroot-patch-output");
        let output = output_scratch.join("output");
        fs::create_dir_all(buildroot.join("package")).expect("package dir");
        let upstream = format!("define per-package-rsync\n{PPD_COPY_UPSTREAM}\nendef\n");
        fs::write(buildroot.join("package/pkg-utils.mk"), &upstream).expect("makefile");

        let message = apply_reflink_finalize(&buildroot, &output).expect("patch");
        let patched = fs::read_to_string(buildroot.join("package/pkg-utils.mk")).expect("read");
        if supports_reflinks(&output) {
            assert!(patched.contains(PPD_COPY_REFLINK));
            assert!(message.is_some());
            // Applying it again changes nothing.
            apply_reflink_finalize(&buildroot, &output).expect("again");
            assert_eq!(
                fs::read_to_string(buildroot.join("package/pkg-utils.mk")).expect("read"),
                patched
            );
        } else {
            assert_eq!(patched, upstream);
            assert_eq!(message, None);
        }

        fs::write(buildroot.join("package/pkg-utils.mk"), "something else\n").expect("other");
        let message = apply_reflink_finalize(&buildroot, &output).expect("unknown text");
        assert!(message.is_some_and(|message| message.contains("left as is")));
    }

    #[test]
    fn host_finalize_skip_is_applied_once_and_reverts_to_upstream() {
        let buildroot_scratch = TempDir::new("gaia-buildroot-patch-host-finalize");
        let buildroot = buildroot_scratch.join("buildroot");
        fs::create_dir_all(&buildroot).expect("buildroot dir");
        let upstream = format!(
            "host-finalize: $(PACKAGES) $(HOST_DIR)\n\t@$(call MESSAGE,\"Finalizing host directory\")\n{HOST_FINALIZE_UPSTREAM}\t$(Q)PARALLEL_JOBS=1 \\\n"
        );
        fs::write(buildroot.join("Makefile"), &upstream).expect("makefile");

        let message = apply_host_finalize_skip(&buildroot).expect("patch");
        let patched = fs::read_to_string(buildroot.join("Makefile")).expect("read");
        assert!(message.is_some_and(|message| message.contains("skips an unchanged")));
        assert!(patched.contains(HOST_FINALIZE_SKIP));
        assert!(!patched.contains(HOST_FINALIZE_UPSTREAM));
        assert!(HOST_FINALIZE_SKIP.contains(HOST_FINALIZED_MARKER));

        // Applying it again changes nothing.
        apply_host_finalize_skip(&buildroot).expect("again");
        assert_eq!(
            fs::read_to_string(buildroot.join("Makefile")).expect("read"),
            patched
        );

        // What the cache keys hash is the upstream text again.
        assert_eq!(without_gaia_patches(&patched), upstream);

        // Text that is not upstream's is left alone, with a message.
        fs::write(buildroot.join("Makefile"), "host-finalize: other\n").expect("other");
        let message = apply_host_finalize_skip(&buildroot).expect("unknown text");
        assert!(message.is_some_and(|message| message.contains("left as is")));
        assert_eq!(
            fs::read_to_string(buildroot.join("Makefile")).expect("read"),
            "host-finalize: other\n"
        );
    }

    #[test]
    fn parse_patches_apply_once_and_revert_to_upstream() {
        let buildroot_scratch = TempDir::new("gaia-buildroot-patch-parse");
        let buildroot = buildroot_scratch.join("buildroot");
        fs::create_dir_all(buildroot.join("package")).expect("package dir");
        let upstream = |patch: &ParsePatch| format!("# before\n{}# after\n", patch.upstream);
        for patch in PARSE_PATCHES {
            fs::write(buildroot.join(patch.file), upstream(patch)).expect("makefile");
        }

        let messages = apply_parse_patches(&buildroot).expect("patch");
        assert_eq!(
            messages,
            ["buildroot makefiles: 2 parse-time shell calls replaced"]
        );
        for patch in PARSE_PATCHES {
            let patched = fs::read_to_string(buildroot.join(patch.file)).expect("read");
            assert!(patched.contains(patch.gaia), "{}", patch.file);
            assert!(!patched.contains(patch.upstream), "{}", patch.file);
            // What the cache keys hash is the upstream text again.
            assert_eq!(without_gaia_patches(&patched), upstream(patch));
        }
        // Applying them again changes nothing.
        let patched =
            |patch: &ParsePatch| fs::read_to_string(buildroot.join(patch.file)).expect("read");
        let before = PARSE_PATCHES.iter().map(patched).collect::<Vec<_>>();
        apply_parse_patches(&buildroot).expect("again");
        assert_eq!(
            PARSE_PATCHES.iter().map(patched).collect::<Vec<_>>(),
            before
        );

        revert_gaia_patches(&buildroot).expect("revert");
        for patch in PARSE_PATCHES {
            assert_eq!(patched(patch), upstream(patch));
        }

        // Text that is not upstream's is left alone, with a message.
        fs::write(
            buildroot.join("package/Makefile.in"),
            "export PERL=/usr/bin/perl\n",
        )
        .expect("other");
        let messages = apply_parse_patches(&buildroot).expect("unknown text");
        assert!(
            messages
                .iter()
                .any(|message| message.contains("package/Makefile.in left as is"))
        );
        assert_eq!(
            fs::read_to_string(buildroot.join("package/Makefile.in")).expect("read"),
            "export PERL=/usr/bin/perl\n"
        );
    }

    /// The luarocks replacement strips what upstream's `sed` strips: one
    /// trailing `-<digit>`, and nothing from a version without one. Checked
    /// with the make on this host, when it has one.
    #[test]
    fn luarocks_subdir_matches_upstream_sed() {
        let Ok(probe) = Command::new("make").arg("--version").output() else {
            return;
        };
        if !probe.status.success() {
            return;
        }
        let scratch = TempDir::new("gaia-buildroot-patch-luarocks");
        let patch = &PARSE_PATCHES[1];
        let define = |line: &str| {
            format!(
                "define inner\n{line}endef\n$(eval $(call inner,x,X_,X))\nall: ; @echo '$(X__SUBDIR)'\n"
            )
        };
        for version in [
            "0.3.0-1", "2.11-1", "0.5-0", "1.0-12", "1.0", "1-2-3", "0.9.2-1",
        ] {
            let run = |text: &str| {
                fs::write(scratch.join("Makefile"), define(text)).expect("makefile");
                let output = Command::new("make")
                    .args(["-s", "-C"])
                    .arg(scratch.path())
                    .arg(format!("X_VERSION={version}"))
                    .arg("X_NAME_UPSTREAM=x")
                    .output()
                    .expect("make");
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            assert_eq!(run(patch.gaia), run(patch.upstream), "version {version}");
        }
    }
}
