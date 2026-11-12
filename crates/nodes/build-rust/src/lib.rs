//! `gaia.build.rust`: a Rust package built from its source tree, in a
//! pinned glibc image, through bubblewrap.
//!
//! - **Sandboxed:** cargo and rustc run by [`gaia_sandbox`], with the image's
//!   root filesystem (`root`, a pulled image tree) as the root. Nothing of
//!   the host is there but what is bound: the toolchain at `/toolchain`,
//!   the source at `/src` and the vendored crates at `/vendor` (all
//!   read-only), and the step's directory at `/build`, the only writable
//!   path (cargo's home, its target directory and `out`). The network is off.
//! - **Pinned toolchain:** the sysroot given, bound read-only. `rustc -vV`
//!   and `cargo -V`, run in the same sandbox, are in the key. Nothing from
//!   the host's `PATH` or `~/.cargo` is used.
//! - **Offline:** `--locked --offline`, with crates.io dependencies taken
//!   from the vendored tree (what `cargo vendor` writes, checked against
//!   `Cargo.lock`). Nothing is fetched.
//! - **Reproducible:** the paths the build sees are fixed inside the sandbox
//!   (`/src`, `/vendor`, `/cargo`), and `SOURCE_DATE_EPOCH` is 0, so the same
//!   inputs give the same bytes in any store.
//! - **Lean by default:** the release profile, symbols stripped. For
//!   `*-musl` and bare-metal targets the link is the toolchain's own
//!   `rust-lld`, with identical code folded (`--icf=all`); unused sections
//!   are dropped as rustc always asks.
//!
//! The outputs are the package's binaries (`bin/<name>`) and its shared
//! and static libraries (`lib/<name>`), as one tree for placing and as
//! artifacts.

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.rust";
/// The probe that reads a tool's version inside the sandbox.
const PROBE: &str = "gaia.build.rust.version";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 2;

/// Where the sandbox binds each input and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const SOURCE: &str = "/src";
const VENDOR: &str = "/vendor";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";

/// Builds `package` from `source` for `triple` (tuned for `cpu`, when not
/// empty), with `features` (names separated by spaces or commas, as cargo
/// takes them; text, since a graph sets it as a constant), in the image
/// `root` (a stored root filesystem tree), with the toolchain at `toolchain`
/// (a sysroot directory on this host, bound read-only) and the crates in
/// `vendor`. `bwrap` is the bubblewrap program that runs every tool.
///
/// `features` goes back to a list (`Vec<String>`) when Daedalus decodes list
/// constants (outside block X7, `docs/design.md`).
#[node(
    id = "gaia.build.rust",
    inputs(
        store, bwrap, root, toolchain, source, vendor, package, features, triple, cpu
    ),
    outputs(tree, artifacts)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_rust(
    store: &str,
    bwrap: &str,
    root: Tree,
    toolchain: &str,
    source: Tree,
    vendor: Tree,
    package: &str,
    features: &str,
    triple: &str,
    cpu: &str,
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
    let (vendor_digest, vendor_dir) = dir_of(&vendor, "vendored crates tree")?;
    if !source_dir.join("Cargo.lock").is_file() {
        return Err(fail(
            "the source has no Cargo.lock; the build is locked".into(),
        ));
    }
    // What every tool of this build sees: the image, the toolchain and the
    // inputs. The writable step directory is added for the build itself.
    let base = Sandbox::new(bwrap, root_dir.clone())
        .input(PathBuf::from(toolchain), TOOLCHAIN)
        .input(source_dir.clone(), SOURCE)
        .input(vendor_dir.clone(), VENDOR);
    // A tool's version, read by a step of its own in the sandbox.
    let probe = |program: &str, arg: &str| -> Result<String, StepError> {
        let key = base
            .key(
                KeyBuilder::new(PROBE, 1)
                    .config("program", program)
                    .config("arg", arg),
            )
            .finish();
        let made = Step::new(&store, cx, PROBE, key).look_up_or_make(
            &[("version", Path::new("version"))],
            |work| {
                let tool = base
                    .tool(&format!("{TOOLCHAIN}/bin/{program}"))
                    .map_err(|e| fail(format!("{program}: {e}")))?
                    .arg(arg);
                let output = work.run(&tool)?;
                std::fs::write(work.dir().join("version"), output.stdout_tail.join("\n"))
                    .map_err(|e| fail(format!("{program} version: {e}")))
            },
        )?;
        std::fs::read_to_string(store.path(&made.outputs["version"]))
            .map_err(|e| fail(format!("{program} version: {e}")))
    };
    let rustc = probe("rustc", "-vV")?;
    let cargo = probe("cargo", "-V")?;
    let mut features: Vec<&str> = features
        .split([' ', ','])
        .filter(|f| !f.is_empty())
        .collect();
    features.sort();
    features.dedup();
    let mut flags = vec![format!("--remap-path-prefix={BUILD}/cargo-home=/cargo")];
    if !cpu.is_empty() {
        flags.push(format!("-Ctarget-cpu={cpu}"));
    }
    let lld = triple.contains("-musl") || triple.contains("-none");
    if lld {
        flags.push("-Clink-arg=--icf=all".into());
    }
    let mut sandbox = base
        .env("PATH", PATH_IN_SANDBOX)
        .env("CARGO_HOME", format!("{BUILD}/cargo-home"))
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\u{1f}"))
        .env("CARGO_PROFILE_RELEASE_STRIP", "symbols")
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TERM_COLOR", "never")
        .env("SOURCE_DATE_EPOCH", "0")
        .cwd(SOURCE);
    if lld {
        let var = format!(
            "CARGO_TARGET_{}_LINKER",
            triple.to_ascii_uppercase().replace(['-', '.'], "_")
        );
        sandbox = sandbox.env(var, "rust-lld");
    }
    let key = sandbox
        .key(
            KeyBuilder::new(NAME, RECIPE_VERSION)
                .config("rustc", rustc)
                .config("cargo", cargo)
                .config("package", package)
                .config("features", features.join(","))
                .config("triple", triple)
                .config("cpu", cpu)
                .input("image", root_digest)
                .input("source", source_digest)
                .input("vendor", vendor_digest),
        )
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let build = work.dir().to_path_buf();
            let home = build.join("cargo-home");
            let target = build.join("target");
            let out = build.join("out");
            for dir in [&home, &out] {
                std::fs::create_dir_all(dir)
                    .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            }
            std::fs::write(home.join("config.toml"), vendor_config(&vendor_dir))
                .map_err(|e| fail(format!("cargo config: {e}")))?;
            let mut tool = sandbox
                .clone()
                .output(&build, BUILD)
                .tool(&format!("{TOOLCHAIN}/bin/cargo"))
                .map_err(|e| fail(format!("the sandbox: {e}")))?
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
                .arg(format!("{SOURCE}/Cargo.toml"))
                .arg("--target-dir")
                .arg(format!("{BUILD}/target"));
            if !features.is_empty() {
                tool = tool.args(["--features", &features.join(",")]);
            }
            work.run(&tool)?;
            collect(&target.join(triple).join("release"), &out)
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
        return Err(fail(format!(
            "{package} built no binary or library for {triple}"
        )));
    }
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

/// Cargo's config: crates.io from the vendored tree, when it has crates.
/// The tree is read here and named by its path inside the sandbox.
fn vendor_config(vendor: &Path) -> String {
    let has_crates = std::fs::read_dir(vendor).is_ok_and(|mut d| d.next().is_some());
    if !has_crates {
        return String::new();
    }
    format!(
        "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"{VENDOR}\"\n"
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
