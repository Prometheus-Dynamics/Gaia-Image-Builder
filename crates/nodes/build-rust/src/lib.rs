//! `gaia.build.rust`: a Rust package built from its source tree.
//!
//! - **Pinned toolchain:** the Rust sysroot given (its `rustc -vV` is in the
//!   key). Nothing from the host's `PATH` or `~/.cargo` is used.
//! - **Offline:** `--locked --offline`, with crates.io dependencies taken
//!   from the vendored tree (what `cargo vendor` writes, checked against
//!   `Cargo.lock`). Nothing is fetched.
//! - **Reproducible:** the source, the vendored crates, cargo's home and
//!   the target directory are remapped to fixed paths (`/src`, `/vendor`,
//!   `/cargo`, `/target`), and `SOURCE_DATE_EPOCH` is 0, so the same inputs
//!   give the same bytes in any store.
//! - **Lean by default:** the release profile, symbols stripped. For
//!   `*-musl` and bare-metal targets the link is the toolchain's own
//!   `rust-lld`, with identical code folded (`--icf=all`); unused sections
//!   are dropped as rustc always asks.
//!
//! The outputs are the package's binaries (`bin/<name>`) and its shared
//! and static libraries (`lib/<name>`), as one tree for placing and as
//! artifacts.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.rust";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 1;

/// Builds `package` from `source` for `triple` (tuned for `cpu`, when not
/// empty), with `features`, using the toolchain at `toolchain` and the
/// crates in `vendor`.
#[node(
    id = "gaia.build.rust",
    inputs(store, toolchain, source, vendor, package, features, triple, cpu),
    outputs(tree, artifacts)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_rust(
    store: &str,
    toolchain: &str,
    source: Tree,
    vendor: Tree,
    package: &str,
    features: Vec<String>,
    triple: &str,
    cpu: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let toolchain = Path::new(toolchain);
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| StepError::body(NAME, format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (vendor_digest, vendor_dir) = dir_of(&vendor, "vendored crates tree")?;
    if !source_dir.join("Cargo.lock").is_file() {
        return Err(StepError::body(
            NAME,
            "the source has no Cargo.lock; the build is locked",
        ));
    }
    let version = |program: &str, arg: &str| {
        std::process::Command::new(toolchain.join("bin").join(program))
            .arg(arg)
            .env_clear()
            .output()
            .map_err(|e| StepError::body(NAME, format!("{program} {arg}: {e}")))
            .map(|o| o.stdout)
    };
    let mut features = features;
    features.sort();
    features.dedup();
    let key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("rustc", version("rustc", "-vV")?)
        .config("cargo", version("cargo", "-V")?)
        .config("package", package)
        .config("features", features.join(","))
        .config("triple", triple)
        .config("cpu", cpu)
        .input("source", source_digest)
        .input("vendor", vendor_digest)
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let home = work.dir().join("cargo-home");
            let target = work.dir().join("target");
            std::fs::create_dir_all(&home).map_err(|e| StepError::body(NAME, e))?;
            std::fs::write(home.join("config.toml"), vendor_config(&vendor_dir))
                .map_err(|e| StepError::body(NAME, e))?;
            let mut flags = vec![
                remap(&source_dir, "/src"),
                remap(&vendor_dir, "/vendor"),
                remap(&home, "/cargo"),
                remap(&target, "/target"),
            ];
            if !cpu.is_empty() {
                flags.push(format!("-Ctarget-cpu={cpu}"));
            }
            let lld = triple.contains("-musl") || triple.contains("-none");
            if lld {
                flags.push("-Clink-arg=--icf=all".into());
            }
            let mut tool = Tool::new(toolchain.join("bin/cargo"))
                .args([
                    "build",
                    "--release",
                    "--locked",
                    "--offline",
                    "--package",
                    package,
                ])
                .args(["--target", triple])
                .arg("--manifest-path")
                .arg(source_dir.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", toolchain.join("bin").display()),
                )
                .env("CARGO_HOME", &home)
                .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\u{1f}"))
                .env("CARGO_PROFILE_RELEASE_STRIP", "symbols")
                .env("CARGO_INCREMENTAL", "0")
                .env("CARGO_TERM_COLOR", "never")
                .env("SOURCE_DATE_EPOCH", "0")
                .cwd(&source_dir);
            if !features.is_empty() {
                tool = tool.args(["--features", &features.join(",")]);
            }
            if lld {
                let var = format!(
                    "CARGO_TARGET_{}_LINKER",
                    triple.to_ascii_uppercase().replace(['-', '.'], "_")
                );
                tool = tool.env(var, "rust-lld");
            }
            work.run(&tool)?;
            collect(
                &target.join(triple).join("release"),
                &work.dir().join("out"),
            )
        })?;
    let out = made.outputs["out"];
    let out_dir = store.path(&out);
    let mut artifacts = Vec::new();
    for (sub, kind) in [
        ("bin", ArtifactKind::Binary),
        ("lib", ArtifactKind::Library),
    ] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(out_dir.join(sub))
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        files.sort();
        for file in files {
            let object = store.put_file(&file).map_err(StepError::store(NAME))?;
            artifacts.push(Artifact {
                kind: kind.clone(),
                object: object.into(),
            });
        }
    }
    if artifacts.is_empty() {
        return Err(StepError::body(
            NAME,
            format!("{package} built no binary or library for {triple}"),
        ));
    }
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

fn remap(from: &Path, to: &str) -> String {
    format!("--remap-path-prefix={}={to}", from.display())
}

/// Cargo's config: crates.io from the vendored tree, when it has crates.
fn vendor_config(vendor: &Path) -> String {
    let has_crates = std::fs::read_dir(vendor).is_ok_and(|mut d| d.next().is_some());
    if !has_crates {
        return String::new();
    }
    format!(
        "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"{}\"\n",
        vendor.display()
    )
}

/// The top-level outputs in cargo's profile directory: executables go to
/// `bin/`, shared and static libraries to `lib/`. Cargo's own files (`.d`,
/// `.rlib`, its directories) are left.
fn collect(profile: &Path, out: &Path) -> Result<(), StepError> {
    let fail = |e: std::io::Error| StepError::body(NAME, format!("{}: {e}", profile.display()));
    let mut entries: Vec<_> = std::fs::read_dir(profile)
        .map_err(fail)?
        .collect::<Result<_, _>>()
        .map_err(fail)?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let meta = entry.metadata().map_err(fail)?;
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let sub = if name.ends_with(".so") || name.ends_with(".a") {
            "lib"
        } else if meta.permissions().mode() & 0o111 != 0 && !name.contains('.') {
            "bin"
        } else {
            continue;
        };
        let dir = out.join(sub);
        std::fs::create_dir_all(&dir).map_err(fail)?;
        std::fs::copy(entry.path(), dir.join(&name)).map_err(fail)?;
    }
    std::fs::create_dir_all(out).map_err(fail)
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_RUST_NODE];
