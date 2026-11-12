//! `gaia.build.node`: a Node.js package built into a bundle.
//!
//! - **Pinned Node.js:** the install at the path given (`bin/node`,
//!   `bin/npm`); `node --version` and `npm --version` are in the key. npm
//!   runs with a cleared environment, its own `HOME` and cache.
//! - **Offline, from the lock:** `npm ci --offline` installs exactly what
//!   `package-lock.json` names, from the `cache` tree: an npm cache
//!   (`npm cache add` of each locked tarball) that a source node fills from
//!   the lock. A source without `package-lock.json` is refused. Dependency
//!   install scripts do not run (`--ignore-scripts`), so a dependency that
//!   needs a native build is not supported here.
//! - **Keyed on the lock, not the cache:** the key holds each
//!   `package-lock.json` entry's path, version, `resolved` and `integrity`,
//!   which `npm ci` checks every tarball against. The cache tree's own
//!   digest is not in the key: npm writes times into its index, so two
//!   caches holding the same tarballs differ byte for byte.
//! - **The build:** the package's own script (`npm run <script>`), when
//!   one is named, then `npm prune --omit=dev`.
//! - **Reproducible:** `SOURCE_DATE_EPOCH` is 0; the store keeps content
//!   and modes, not times, and npm writes no time into what it installs.
//!
//! Outputs: the bundle (the package with its built files and production
//! `node_modules`) as a tree, and the package's `npm pack` tarball as an
//! artifact.

use daedalus::prelude::*;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Tool, once_per_process};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const NAME: &str = "gaia.build.node";
const RECIPE_VERSION: u32 = 2;

/// Installs `source`'s locked dependencies from `cache` with the Node.js at
/// `node`, runs its `script` (none when empty) and prunes dev dependencies.
#[node(
    id = "gaia.build.node",
    inputs(store, node, source, cache, script),
    outputs(bundle, package)
)]
pub fn build_node(
    store: &str,
    node: &str,
    source: Tree,
    cache: Tree,
    script: &str,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let node = Path::new(node);
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| StepError::body(NAME, format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (_, cache_dir) = dir_of(&cache, "npm cache tree")?;
    let lock = source_dir.join("package-lock.json");
    if !lock.is_file() {
        return Err(StepError::body(
            NAME,
            "the source has no package-lock.json; the install is locked",
        ));
    }
    let locked = std::fs::read(&lock)
        .map_err(|e| StepError::body(NAME, format!("package-lock.json: {e}")))
        .and_then(|bytes| locked(&bytes).map_err(|e| StepError::body(NAME, e)))?;
    let version = |program: &str| {
        let path = node.join("bin").join(program);
        once_per_process(&path, || {
            std::process::Command::new(&path)
                .arg("--version")
                .env_clear()
                .env("PATH", node.join("bin"))
                .output()
                .map(|o| o.stdout)
        })
        .map_err(|e| StepError::body(NAME, format!("{program} --version: {e}")))
    };
    let key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("node", version("node")?)
        .config("npm", version("npm")?)
        .config("script", script)
        .input("source", source_digest)
        .input("locked", Digest::of_bytes(locked.as_bytes()))
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("bundle", Path::new("bundle")),
            ("package", Path::new("package.tgz")),
        ],
        |work| {
            let fail = |e: std::io::Error| StepError::body(NAME, e);
            let app = work.dir().join("bundle");
            let npm_cache = work.dir().join("cache");
            let home = work.dir().join("home");
            let packed = work.dir().join("packed");
            copy_writable(&source_dir, &app).map_err(fail)?;
            copy_writable(&cache_dir, &npm_cache).map_err(fail)?;
            for dir in [&home, &packed] {
                std::fs::create_dir_all(dir).map_err(fail)?;
            }
            let npm = |args: &[&str]| {
                Tool::new(node.join("bin/npm"))
                    .args(args)
                    .args([
                        "--offline",
                        "--no-audit",
                        "--no-fund",
                        "--no-update-notifier",
                    ])
                    .arg("--cache")
                    .arg(&npm_cache)
                    .env(
                        "PATH",
                        format!("{}:/usr/bin:/bin", node.join("bin").display()),
                    )
                    .env("HOME", &home)
                    .env("SOURCE_DATE_EPOCH", "0")
                    .env("TZ", "UTC")
                    .env("LC_ALL", "C")
                    .env("NODE_ENV", "production")
                    .cwd(&app)
            };
            work.run(&npm(&["ci", "--ignore-scripts", "--include=dev"]))?;
            if !script.is_empty() {
                work.run(&npm(&["run", script]))?;
            }
            work.run(&npm(&["prune", "--omit=dev", "--ignore-scripts"]))?;
            let mut pack = npm(&["pack", "--ignore-scripts"]);
            pack = pack.arg("--pack-destination").arg(&packed);
            work.run(&pack)?;
            let mut tarballs: Vec<_> = std::fs::read_dir(&packed)
                .map_err(fail)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            if tarballs.len() != 1 {
                return Err(StepError::body(NAME, "npm pack made no single tarball"));
            }
            std::fs::rename(tarballs.remove(0), work.dir().join("package.tgz")).map_err(fail)
        },
    )?;
    Ok((
        Tree {
            object: made.outputs["bundle"].into(),
            entries: Vec::new(),
        },
        Artifact {
            kind: ArtifactKind::Package,
            object: made.outputs["package"].into(),
        },
    ))
}

/// What `package-lock.json` pins, one line per package in path order:
/// `path`, `version`, `resolved`, `integrity` and `link`, tab-separated, a
/// missing field empty. The root package (`""`) is left out; the source's
/// digest covers it. Lock files without `packages` (lockfileVersion 1) are
/// refused.
fn locked(bytes: &[u8]) -> Result<String, String> {
    let lock: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("package-lock.json: {e}"))?;
    let packages = lock.get("packages").and_then(|p| p.as_object()).ok_or(
        "package-lock.json has no `packages` (lockfileVersion 1); lock with npm 7 or later",
    )?;
    let mut paths: Vec<&String> = packages.keys().filter(|p| !p.is_empty()).collect();
    paths.sort();
    let mut out = String::new();
    for path in paths {
        let entry = &packages[path.as_str()];
        let field = |name: &str| match entry.get(name) {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Bool(flag)) => flag.to_string(),
            _ => String::new(),
        };
        for text in [
            path.as_str(),
            &field("version"),
            &field("resolved"),
            &field("integrity"),
            &field("link"),
        ] {
            if text.contains(['\t', '\n']) {
                return Err(format!(
                    "package-lock.json: {path:?} has a tab or newline in a field"
                ));
            }
        }
        out.push_str(&format!(
            "{path}\t{}\t{}\t{}\t{}\n",
            field("version"),
            field("resolved"),
            field("integrity"),
            field("link")
        ));
    }
    Ok(out)
}

/// Copies `from` to `to` (which must not exist), every file and directory
/// writable by its owner, symlinks as links.
fn copy_writable(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_writable(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            let mode = std::fs::metadata(&src)?.permissions().mode() | 0o200;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode & 0o7777))?;
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_NODE_NODE];

#[cfg(test)]
mod tests {
    use super::locked;

    const LOCK: &str = r#"{"lockfileVersion":3,"packages":{
        "":{"name":"app","version":"1.0.0"},
        "node_modules/b":{"version":"2.0.0","resolved":"https://r/b-2.0.0.tgz","integrity":"sha512-B"},
        "node_modules/a":{"version":"1.0.0","resolved":"https://r/a-1.0.0.tgz","integrity":"sha512-A"},
        "node_modules/l":{"resolved":"../l","link":true}}}"#;

    #[test]
    fn the_lock_key_is_every_locked_package_in_path_order() {
        let text = locked(LOCK.as_bytes()).expect("parses");
        assert_eq!(
            text,
            "node_modules/a\t1.0.0\thttps://r/a-1.0.0.tgz\tsha512-A\t\n\
             node_modules/b\t2.0.0\thttps://r/b-2.0.0.tgz\tsha512-B\t\n\
             node_modules/l\t\t../l\t\ttrue\n"
        );
        // Another tarball for the same version is another key.
        let other = LOCK.replace("sha512-A", "sha512-X");
        assert_ne!(locked(other.as_bytes()).expect("parses"), text);
        // The root's own fields are the source's, not the lock key's.
        let root = LOCK.replace(r#""version":"1.0.0"}"#, r#""version":"9.9.9"}"#);
        assert_eq!(locked(root.as_bytes()).expect("parses"), text);
    }

    #[test]
    fn a_lock_without_packages_is_refused() {
        let error = locked(br#"{"lockfileVersion":1,"dependencies":{}}"#).expect_err("v1");
        assert!(error.contains("lockfileVersion 1"), "{error}");
    }
}
