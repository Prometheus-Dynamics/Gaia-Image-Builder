//! The three proof steps. Minimal versions: groups C and E replace them.

use crate::tar;
use daedalus::prelude::*;
use gaia_step::{Ctx, Digest, KeyBuilder, ObjectRef, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, StoreRef, Tree};
use std::path::{Path, PathBuf};

const GIT: &str = "gaia.proof.git";
const CARGO: &str = "gaia.proof.cargo";
const PACK: &str = "gaia.proof.pack";

fn open(store: &str, step: &'static str) -> Result<Store, StepError> {
    Store::open(store).map_err(StepError::store(step))
}

fn stored(store: &Store, object: &StoreRef, step: &str) -> Result<(ObjectRef, PathBuf), StepError> {
    let object = object
        .object_ref()
        .ok_or_else(|| StepError::body(step, format!("bad object {}", object.digest)))?;
    Ok((object, store.path(&object)))
}

/// Clones `url` at `commit` with the `git` program given, without its
/// `.git` directory. The commit is the source's content digest, so the key
/// needs nothing else.
#[node(
    id = "gaia.proof.git",
    inputs(store, git, url, commit),
    outputs(source)
)]
fn clone(store: &str, git: &str, url: &str, commit: &str, cx: &Ctx) -> Result<Tree, StepError> {
    let store = open(store, GIT)?;
    let key = KeyBuilder::new(GIT, 1)
        .config("url", url)
        .config("commit", commit)
        .finish();
    let made =
        Step::new(&store, cx, GIT, key).look_up_or_make(&[("src", Path::new("src"))], |work| {
            let src = work.dir().join("src");
            std::fs::create_dir(&src).map_err(|e| StepError::body(GIT, e))?;
            let run_git = |work: &mut gaia_step::Work<'_>, args: &[&str]| {
                work.run(
                    &Tool::new(git)
                        .args(args)
                        .cwd(&src)
                        .env("PATH", "/usr/bin:/bin")
                        .env("HOME", work.dir())
                        .env("GIT_CONFIG_NOSYSTEM", "1")
                        .env("GIT_TERMINAL_PROMPT", "0"),
                )
            };
            run_git(work, &["init", "--quiet"])?;
            run_git(work, &["fetch", "--quiet", "--depth", "1", url, commit])?;
            run_git(
                work,
                &[
                    "-c",
                    "advice.detachedHead=false",
                    "checkout",
                    "--quiet",
                    "FETCH_HEAD",
                ],
            )?;
            std::fs::remove_dir_all(src.join(".git")).map_err(|e| StepError::body(GIT, e))
        })?;
    Ok(Tree {
        object: made.outputs["src"].into(),
        entries: Vec::new(),
    })
}

/// Builds `bin` from the source with the toolchain at `toolchain` (a Rust
/// sysroot), offline and locked, with the source's path remapped so the
/// binary does not depend on where the store is.
#[node(
    id = "gaia.proof.cargo",
    inputs(store, toolchain, profile, bin, source),
    outputs(binary)
)]
fn build(
    store: &str,
    toolchain: &str,
    profile: &str,
    bin: &str,
    source: &Tree,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = open(store, CARGO)?;
    let (source_ref, source_dir) = stored(&store, &source.object, CARGO)?;
    let toolchain = Path::new(toolchain);
    let version = std::process::Command::new(toolchain.join("bin/rustc"))
        .arg("-vV")
        .env_clear()
        .output()
        .map_err(|e| StepError::body(CARGO, format!("rustc -vV: {e}")))?
        .stdout;
    let key = KeyBuilder::new(CARGO, 1)
        .config("toolchain", &version)
        .config("profile", profile)
        .config("bin", bin)
        .input("source", source_ref.digest)
        .finish();
    let made = Step::new(&store, cx, CARGO, key).look_up_or_make(
        &[("bin", Path::new("bin"))],
        |work| {
            let target = work.dir().join("target");
            let remap = format!("--remap-path-prefix={}=/src", source_dir.display());
            work.run(
                &Tool::new(toolchain.join("bin/cargo"))
                    .args([
                        "build",
                        "--locked",
                        "--offline",
                        "--profile",
                        profile,
                        "--bin",
                        bin,
                    ])
                    .arg("--manifest-path")
                    .arg(source_dir.join("Cargo.toml"))
                    .arg("--target-dir")
                    .arg(&target)
                    .env(
                        "PATH",
                        format!("{}:/usr/bin:/bin", toolchain.join("bin").display()),
                    )
                    .env("CARGO_HOME", work.dir().join("cargo-home"))
                    .env("CARGO_ENCODED_RUSTFLAGS", remap)
                    .env("CARGO_TERM_COLOR", "never"),
            )?;
            let built = target
                .join(if profile == "dev" { "debug" } else { profile })
                .join(bin);
            std::fs::rename(built, work.dir().join("bin")).map_err(|e| StepError::body(CARGO, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::Binary,
        object: made.outputs["bin"].into(),
    })
}

/// Packs the binary at `path` into a tar with its parent directories,
/// sorted, mtime 0, owned by 0:0.
#[node(id = "gaia.proof.pack", inputs(store, path, binary), outputs(archive))]
fn pack(store: &str, path: &str, binary: &Artifact, cx: &Ctx) -> Result<Artifact, StepError> {
    let store = open(store, PACK)?;
    let (binary_ref, binary_path) = stored(&store, &binary.object, PACK)?;
    let key = KeyBuilder::new(PACK, 1)
        .config("path", path)
        .input("binary", binary_ref.digest)
        .finish();
    let made = Step::new(&store, cx, PACK, key).look_up_or_make(
        &[("tar", Path::new("out.tar"))],
        |work| {
            let data = std::fs::read(&binary_path).map_err(|e| StepError::body(PACK, e))?;
            let path = path.trim_matches('/');
            let mut entries: Vec<tar::Entry<'_>> = path
                .match_indices('/')
                .map(|(at, _)| tar::Entry {
                    path: &path[..at],
                    mode: 0o755,
                    data: None,
                })
                .collect();
            entries.push(tar::Entry {
                path,
                mode: 0o755,
                data: Some(&data),
            });
            let mut out = Vec::new();
            tar::write(&mut out, &mut entries).map_err(|e| StepError::body(PACK, e))?;
            std::fs::write(work.dir().join("out.tar"), out).map_err(|e| StepError::body(PACK, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::Archive,
        object: made.outputs["tar"].into(),
    })
}

/// The digest an artifact's object has, for printing.
pub fn digest_of(artifact: &Artifact) -> Option<Digest> {
    artifact.object.object_ref().map(|object| object.digest)
}

pub(crate) const NODES: [&NodeRecord; 3] = [&CLONE_NODE, &BUILD_NODE, &PACK_NODE];
