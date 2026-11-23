//! `gaia.build.rust.toolchain`: a Rust sysroot made from a release's files.
//!
//! Each input is one official release file unpacked without its top
//! directory (`rustc-1.99.0-x86_64-unknown-linux-gnu.tar.xz` and the like): a
//! `components` file naming its component directories, and in each of those
//! the files at their place in the sysroot (`bin/rustc`,
//! `lib/rustlib/<triple>/lib/...`) beside the installer's `manifest.in`. The
//! step lays every listed component's files into one tree, the layout
//! `rustup` installs: `bin/rustc`, `bin/cargo`, `lib/rustlib/<triple>`. The
//! installer's own files (`install.sh`, `manifest.in`, the licences at the
//! top) are left out.
//!
//! The sysroot is a store tree, so a build that takes it has its digest in
//! its key: a different release, or another target's std, is another build.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::Tree;
use std::path::Path;

const NAME: &str = "gaia.build.rust.toolchain";
/// Changes when how the sysroot is laid out changes.
const RECIPE_VERSION: u32 = 1;

/// The sysroot of `rustc`, `cargo` and the standard library of the host
/// (`std_host`) and of the build's target (`std_target`, none when the
/// target is the host), each an unpacked release file.
#[node(
    id = "gaia.build.rust.toolchain",
    inputs(store, rustc, cargo, std_host, std_target),
    outputs(tree)
)]
pub fn build_rust_toolchain(
    store: &str,
    rustc: Tree,
    cargo: Tree,
    std_host: Tree,
    std_target: Option<&Tree>,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    let mut parts = vec![
        ("rustc", &rustc),
        ("cargo", &cargo),
        ("std_host", &std_host),
    ];
    if let Some(tree) = std_target {
        parts.push(("std_target", tree));
    }
    let mut key = KeyBuilder::new(NAME, RECIPE_VERSION);
    let mut dirs = Vec::new();
    for (name, tree) in &parts {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| fail(format!("{name} is not in the store")))?;
        key = key.input(name, object.digest);
        dirs.push((*name, store.path(&object)));
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("out", Path::new("out"))],
        |work| {
            let out = work.dir().join("out");
            std::fs::create_dir_all(&out).map_err(|e| fail(format!("{}: {e}", out.display())))?;
            for (name, dir) in &dirs {
                lay_out(dir, &out).map_err(|why| fail(format!("{name}: {why}")))?;
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["out"].into(),
        entries: Vec::new(),
    })
}

/// Lays the components that `release` (an unpacked release file) lists into
/// `sysroot`. A file two components both give must be the same bytes.
pub fn lay_out(release: &Path, sysroot: &Path) -> Result<(), String> {
    let list = release.join("components");
    let text = std::fs::read_to_string(&list).map_err(|e| {
        format!(
            "{}: {e} (not an unpacked Rust release file: it names its components there)",
            list.display()
        )
    })?;
    let mut any = false;
    for name in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let invalid = name.contains('/') || name == "." || name == "..";
        let component = release.join(name);
        if invalid || !component.is_dir() {
            return Err(format!(
                "`components` names {name}, which is not a directory of the release"
            ));
        }
        copy_component(&component, &component, sysroot)?;
        any = true;
    }
    if any {
        Ok(())
    } else {
        Err(format!("{} names no component", list.display()))
    }
}

/// The files under `dir` (inside the component at `top`) into `sysroot`, at
/// their path below `top`. The component's `manifest.in` stays out.
fn copy_component(top: &Path, dir: &Path, sysroot: &Path) -> Result<(), String> {
    let io = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| io(dir, e))?
        .collect::<Result<_, _>>()
        .map_err(|e| io(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let from = entry.path();
        let inside = from.strip_prefix(top).map_err(|e| e.to_string())?;
        if inside == Path::new("manifest.in") {
            continue;
        }
        let to = sysroot.join(inside);
        let kind = entry.file_type().map_err(|e| io(&from, e))?;
        if kind.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| io(&to, e))?;
            copy_component(top, &from, sysroot)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(&from).map_err(|e| io(&from, e))?;
            match std::fs::read_link(&to) {
                Ok(existing) if existing == link => {}
                Ok(_) => return Err(format!("{} is given twice, differently", inside.display())),
                Err(_) => std::os::unix::fs::symlink(&link, &to).map_err(|e| io(&to, e))?,
            }
        } else if to.exists() {
            let same = std::fs::read(&from).map_err(|e| io(&from, e))?
                == std::fs::read(&to).map_err(|e| io(&to, e))?;
            if !same {
                return Err(format!("{} is given twice, differently", inside.display()));
            }
        } else {
            gaia_store::copy_file(&from, &to).map_err(|e| io(&to, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release file as unpacked: `components`, and each component's files
    /// beside its `manifest.in`.
    fn release(dir: &Path, components: &[(&str, &[(&str, &str)])]) {
        let names: Vec<&str> = components.iter().map(|(n, _)| *n).collect();
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(dir.join("components"), names.join("\n") + "\n").expect("write");
        std::fs::write(dir.join("install.sh"), "#!/bin/sh\n").expect("write");
        for (name, files) in components {
            let component = dir.join(name);
            std::fs::create_dir_all(&component).expect("mkdir");
            std::fs::write(component.join("manifest.in"), "file:x\n").expect("write");
            for (path, text) in *files {
                let file = component.join(path);
                std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
                std::fs::write(file, text).expect("write");
            }
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).expect("read") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(dir).expect("inside");
                    out.push(rel.display().to_string());
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn the_listed_components_make_one_sysroot_without_the_installers_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rustc = dir.path().join("rustc");
        release(
            &rustc,
            &[(
                "rustc",
                &[("bin/rustc", "rustc"), ("lib/librustc_driver.so", "driver")],
            )],
        );
        let std = dir.path().join("std");
        release(
            &std,
            &[(
                "rust-std-aarch64-unknown-linux-musl",
                &[(
                    "lib/rustlib/aarch64-unknown-linux-musl/lib/libstd.rlib",
                    "std",
                )],
            )],
        );
        let sysroot = dir.path().join("sysroot");
        std::fs::create_dir(&sysroot).expect("mkdir");
        lay_out(&rustc, &sysroot).expect("rustc");
        lay_out(&std, &sysroot).expect("std");
        assert_eq!(
            files(&sysroot),
            [
                "bin/rustc",
                "lib/librustc_driver.so",
                "lib/rustlib/aarch64-unknown-linux-musl/lib/libstd.rlib"
            ]
        );
    }

    #[test]
    fn a_file_given_twice_differently_or_an_unlisted_layout_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let one = dir.path().join("one");
        release(&one, &[("a", &[("lib/x", "one")])]);
        let two = dir.path().join("two");
        release(&two, &[("b", &[("lib/x", "two")])]);
        let sysroot = dir.path().join("sysroot");
        std::fs::create_dir(&sysroot).expect("mkdir");
        lay_out(&one, &sysroot).expect("one");
        let error = lay_out(&two, &sysroot).expect_err("differs");
        assert!(error.contains("lib/x is given twice"), "{error}");
        // The same bytes again are fine.
        lay_out(&one, &sysroot).expect("same bytes");
        let bare = dir.path().join("bare");
        std::fs::create_dir(&bare).expect("mkdir");
        let error = lay_out(&bare, &sysroot).expect_err("no components file");
        assert!(
            error.contains("not an unpacked Rust release file"),
            "{error}"
        );
        std::fs::write(bare.join("components"), "missing\n").expect("write");
        let error = lay_out(&bare, &sysroot).expect_err("a missing component");
        assert!(error.contains("names missing"), "{error}");
    }
}
