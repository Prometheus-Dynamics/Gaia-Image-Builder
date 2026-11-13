//! `gaia.build.rust.vendor`: the crates a source's `Cargo.lock` names, as
//! the tree `gaia.build.rust` builds from offline.
//!
//! This is a fetch, like a source step: it runs the toolchain's own
//! `cargo vendor --locked` on this host, with the network. Cargo checks
//! every crate against the checksum `Cargo.lock` records, and a git
//! dependency is taken at the commit `Cargo.lock` records, so the tree is a
//! function of `Cargo.lock`. The key is that file's digest and cargo's
//! version: a source change that leaves `Cargo.lock` alone fetches nothing.
//!
//! The tree is what `cargo vendor` writes (one directory per crate), plus
//! `.cargo-config.toml`: the source replacement cargo prints, with the
//! tree's place inside the build sandbox. `gaia.build.rust` uses that file,
//! so git dependencies are replaced as well as crates.io's.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Stream, Tool};
use gaia_store::file_digest;
use gaia_types::Tree;
use std::path::Path;
use std::sync::{Arc, Mutex};

const NAME: &str = "gaia.build.rust.vendor";
/// The source replacement, in the vendored tree. Cargo skips a dot file in
/// a directory source.
pub(crate) const CONFIG: &str = ".cargo-config.toml";
/// What the directory's place is written as before it is known.
const PLACE: &str = "@vendor@";

/// The crates `source`'s `Cargo.lock` names, fetched by `toolchain`'s cargo.
#[node(
    id = "gaia.build.rust.vendor",
    inputs(store, toolchain, source),
    outputs(vendor)
)]
pub fn vendor(store: &str, toolchain: &str, source: Tree, cx: &Ctx) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |message: String| StepError::body(NAME, message);
    let object = source
        .object
        .object_ref()
        .ok_or_else(|| fail("the source is not in the store".into()))?;
    let source_dir = store.path(&object);
    let lock = source_dir.join("Cargo.lock");
    if !lock.is_file() {
        return Err(fail(
            "the source has no Cargo.lock: commit it, so what is built is what its own tests built"
                .into(),
        ));
    }
    let cargo = Path::new(toolchain).join("bin/cargo");
    if !cargo.is_file() {
        return Err(fail(format!("no cargo at {}", cargo.display())));
    }
    let version = std::process::Command::new(&cargo)
        .arg("-V")
        .env_clear()
        .output()
        .map_err(|e| fail(format!("cargo -V: {e}")))?;
    let lock_digest = file_digest(&lock).map_err(|e| fail(format!("Cargo.lock: {e}")))?;
    let key = KeyBuilder::new(NAME, 1)
        .config("cargo", String::from_utf8_lossy(&version.stdout).trim())
        .input("Cargo.lock", lock_digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("vendor", Path::new("vendor"))],
        |work| {
            let dir = work.dir().to_path_buf();
            let out = dir.join("vendor");
            let home = dir.join("cargo-home");
            std::fs::create_dir_all(&home).map_err(|e| fail(e.to_string()))?;
            let printed = Arc::new(Mutex::new(String::new()));
            let sink = printed.clone();
            let tool = Tool::new(&cargo)
                .args(["vendor", "--locked", "--versioned-dirs", "--manifest-path"])
                .arg(source_dir.join("Cargo.toml"))
                .arg(&out)
                .env("CARGO_HOME", home.display().to_string())
                .env("HOME", dir.display().to_string())
                .env(
                    "PATH",
                    format!("{}/bin:/usr/bin:/bin", Path::new(toolchain).display()),
                )
                .env("CARGO_TERM_COLOR", "never")
                .env("CARGO_NET_GIT_FETCH_WITH_CLI", "false")
                .on_line(move |stream, line| {
                    if stream == Stream::Stdout
                        && let Ok(mut text) = sink.lock()
                    {
                        text.push_str(line);
                        text.push('\n');
                    }
                });
            work.run(&tool)?;
            // A workspace with no dependencies vendors nothing.
            std::fs::create_dir_all(&out).map_err(|e| fail(e.to_string()))?;
            let printed = printed.lock().map(|text| text.clone()).unwrap_or_default();
            let config = config_for(&printed, &out);
            std::fs::write(out.join(CONFIG), config).map_err(|e| fail(e.to_string()))?;
            std::fs::remove_dir_all(&home).map_err(|e| fail(e.to_string()))
        },
    )?;
    Ok(Tree {
        object: made.outputs["vendor"].into(),
        entries: Vec::new(),
    })
}

/// What `cargo vendor` printed, with the directory it wrote to as `@vendor@`.
fn config_for(printed: &str, out: &Path) -> String {
    printed.replace(&out.display().to_string(), PLACE)
}

/// The vendored tree's own source replacement, with the tree at `at`.
/// None for a tree with no such file (one made by hand).
pub(crate) fn config_in(vendor: &Path, at: &str) -> Option<String> {
    std::fs::read_to_string(vendor.join(CONFIG))
        .ok()
        .map(|text| text.replace(PLACE, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_printed_config_names_the_tree_by_its_place_in_the_sandbox() {
        let printed = "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"/s/scratch/1/vendor\"\n";
        let config = config_for(printed, Path::new("/s/scratch/1/vendor"));
        assert!(config.contains("directory = \"@vendor@\""), "{config}");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(CONFIG), &config).expect("write");
        let used = config_in(dir.path(), "/vendor").expect("a config");
        assert!(used.contains("directory = \"/vendor\""), "{used}");
        assert!(used.contains("[source.crates-io]"));
        assert_eq!(config_in(Path::new("/nonexistent"), "/vendor"), None);
    }
}
