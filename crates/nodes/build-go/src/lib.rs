//! `gaia.build.go`: a Go module built into binaries for a target.
//!
//! - **Pinned toolchain:** the Go root given (`GOROOT`, with
//!   `GOTOOLCHAIN=local`, so no other toolchain is fetched); its
//!   `go version` is in the key.
//! - **Offline, from the lock:** a module with `vendor/modules.txt` builds
//!   with `-mod=vendor`; otherwise modules come from the module cache tree
//!   given (as `go mod download` fills it), with `GOPROXY=off`, so a
//!   module missing from it is an error rather than a download. `go.sum`
//!   is still checked.
//! - **For a target:** `GOOS`, `GOARCH` and `GOARM` from the target triple
//!   (`aarch64-unknown-linux-musl` is `linux/arm64`, `armv7-…-gnueabihf` is
//!   `linux/arm` with `GOARM=7`). `CGO_ENABLED=0`: static binaries, no C
//!   toolchain.
//! - **Reproducible and lean:** `-trimpath -buildvcs=false` and
//!   `-ldflags=-s -w -buildid=`, with the build and module caches in the
//!   step's own directory, so two stores give the same bytes.
//!
//! The outputs are the binaries (`bin/<name>`, one per main package the
//! pattern matches), as a tree for placing and as artifacts.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.go";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 1;

/// `(GOOS, GOARCH, GOARM)` for a target triple.
pub fn go_target(triple: &str) -> Result<(String, String, Option<String>), String> {
    let lowered = triple.to_ascii_lowercase();
    let arch = lowered.split('-').next().unwrap_or_default();
    let os = if lowered.contains("-linux") {
        "linux"
    } else if lowered.contains("-windows") {
        "windows"
    } else if lowered.contains("-darwin") || lowered.contains("-apple") {
        "darwin"
    } else if lowered.contains("-freebsd") {
        "freebsd"
    } else {
        return Err(format!(
            "{triple}: Go builds for an OS, and this names none it knows"
        ));
    };
    let (goarch, goarm) = match arch {
        "x86_64" | "amd64" => ("amd64", None),
        "aarch64" | "arm64" => ("arm64", None),
        "i686" | "i586" | "i386" => ("386", None),
        "riscv64" | "riscv64gc" => ("riscv64", None),
        "armv7" | "armv7a" | "thumbv7neon" => ("arm", Some("7")),
        "armv6" | "arm" => ("arm", Some("6")),
        "armv5te" => ("arm", Some("5")),
        "powerpc64le" => ("ppc64le", None),
        "s390x" => ("s390x", None),
        other => return Err(format!("{triple}: no Go architecture for {other}")),
    };
    Ok((os.into(), goarch.into(), goarm.map(str::to_string)))
}

/// Builds the main packages `package` names (`./cmd/server`, `./...`) in
/// `source` for `triple`, with the Go root at `goroot` and the modules in
/// `modules` (empty when the source vendors them).
#[node(
    id = "gaia.build.go",
    inputs(store, goroot, source, modules, package, triple),
    outputs(tree, artifacts)
)]
pub fn build_go(
    store: &str,
    goroot: &str,
    source: Tree,
    modules: Tree,
    package: &str,
    triple: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| StepError::body(NAME, format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (modules_digest, modules_dir) = dir_of(&modules, "module cache tree")?;
    if !source_dir.join("go.mod").is_file() {
        return Err(StepError::body(NAME, "the source has no go.mod"));
    }
    let (goos, goarch, goarm) = go_target(triple).map_err(|e| StepError::body(NAME, e))?;
    let goroot = Path::new(goroot);
    let go = goroot.join("bin/go");
    let version = std::process::Command::new(&go)
        .arg("version")
        .env_clear()
        .env("GOROOT", goroot)
        .env("GOTOOLCHAIN", "local")
        .output()
        .map_err(|e| StepError::body(NAME, format!("{}: {e}", go.display())))?
        .stdout;
    let vendored = source_dir.join("vendor/modules.txt").is_file();
    let key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("go", &version)
        .config("package", package)
        .config(
            "target",
            format!("{goos}/{goarch}/{}", goarm.as_deref().unwrap_or("")),
        )
        .input("source", source_digest)
        .input("modules", modules_digest)
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let dir = work.dir().to_path_buf();
            let fail = |e: std::io::Error| StepError::body(NAME, e);
            let bin = dir.join("out/bin");
            let cache = dir.join("gocache");
            let modcache = dir.join("gomodcache");
            for d in [&bin, &cache, &dir.join("home"), &dir.join("tmp")] {
                std::fs::create_dir_all(d).map_err(fail)?;
            }
            if !vendored {
                copy_dir(&modules_dir, &modcache).map_err(fail)?;
            }
            let mut tool = Tool::new(&go)
                .args([
                    "build",
                    "-trimpath",
                    "-buildvcs=false",
                    "-ldflags=-s -w -buildid=",
                ])
                .arg(if vendored {
                    "-mod=vendor"
                } else {
                    "-mod=readonly"
                })
                .arg("-o")
                .arg(format!("{}/", bin.display()))
                .arg(package)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", goroot.join("bin").display()),
                )
                .env("GOROOT", goroot)
                .env("GOTOOLCHAIN", "local")
                .env("GOOS", &goos)
                .env("GOARCH", &goarch)
                .env("CGO_ENABLED", "0")
                .env("GOPROXY", "off")
                .env("GOFLAGS", "")
                .env("GOCACHE", &cache)
                .env("GOMODCACHE", &modcache)
                .env("GOPATH", dir.join("gopath"))
                .env("HOME", dir.join("home"))
                .env("TMPDIR", dir.join("tmp"))
                .cwd(&source_dir);
            if let Some(arm) = &goarm {
                tool = tool.env("GOARM", arm);
            }
            work.run(&tool)?;
            // Go marks its module cache read-only.
            for d in [&cache, &modcache, &dir.join("gopath")] {
                make_writable(d).map_err(fail)?;
                if d.exists() {
                    std::fs::remove_dir_all(d).map_err(fail)?;
                }
            }
            for d in ["home", "tmp"] {
                std::fs::remove_dir_all(dir.join(d)).map_err(fail)?;
            }
            Ok(())
        })?;
    let out = made.outputs["out"];
    let mut files: Vec<PathBuf> = std::fs::read_dir(store.path(&out).join("bin"))
        .map_err(|e| StepError::body(NAME, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(StepError::body(
            NAME,
            format!("{package} has no main package to build for {goos}/{goarch}"),
        ));
    }
    let artifacts = files
        .iter()
        .map(|f| {
            let object = store.put_file(f).map_err(StepError::store(NAME))?;
            Ok(Artifact {
                kind: ArtifactKind::Binary,
                object: object.into(),
            })
        })
        .collect::<Result<_, StepError>>()?;
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            let mode = std::fs::metadata(&dst)?.permissions().mode();
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode | 0o200))?;
        }
    }
    Ok(())
}

fn make_writable(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return Ok(());
    };
    if meta.is_dir() {
        std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(meta.permissions().mode() | 0o700),
        )?;
        for entry in std::fs::read_dir(dir)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_GO_NODE];
