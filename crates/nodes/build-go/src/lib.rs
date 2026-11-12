//! `gaia.build.go`: a Go module built into binaries for a target, in a
//! pinned glibc image, through bubblewrap.
//!
//! - **Sandboxed:** `go` runs by [`gaia_sandbox`], with the image's root
//!   filesystem (`root`, a pulled image tree) as the root. Nothing of the
//!   host is there but what is bound: the Go root at `/toolchain` and the
//!   source at `/src` (both read-only), and the step's directory at
//!   `/build`, the only writable path (the build and module caches, and
//!   `out`). The network is off.
//! - **Pinned toolchain:** the Go root given (`GOROOT=/toolchain`, with
//!   `GOTOOLCHAIN=local`, so no other toolchain is fetched); its
//!   `go version`, run in the same sandbox, is in the key.
//! - **Offline, from the lock:** a module with `vendor/modules.txt` builds
//!   with `-mod=vendor`; otherwise modules come from the module cache tree
//!   given (as `go mod download` fills it, copied into the step's
//!   directory, since Go writes beside it), with `GOPROXY=off`, so a
//!   module missing from it is an error rather than a download. `go.sum`
//!   is still checked.
//! - **For a target:** `GOOS`, `GOARCH` and `GOARM` from the target triple
//!   (`aarch64-unknown-linux-musl` is `linux/arm64`, `armv7-…-gnueabihf` is
//!   `linux/arm` with `GOARM=7`). `CGO_ENABLED=0`: static binaries, no C
//!   toolchain.
//! - **Reproducible and lean:** `-trimpath -buildvcs=false` and
//!   `-ldflags=-s -w -buildid=`, with the build and module caches in the
//!   step's own directory at a fixed path, so two stores give the same
//!   bytes.
//!
//! The outputs are the binaries (`bin/<name>`, one per main package the
//! pattern matches), as a tree for placing and as artifacts.

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.go";
/// The probe that reads the toolchain's version inside the sandbox.
const PROBE: &str = "gaia.build.go.version";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 2;

/// Where the sandbox binds each input and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const SOURCE: &str = "/src";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";

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
/// `source` for `triple`, with the Go root at `goroot` (a directory on this
/// host, bound read-only) and the modules in `modules` (empty when the
/// source vendors them), in the image `root` (a stored root filesystem
/// tree). `bwrap` is the bubblewrap program that runs every tool.
#[node(
    id = "gaia.build.go",
    inputs(store, bwrap, root, goroot, source, modules, package, triple),
    outputs(tree, artifacts)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_go(
    store: &str,
    bwrap: &str,
    root: Tree,
    goroot: &str,
    source: Tree,
    modules: Tree,
    package: &str,
    triple: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |message: String| StepError::body(NAME, message);
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| fail(format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (root_digest, root_dir) = dir_of(&root, "image")?;
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (modules_digest, modules_dir) = dir_of(&modules, "module cache tree")?;
    if !source_dir.join("go.mod").is_file() {
        return Err(fail("the source has no go.mod".into()));
    }
    let (goos, goarch, goarm) = go_target(triple).map_err(fail)?;
    // What every tool of this build sees: the image, the Go root and the
    // source. The writable step directory is added for each step.
    let base = Sandbox::new(bwrap, root_dir)
        .input(PathBuf::from(goroot), TOOLCHAIN)
        .input(source_dir.clone(), SOURCE)
        .env("PATH", PATH_IN_SANDBOX)
        .env("GOROOT", TOOLCHAIN)
        .env("GOTOOLCHAIN", "local");
    let version = {
        let key = base.key(KeyBuilder::new(PROBE, 1)).finish();
        let made = Step::new(&store, cx, PROBE, key).look_up_or_make(
            &[("version", Path::new("version"))],
            |work| {
                let tool = base
                    .clone()
                    .output(work.dir(), BUILD)
                    .tool("/bin/sh")
                    .map_err(|e| fail(format!("the sandbox: {e}")))?
                    .args(["-c", "exec \"$0\" version >/build/version 2>&1"])
                    .arg(format!("{TOOLCHAIN}/bin/go"));
                work.run(&tool).map(|_| ())
            },
        )?;
        std::fs::read_to_string(store.path(&made.outputs["version"]))
            .map_err(|e| fail(format!("go version: {e}")))?
    };
    let vendored = source_dir.join("vendor/modules.txt").is_file();
    let mut sandbox = base
        .env("GOOS", &goos)
        .env("GOARCH", &goarch)
        .env("CGO_ENABLED", "0")
        .env("GOPROXY", "off")
        .env("GOFLAGS", "")
        .env("GOCACHE", format!("{BUILD}/gocache"))
        .env("GOMODCACHE", format!("{BUILD}/gomodcache"))
        .env("GOPATH", format!("{BUILD}/gopath"))
        .env("HOME", format!("{BUILD}/home"))
        .cwd(SOURCE);
    if let Some(arm) = &goarm {
        sandbox = sandbox.env("GOARM", arm);
    }
    let key = sandbox
        .key(
            KeyBuilder::new(NAME, RECIPE_VERSION)
                .config("go", &version)
                .config("package", package)
                .config(
                    "target",
                    format!("{goos}/{goarch}/{}", goarm.as_deref().unwrap_or("")),
                )
                .input("image", root_digest)
                .input("source", source_digest)
                .input("modules", modules_digest),
        )
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let dir = work.dir().to_path_buf();
            let io = |e: std::io::Error| fail(e.to_string());
            let (cache, modcache, home) = (
                dir.join("gocache"),
                dir.join("gomodcache"),
                dir.join("home"),
            );
            for d in [&dir.join("out/bin"), &cache, &home] {
                std::fs::create_dir_all(d).map_err(io)?;
            }
            if !vendored {
                copy_dir(&modules_dir, &modcache).map_err(io)?;
            }
            let tool = sandbox
                .clone()
                .output(&dir, BUILD)
                .tool(&format!("{TOOLCHAIN}/bin/go"))
                .map_err(|e| fail(format!("the sandbox: {e}")))?
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
                .arg(format!("{BUILD}/out/bin/"))
                .arg(package);
            work.run(&tool)?;
            // Go marks its module cache read-only.
            for d in [&cache, &modcache, &dir.join("gopath"), &home] {
                make_writable(d).map_err(io)?;
                if d.exists() {
                    std::fs::remove_dir_all(d).map_err(io)?;
                }
            }
            Ok(())
        })?;
    let out = made.outputs["out"];
    let mut files: Vec<PathBuf> = std::fs::read_dir(store.path(&out).join("bin"))
        .map_err(|e| fail(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(fail(format!(
            "{package} has no main package to build for {goos}/{goarch}"
        )));
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
