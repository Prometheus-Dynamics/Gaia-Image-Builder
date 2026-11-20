//! The download pool: Buildroot's downloads kept by their sha256, shared by
//! every store and every key on the machine, so a new store or a new key
//! does not fetch again what one fetched before.
//!
//! Buildroot reads a source at `<download dir>/<package>/<file>` and checks
//! it against a `sha256 <hex> <file>` line of the package's `.hash` file.
//! The pool is a flat directory of such files named by that hex. Before a
//! step's makes run, every file a `.hash` line of the Buildroot tree names
//! that the pool has and the download directory does not is linked (or,
//! across file systems, copied) into the download directory; Buildroot then
//! finds it and checks it as it would a file it fetched. After the makes,
//! and before them for what an earlier step left, every such file in the
//! download directory the pool does not have is hashed and, when the hash
//! is its line's, linked into the pool. A file is never put in the pool
//! under a hash it does not have.
//!
//! A package's download subdirectory is taken to be its `.hash` file's name
//! (`package/gcc/14.2.0/gcc.hash` is `gcc`), which is Buildroot's rule but
//! for the few packages that name another; for those the pool does nothing
//! and Buildroot fetches as before.
//!
//! A download that fails Buildroot's hash check is named in the step's error
//! with the hashes Buildroot printed ([`Mismatches`]); Buildroot removes the
//! file, so nothing wrong is left in the download directory or the pool.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// The step detail the pool's work is recorded under.
pub(crate) const DETAIL: &str = "download-pool";
/// The detail for what a step's own make fetched, kept after it.
pub(crate) const AFTER: &str = "download-pool-after";

/// What the `.hash` files of a Buildroot tree name: for each download
/// subdirectory and file, the sha256 hashes it may have.
#[derive(Debug, Default)]
pub(crate) struct Wanted {
    files: BTreeMap<(String, String), BTreeSet<String>>,
}

/// Reads every `*.hash` file under `src`.
pub(crate) fn wanted(src: &Path) -> io::Result<Wanted> {
    let mut wanted = Wanted::default();
    let mut dirs = vec![src.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if !name.starts_with('.') {
                    dirs.push(entry.path());
                }
                continue;
            }
            let Some(package) = name.strip_suffix(".hash") else {
                continue;
            };
            if !kind.is_file() || package.is_empty() {
                continue;
            }
            let text = fs::read_to_string(entry.path())?;
            for line in text.lines() {
                let words: Vec<&str> = line.split_whitespace().collect();
                if let ["sha256", hex, file] = words.as_slice()
                    && hex.len() == 64
                    && hex.bytes().all(|b| b.is_ascii_hexdigit())
                    && !file.contains('/')
                    && !file.starts_with('.')
                {
                    wanted
                        .files
                        .entry((package.to_string(), file.to_string()))
                        .or_default()
                        .insert(hex.to_ascii_lowercase());
                }
            }
        }
    }
    Ok(wanted)
}

/// What one pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Moved {
    pub files: u32,
    pub bytes: u64,
}

impl Moved {
    fn add(&mut self, path: &Path) {
        self.files += 1;
        self.bytes += fs::metadata(path).map_or(0, |m| m.len());
    }
}

/// Puts the download directory's files the pool lacks into the pool, each
/// under the hash it has when that is one its line names.
pub(crate) fn harvest(wanted: &Wanted, dl: &Path, pool: &Path) -> io::Result<Moved> {
    let mut moved = Moved::default();
    for ((package, file), hexes) in &wanted.files {
        let path = dl.join(package).join(file);
        if hexes.iter().any(|hex| pool.join(hex).exists()) || !path.is_file() {
            continue;
        }
        let Ok(digest) = gaia_store::file_digest(&path) else {
            continue;
        };
        let hex = digest.hex();
        if hexes.contains(&hex) {
            fs::create_dir_all(pool)?;
            if put(&path, &pool.join(&hex))? {
                moved.add(&path);
            }
        }
    }
    Ok(moved)
}

/// Puts the pool's files the download directory lacks into it.
pub(crate) fn seed(wanted: &Wanted, dl: &Path, pool: &Path) -> io::Result<Moved> {
    let mut moved = Moved::default();
    for ((package, file), hexes) in &wanted.files {
        let path = dl.join(package).join(file);
        if fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        let Some(from) = hexes.iter().map(|hex| pool.join(hex)).find(|p| p.is_file()) else {
            continue;
        };
        fs::create_dir_all(dl.join(package))?;
        if put(&from, &path)? {
            moved.add(&path);
        }
    }
    Ok(moved)
}

/// Links `from` at `to`, or copies it there (through a temporary name, so
/// `to` is whole or absent) when they are on different file systems. False
/// when `to` was there already.
fn put(from: &Path, to: &Path) -> io::Result<bool> {
    match fs::hard_link(from, to) {
        Ok(()) => return Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) if e.kind() != io::ErrorKind::CrossesDevices => return Err(e),
        Err(_) => {}
    }
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp: PathBuf = to.with_file_name(format!(".{name}.{}", std::process::id()));
    gaia_store::copy_file(from, &temp)?;
    match fs::hard_link(&temp, to) {
        Ok(()) => {
            fs::remove_file(&temp)?;
            Ok(true)
        }
        Err(e) => {
            let _ = fs::remove_file(&temp);
            if e.kind() == io::ErrorKind::AlreadyExists {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// Before a step's makes: what the download directory has that the pool
/// lacks into the pool (an earlier step's downloads), then the pool's files
/// it lacks into it. Nothing without a pool.
pub(crate) fn before(
    step: &'static str,
    work: &mut gaia_step::Work<'_>,
    src: &Path,
    host: &gaia_types::BuildrootHost,
) -> Result<(), gaia_step::StepError> {
    if host.download_pool.is_empty() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let (dl, pool) = (
        Path::new(&host.download_dir),
        Path::new(&host.download_pool),
    );
    let fail = |e: io::Error| {
        gaia_step::StepError::body(step, format!("the download pool {}: {e}", pool.display()))
    };
    let wanted = wanted(src).map_err(fail)?;
    let harvested = harvest(&wanted, dl, pool).map_err(fail)?;
    let seeded = seed(&wanted, dl, pool).map_err(fail)?;
    work.detail(
        DETAIL,
        serde_json::json!({
            "seeded": seeded.files,
            "seeded_bytes": seeded.bytes,
            "kept": harvested.files,
            "kept_bytes": harvested.bytes,
            "seconds": started.elapsed().as_secs_f64(),
        }),
    );
    Ok(())
}

/// After a step's make: its downloads into the pool, and, when the make
/// failed, the downloads Buildroot's hash check refused named in its error.
pub(crate) fn after<T>(
    step: &'static str,
    work: &mut gaia_step::Work<'_>,
    src: &Path,
    host: &gaia_types::BuildrootHost,
    seen: &Mismatches,
    made: Result<T, gaia_step::StepError>,
) -> Result<T, gaia_step::StepError> {
    let made = made.map_err(|error| match seen.named() {
        Some(named) => gaia_step::StepError::body(step, format!("{named}\n{error}")),
        None => error,
    });
    if host.download_pool.is_empty() {
        return made;
    }
    let started = std::time::Instant::now();
    let (dl, pool) = (
        Path::new(&host.download_dir),
        Path::new(&host.download_pool),
    );
    let kept = wanted(src).and_then(|wanted| harvest(&wanted, dl, pool));
    match kept {
        Ok(kept) => work.detail(
            AFTER,
            serde_json::json!({
                "kept": kept.files,
                "kept_bytes": kept.bytes,
                "seconds": started.elapsed().as_secs_f64(),
            }),
        ),
        // The make's own failure is the one to report.
        Err(e) if made.is_ok() => {
            return Err(gaia_step::StepError::body(
                step,
                format!("the download pool {}: {e}", pool.display()),
            ));
        }
        Err(_) => {}
    }
    made
}

/// The lines of Buildroot's hash check failing, from a make's output.
#[derive(Clone, Debug, Default)]
pub(crate) struct Mismatches(Arc<Mutex<Vec<String>>>);

impl Mismatches {
    /// Takes one line the make printed.
    pub(crate) fn line(&self, line: &str) {
        let Some(at) = line.find("ERROR: ") else {
            return;
        };
        let text = line[at + "ERROR: ".len()..].trim();
        let wanted = text.contains("has wrong sha256 hash")
            || text.starts_with("expected:")
            || text.starts_with("got ")
            || text.starts_with("got:");
        if wanted {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(text.to_string());
        }
    }

    /// What to add to a failed make's error: the files Buildroot refused,
    /// with the hashes, or nothing.
    pub(crate) fn named(&self) -> Option<String> {
        let lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (!lines.is_empty()).then(|| {
            format!(
                "a download failed Buildroot's hash check (the site served another file; \
                 Buildroot removed it, and nothing was kept):\n  {}",
                lines.join("\n  ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_of(bytes: &[u8]) -> String {
        gaia_store::Digest::of_bytes(bytes).hex()
    }

    fn tree(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }
    }

    /// A file fetched in one store is in the next store's download
    /// directory before its make runs; one whose bytes are not its line's
    /// never reaches the pool.
    #[test]
    fn a_download_reaches_another_store_through_the_pool_by_its_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (src, pool) = (dir.path().join("buildroot"), dir.path().join("pool"));
        let (one, two) = (dir.path().join("one/dl"), dir.path().join("two/dl"));
        let good = hex_of(b"zlib source");
        tree(
            &src,
            &[
                (
                    "package/zlib/zlib.hash",
                    &format!(
                        "# comment\nsha256  {good}  zlib-1.3.tar.xz\nsha256  {}  LICENSE\n",
                        hex_of(b"license")
                    ),
                ),
                (
                    "package/gcc/14.2.0/gcc.hash",
                    &format!(
                        "sha256  {}  gcc-14.2.0.tar.xz\nmd5 abc gcc.tar\n",
                        hex_of(b"gcc")
                    ),
                ),
                (
                    "package/bad/bad.hash",
                    &format!("sha256  {}  bad-1.tar.gz\n", "a".repeat(64)),
                ),
            ],
        );
        tree(
            &one,
            &[
                ("zlib/zlib-1.3.tar.xz", "zlib source"),
                ("gcc/gcc-14.2.0.tar.xz", "gcc"),
                ("bad/bad-1.tar.gz", "not what its line says"),
                ("other/unlisted.tar", "x"),
            ],
        );
        let wanted = wanted(&src).expect("reads");
        let harvested = harvest(&wanted, &one, &pool).expect("harvest");
        assert_eq!(harvested.files, 2);
        let mut pooled: Vec<String> = fs::read_dir(&pool)
            .expect("pool")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        pooled.sort();
        let mut want = vec![good.clone(), hex_of(b"gcc")];
        want.sort();
        assert_eq!(pooled, want, "by hash, and the bad file is not there");
        assert_eq!(harvest(&wanted, &one, &pool).expect("again").files, 0);

        let seeded = seed(&wanted, &two, &pool).expect("seed");
        assert_eq!(seeded.files, 2);
        assert_eq!(
            fs::read(two.join("zlib/zlib-1.3.tar.xz")).expect("seeded"),
            b"zlib source"
        );
        assert_eq!(
            fs::read(two.join("gcc/gcc-14.2.0.tar.xz")).expect("seeded"),
            b"gcc"
        );
        assert!(!two.join("bad").exists());
        assert!(!two.join("zlib/LICENSE").exists(), "only what the pool has");
        assert_eq!(seed(&wanted, &two, &pool).expect("again").files, 0);
    }

    #[test]
    fn a_wrong_hash_is_named() {
        let seen = Mismatches::default();
        for line in [
            ">>> foo 1.0 Downloading",
            "ERROR: while checking hashes from package/foo/foo.hash",
            "ERROR: foo-1.0.tar.gz has wrong sha256 hash:",
            "ERROR: expected: 1111",
            "ERROR: got     : 2222",
            "ERROR: Incomplete download, or man-in-the-middle (MITM) attack",
        ] {
            seen.line(line);
        }
        let named = seen.named().expect("named");
        println!("{named}");
        assert!(
            named.contains(
                "foo-1.0.tar.gz has wrong sha256 hash:\n  expected: 1111\n  got     : 2222"
            )
        );
        assert!(Mismatches::default().named().is_none());
    }
}
