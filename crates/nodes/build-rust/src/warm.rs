//! Dev reuse (`gaia build --dev-reuse`): a cargo target directory kept
//! between dev builds, so a change rebuilds the crates it touches and not
//! every dependency and every package of the step.
//!
//! **For bench iteration only.** A clean build, and every release, starts
//! from an empty target directory, as it always has. The dev build's step
//! key carries `build = dev-reuse <VERSION>`, so what a dev build makes is
//! never a clean build's in the store, and a clean build never finds it.
//!
//! - **Where:** one warm directory for each identity, under the host's dev
//!   root (`<store>/dev-cargo/<identity>`), outside the content store, held
//!   under an exclusive lock while a build uses it. A second build of the
//!   same identity is refused, not queued.
//! - **Identity:** the step's key without the source: the toolchain, the
//!   image, the vendored crates (so `Cargo.lock`'s dependencies), the
//!   packages, the profile, the features, the target, the CPU, the tools
//!   and the sandbox's environment. Any of them changed is another directory.
//! - **The source:** cargo decides what is stale by modification times, and
//!   a store tree's times say nothing of what changed (an exported revision
//!   carries its commit's time, older than the last build). So the warm
//!   directory holds its own copy of the source (`src`): a file whose bytes,
//!   mode or link are as the last build had them is kept, with its time;
//!   any other is written now, and what the source no longer has is
//!   removed. That copy is bound at `/src`, where the source is in a clean
//!   build, so cargo sees the same paths and exactly what changed.
//! - **The target:** `target` is bound writable where a clean build's
//!   target is (the step's directory may be in RAM, on another
//!   filesystem), and stays there after the build, whether it worked or
//!   not: cargo keeps its own state of what a failed build left.
//!
//! - **The profile:** a dev build also builds with the dev profile
//!   ([`dev_profile`]): no LTO and 16 codegen units. That is code a clean
//!   build does not make, so a dev binary is for the inner loop only. The
//!   landing gate, and any image flashed for a proof or a release, are
//!   clean builds with the source's own profile.
//!
//! `gaia build --dev-reuse --verify` builds the product clean beside it and
//! compares the outputs; a stale reuse shows there, and with the dev
//! profile the Rust binaries differ by design.
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// What a dev build does, as a number in its step key and its warm
/// directory's identity. Raised when that changes. 2: the dev profile.
pub const VERSION: u32 = 2;

/// The dev profile, in words: what a dev build changes in the package's
/// cargo profile, and nothing else. It is in the dev step's key, on stderr
/// for each dev Rust step, and in every dev output's marker.
pub const DEV_PROFILE_TEXT: &str = "lto off, codegen-units 16";

/// The prefix of a `dev` value that keeps the source's profile:
/// `source-profile <root>` (`gaia build --dev-reuse --verify`).
pub const SOURCE_PROFILE: &str = "source-profile ";

/// The dev profile as cargo's environment for profile `profile`: no LTO and
/// 16 codegen units, so a one-line change recompiles and links one crate in
/// seconds. Everything else in the profile (opt-level, panic, strip) is the
/// source's. A clean build, and so every image that is judged, uses the
/// profile as the source sets it.
pub fn dev_profile(profile: &str) -> [(String, &'static str); 2] {
    let name = profile.to_ascii_uppercase().replace('-', "_");
    [
        (format!("CARGO_PROFILE_{name}_LTO"), "off"),
        (format!("CARGO_PROFILE_{name}_CODEGEN_UNITS"), "16"),
    ]
}
/// The lock's file in a warm directory.
const LOCK: &str = "lock";

/// A warm directory, locked while this is held.
#[derive(Debug)]
pub struct Warm {
    dir: PathBuf,
    _lock: fs::File,
}

impl Warm {
    /// The warm directory of `identity` under `root`, locked. Refused when
    /// another build holds it.
    pub fn open(root: &Path, identity: &str) -> io::Result<Self> {
        let dir = root.join(identity);
        fs::create_dir_all(&dir)?;
        let lock = fs::File::create(dir.join(LOCK))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |_| {
                io::Error::other(format!(
                    "another dev build is using {}; dev builds of one identity run one at a time",
                    dir.display()
                ))
            },
        )?;
        Ok(Self { dir, _lock: lock })
    }

    /// The kept cargo target directory.
    pub fn target(&self) -> PathBuf {
        self.dir.join("target")
    }

    /// The copy of the source the build sees, made like `source`: see the
    /// module's notes. Returns it and how many files were written anew.
    pub fn source(&self, source: &Path) -> io::Result<(PathBuf, usize)> {
        let copy = self.dir.join("src");
        fs::create_dir_all(&copy)?;
        let written = mirror(source, &copy)?;
        Ok((copy, written))
    }
}

/// Makes `copy` hold what `source` holds: an entry the same in kind, mode
/// and bytes (or link) is left alone, with its time; any other is written.
/// Returns how many files and links were written.
pub fn mirror(source: &Path, copy: &Path) -> io::Result<usize> {
    let mut written = 0;
    let mut wanted = std::collections::BTreeSet::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        wanted.insert(name.clone());
        let (from, to) = (entry.path(), copy.join(&name));
        let kind = entry.file_type()?;
        let have = fs::symlink_metadata(&to).ok();
        if kind.is_dir() {
            if have.as_ref().is_some_and(|m| !m.is_dir()) {
                fs::remove_file(&to)?;
            }
            if !to.is_dir() {
                fs::create_dir(&to)?;
            }
            let mode = fs::metadata(&from)?.permissions().mode();
            if fs::metadata(&to)?.permissions().mode() != mode {
                fs::set_permissions(&to, fs::Permissions::from_mode(mode))?;
            }
            written += mirror(&from, &to)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&from)?;
            if have.as_ref().is_some_and(|m| m.is_symlink()) && fs::read_link(&to)? == target {
                continue;
            }
            remove_any(&to, have.as_ref())?;
            std::os::unix::fs::symlink(&target, &to)?;
            written += 1;
        } else {
            let meta = fs::metadata(&from)?;
            if have.as_ref().is_some_and(|m| {
                m.is_file() && m.len() == meta.len() && m.mode() & 0o7777 == meta.mode() & 0o7777
            }) && same_bytes(&from, &to)?
            {
                continue;
            }
            remove_any(&to, have.as_ref())?;
            // A new file, dated now, after anything cargo built (a reflink
            // may carry the source's time).
            gaia_store::copy_file(&from, &to)?;
            fs::set_permissions(&to, fs::Permissions::from_mode(meta.mode() & 0o7777))?;
            fs::File::options()
                .write(true)
                .open(&to)?
                .set_modified(std::time::SystemTime::now())?;
            written += 1;
        }
    }
    for entry in fs::read_dir(copy)? {
        let entry = entry?;
        if !wanted.contains(&entry.file_name()) {
            remove_any(&entry.path(), Some(&entry.metadata()?))?;
        }
    }
    Ok(written)
}

fn remove_any(path: &Path, meta: Option<&fs::Metadata>) -> io::Result<()> {
    match meta {
        None => Ok(()),
        Some(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Some(_) => fs::remove_file(path),
    }
}

fn same_bytes(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = read_full(&mut a, &mut x)?;
        let m = read_full(&mut b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(file: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn mtime(path: &Path) -> SystemTime {
        fs::symlink_metadata(path)
            .expect("meta")
            .modified()
            .expect("mtime")
    }

    /// An unchanged file keeps its time; a changed one, even with an older
    /// time in the source, is written now; a removed one goes; links and
    /// modes follow.
    #[test]
    fn the_copy_changes_only_what_changed_and_dates_it_now() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (src, copy) = (dir.path().join("src"), dir.path().join("copy"));
        fs::create_dir_all(src.join("a/b")).expect("mkdir");
        fs::write(src.join("a/b/same.rs"), "same").expect("write");
        fs::write(src.join("a/changed.rs"), "one").expect("write");
        fs::write(src.join("gone.rs"), "gone").expect("write");
        std::os::unix::fs::symlink("a/changed.rs", src.join("link")).expect("link");
        fs::create_dir(&copy).expect("mkdir");
        assert_eq!(mirror(&src, &copy).expect("first"), 4);
        let old = SystemTime::now() - Duration::from_secs(3600);
        for path in ["a/b/same.rs", "a/changed.rs"] {
            fs::File::options()
                .write(true)
                .open(copy.join(path))
                .expect("open")
                .set_modified(old)
                .expect("set");
        }
        // The source's new revision: one file changed (same length, and an
        // old time, as an export gives), one gone, one new, a link moved, a
        // mode changed.
        fs::write(src.join("a/changed.rs"), "two").expect("write");
        fs::File::options()
            .write(true)
            .open(src.join("a/changed.rs"))
            .expect("open")
            .set_modified(old - Duration::from_secs(3600))
            .expect("set");
        fs::remove_file(src.join("gone.rs")).expect("rm");
        fs::write(src.join("new.rs"), "new").expect("write");
        fs::remove_file(src.join("link")).expect("rm");
        std::os::unix::fs::symlink("a/b/same.rs", src.join("link")).expect("link");
        fs::set_permissions(src.join("a/b/same.rs"), fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let written = mirror(&src, &copy).expect("second");
        assert_eq!(written, 4, "changed, new, link, mode");
        assert_eq!(
            fs::read_to_string(copy.join("a/changed.rs")).expect("read"),
            "two"
        );
        assert!(
            mtime(&copy.join("a/changed.rs")) > old,
            "a changed file is dated now"
        );
        assert!(!copy.join("gone.rs").exists());
        assert_eq!(
            fs::read_to_string(copy.join("new.rs")).expect("read"),
            "new"
        );
        assert_eq!(
            fs::read_link(copy.join("link")).expect("link"),
            Path::new("a/b/same.rs")
        );
        assert_eq!(
            fs::metadata(copy.join("a/b/same.rs")).expect("meta").mode() & 0o777,
            0o755
        );
        // Nothing changed: nothing written, every time kept.
        fs::File::options()
            .write(true)
            .open(copy.join("a/changed.rs"))
            .expect("open")
            .set_modified(old)
            .expect("set");
        assert_eq!(mirror(&src, &copy).expect("third"), 0);
        assert_eq!(mtime(&copy.join("a/changed.rs")), old);
    }

    /// The dev profile names the package's own profile, as cargo spells
    /// its environment.
    #[test]
    fn the_dev_profile_is_cargos_environment_for_the_named_profile() {
        assert_eq!(
            dev_profile("service"),
            [
                ("CARGO_PROFILE_SERVICE_LTO".to_string(), "off"),
                ("CARGO_PROFILE_SERVICE_CODEGEN_UNITS".to_string(), "16"),
            ]
        );
        assert_eq!(
            dev_profile("small-fast")[0].0,
            "CARGO_PROFILE_SMALL_FAST_LTO"
        );
    }

    /// One build of an identity at a time.
    #[test]
    fn a_second_build_of_one_identity_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = Warm::open(dir.path(), "id").expect("first");
        let error = Warm::open(dir.path(), "id").expect_err("refused");
        assert!(
            error.to_string().contains("another dev build is using"),
            "{error}"
        );
        drop(first);
        Warm::open(dir.path(), "id").expect("free again");
    }
}
