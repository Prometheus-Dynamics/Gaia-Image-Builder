//! Recovery from an interrupted make.
//!
//! Buildroot writes a package's stamp only after the step it marks has
//! succeeded, and a finished package has `.stamp_installed`. So a make that
//! is killed leaves every finished package in place, and the next make
//! continues from the first step that has no stamp. That holds for the
//! steps Buildroot records. A step killed half way (a patch half applied, a
//! tree half extracted) has no stamp, but its files are there, and redoing
//! that step on top of them can fail or build something else. So the build
//! directory of a package with no `.stamp_installed` is removed, and that
//! package builds again from the start. Finished packages are not touched.
//!
//! A marker tells the two cases apart. A make writes it before it starts
//! and removes it when it exits on its own, with success or with a build
//! error. A marker still there when the next make starts means the last
//! make was killed.
//!
//! The directory these work on is the make's output directory, which must
//! outlive a killed step: it is not a step's scratch directory, which is
//! removed when the step fails.

use std::fs;
use std::io;
use std::path::Path;

/// The marker a make leaves behind when it is killed.
pub const MARKER: &str = ".gaia-make-running";

/// Written before a make runs in `out`.
pub fn start(out: &Path) -> io::Result<()> {
    fs::create_dir_all(out)?;
    fs::write(out.join(MARKER), "")
}

/// Removed when a make exits on its own.
pub fn finish(out: &Path) -> io::Result<()> {
    match fs::remove_file(out.join(MARKER)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether the last make in `out` was killed.
pub fn was_killed(out: &Path) -> bool {
    out.join(MARKER).is_file()
}

/// The build directories a killed make was in the middle of: those without
/// `.stamp_installed`. Empty when the last make was not killed. Read only.
pub fn in_progress(out: &Path) -> io::Result<Vec<String>> {
    if !was_killed(out) {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    let build = out.join("build");
    if !build.is_dir() {
        return Ok(names);
    }
    for entry in fs::read_dir(&build)? {
        let entry = entry?;
        let dir = entry.path();
        if entry.file_type()?.is_dir() && started(&dir) && !dir.join(".stamp_installed").exists() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// A package build directory a make has taken at least one step of: one
/// with a stamp in it. Other directories under `build/` (Buildroot's own
/// kconfig build, for one) are not packages and are left alone.
fn started(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".stamp_"))
        })
    })
}

/// Removes the build directories [`in_progress`] lists and the marker, so
/// the next make builds those packages from the start. Returns the names
/// removed, sorted; empty when there was nothing to redo.
pub fn redo(out: &Path) -> io::Result<Vec<String>> {
    let names = in_progress(out)?;
    for name in &names {
        fs::remove_dir_all(out.join("build").join(name))?;
    }
    finish(out)?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(out: &Path, dir: &str, files: &[&str]) {
        let build = out.join("build").join(dir);
        fs::create_dir_all(&build).expect("build dir");
        for file in files {
            fs::write(build.join(file), "").expect("stamp");
        }
    }

    #[test]
    fn a_make_that_exits_on_its_own_leaves_nothing_to_redo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("out");
        package(&out, "busybox-1.37.0", &[".stamp_extracted"]);
        start(&out).expect("start");
        finish(&out).expect("finish");
        assert!(!was_killed(&out));
        assert_eq!(redo(&out).expect("redo"), Vec::<String>::new());
        assert!(out.join("build/busybox-1.37.0").is_dir());
    }

    #[test]
    fn a_killed_make_redoes_the_packages_it_was_in_the_middle_of_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("out");
        package(
            &out,
            "host-m4-1.4.21",
            &[".stamp_built", ".stamp_installed"],
        );
        package(
            &out,
            "busybox-1.37.0",
            &[".stamp_extracted", ".stamp_patched"],
        );
        package(&out, "zlib-1.3", &[".stamp_extracted"]);
        // Not a package: no stamp, so it is left alone.
        fs::create_dir_all(out.join("build/buildroot-config")).expect("kconfig dir");
        fs::write(out.join("build/buildroot-config/conf.o"), "").expect("object");
        start(&out).expect("start");
        // The make is killed here: the marker stays.
        assert!(was_killed(&out));
        assert_eq!(
            in_progress(&out).expect("in progress"),
            vec!["busybox-1.37.0".to_string(), "zlib-1.3".to_string()]
        );
        assert_eq!(
            redo(&out).expect("redo"),
            vec!["busybox-1.37.0".to_string(), "zlib-1.3".to_string()]
        );
        assert!(!out.join("build/busybox-1.37.0").exists());
        assert!(!out.join("build/zlib-1.3").exists());
        assert!(out.join("build/buildroot-config/conf.o").is_file());
        assert!(out.join("build/host-m4-1.4.21/.stamp_installed").is_file());
        assert!(!was_killed(&out), "the redo clears the marker");
    }
}
