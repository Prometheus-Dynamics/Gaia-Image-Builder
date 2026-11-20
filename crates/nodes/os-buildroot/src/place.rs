//! Where a Buildroot build runs, as the build sees it.
//!
//! Buildroot writes its output directory's path into what it makes: dbus's
//! user unit names `<out>/per-package/dbus/host/bin/systemctl`, the
//! libstdc++ gdb helper names the toolchain's directory, udev's `hwdb.bin`
//! names its source files, and a path's length moves what a binary links
//! after it. A clean build runs in a directory of its own (in RAM, named for
//! the run), a dev build in its warm directory, each store in another place:
//! left to itself, each would make a different image from the same inputs.
//!
//! So every make runs under `bwrap` (bubblewrap), with the host's own root as
//! it is and the real output directory bound at [`FIXED`]: the same path for
//! every build on every machine. The bind is in a mount namespace of the
//! make's own, so builds that run at once each see their own directory
//! there. Files land in the real directory, owned by the caller.
//!
//! What Gaia writes or reads as the build's path (the host tools' seed, the
//! toolchain's marked paths) is [`seen`], the path the build saw.
use gaia_types::BuildrootHost;
use std::path::{Path, PathBuf};

/// The output directory's path inside every build.
pub(crate) const FIXED: &str = "/dev/shm/gaia-buildroot/out";

/// The path a build in `out` sees: [`FIXED`], or `out` itself where no
/// `bwrap` is given (a build whose files then name its own directory).
pub(crate) fn seen(host: &BuildrootHost, out: &Path) -> PathBuf {
    if host.bwrap.is_empty() {
        out.to_path_buf()
    } else {
        PathBuf::from(FIXED)
    }
}

/// `words`, a command run in `out`, wrapped so that it sees `out` at
/// [`FIXED`]. Unchanged where no `bwrap` is given.
pub(crate) fn wrapped(host: &BuildrootHost, out: &Path, words: Vec<String>) -> Vec<String> {
    if host.bwrap.is_empty() {
        return words;
    }
    // The mount point. Empty, and never removed: each build binds over it
    // in its own namespace.
    let _ = std::fs::create_dir_all(FIXED);
    let mut wrapped = vec![
        host.bwrap.clone(),
        "--dev-bind".into(),
        "/".into(),
        "/".into(),
        "--bind".into(),
        out.display().to_string(),
        FIXED.into(),
        "--die-with-parent".into(),
        "--".into(),
    ];
    wrapped.extend(words);
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(bwrap: &str) -> BuildrootHost {
        BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: String::new(),
            cpus: String::new(),
            jobs: 1,
            download_dir: "/dl".into(),
            download_pool: String::new(),
            ccache_dir: String::new(),
            mirror: String::new(),
            config_only: false,
            dev_reuse: String::new(),
            bwrap: bwrap.into(),
        }
    }

    #[test]
    fn a_build_sees_the_fixed_path_and_writes_the_real_one() {
        let out = Path::new("/dev/shm/gaia-1234/run-7/out");
        let bound = host("/usr/bin/bwrap");
        assert_eq!(seen(&bound, out), Path::new(FIXED));
        let words = wrapped(&bound, out, vec!["make".into(), format!("O={FIXED}")]);
        assert_eq!(words[0], "/usr/bin/bwrap");
        let bind = words.iter().position(|w| w == "--bind").expect("a bind");
        assert_eq!(words[bind + 1], out.display().to_string());
        assert_eq!(words[bind + 2], FIXED);
        let tail = words.iter().position(|w| w == "--").expect("the command");
        assert_eq!(
            words[tail + 1..],
            ["make".to_string(), format!("O={FIXED}")]
        );

        let bare = host("");
        assert_eq!(seen(&bare, out), out);
        assert_eq!(wrapped(&bare, out, vec!["make".into()]), ["make"]);
    }
}
