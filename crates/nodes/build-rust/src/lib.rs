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

mod group;
mod split;
mod vendor;

pub use vendor::vendor;

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Stream, Work};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const NAME: &str = "gaia.build.rust";
/// The probe that reads a tool's version inside the sandbox.
const PROBE: &str = "gaia.build.rust.version";
/// Changes when how the build is run changes.
/// 3: a profile, and the target's own flags added to the source's.
const RECIPE_VERSION: u32 = 3;

/// Where the sandbox binds each input and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const SOURCE: &str = "/src";
const VENDOR: &str = "/vendor";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";
/// Where a build's own tools are bound, and `PATH` with them.
const TOOLS: &str = "/tools";
const PATH_WITH_TOOLS: &str = "/toolchain/bin:/tools/bin:/tools:/usr/bin:/bin";

/// Builds `package` from `source` for `triple` (tuned for `cpu`, when not
/// empty), with `features` (names separated by spaces or commas, as cargo
/// takes them; text, since a graph sets it as a constant), in the image
/// `root` (a stored root filesystem tree), with the toolchain at `toolchain`
/// (a sysroot directory on this host, bound read-only) and the crates in
/// `vendor`. `bwrap` is the bubblewrap program that runs every tool.
///
/// `cc` and `env` are for a crate that compiles C: the image `root` carries
/// the C compiler, `cc` names a directory of per-target compiler programs
/// (in the source, at `/src/...`, or in the image), and `env` sets what that
/// compiler needs (a cache directory under `/build`, say). Both are in the
/// key through the sandbox's environment.
///
/// `tools` is a tree of programs the build needs and the image lacks (a C
/// compiler for the target, say), bound read-only at `/tools`, with
/// `/tools/bin` and `/tools` on `PATH` after the toolchain. It is in the key.
///
/// `features` goes back to a list (`Vec<String>`) when Daedalus decodes list
/// constants (outside block X7, `docs/design.md`).
#[node(
    id = "gaia.build.rust",
    inputs(
        store, bwrap, root, toolchain, source, vendor, package, features, profile, triple, cpu, cc,
        env, tools
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
    features: Option<&String>,
    profile: Option<&String>,
    triple: &str,
    cpu: Option<&String>,
    cc: Option<&String>,
    env: Option<&String>,
    tools: Option<&Tree>,
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
    // One package, or a sorted list of them (a group): checked first.
    let names = group::names(package).map_err(fail)?;
    // The component's own variables, one `NAME=value` a line. Checked first:
    // a bad line is the description's error, not a build's.
    let mut component_env: Vec<(String, String)> = Vec::new();
    for line in env.map(String::as_str).unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(fail(format!("env line `{line}` is not NAME=value")));
        };
        let reserved = ["PATH", "CARGO_HOME", "SOURCE_DATE_EPOCH", "RUSTFLAGS"];
        let valid = !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !reserved.contains(&name)
            && !name.starts_with("CARGO_ENCODED_");
        if !valid {
            return Err(fail(format!(
                "env name `{name}` is not one a component sets: letters, digits and _, and not {}",
                reserved.join(", ")
            )));
        }
        component_env.push((name.to_string(), value.to_string()));
    }
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
    let cpu = cpu.map(String::as_str).unwrap_or_default();
    // Cargo's name for the profile, and its directory under `target/`.
    let profile = match profile.map(String::as_str).unwrap_or_default() {
        "" | "release" => "release",
        other => other,
    };
    if !profile
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(fail(format!("`{profile}` is not a cargo profile name")));
    }
    let mut features: Vec<&str> = features
        .map(String::as_str)
        .unwrap_or_default()
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
    // The target's own variables: cargo joins a list from the environment
    // to the one in the source's `.cargo/config.toml`, where `RUSTFLAGS`
    // would replace it. So the source's flags for its target stay.
    let target_var = |what: &str| {
        format!(
            "CARGO_TARGET_{}_{what}",
            triple.to_ascii_uppercase().replace(['-', '.'], "_")
        )
    };
    let tools = match tools {
        Some(tree) => Some(dir_of(tree, "tools tree")?),
        None => None,
    };
    let mut sandbox = match &tools {
        Some((_, dir)) => base.input(dir.clone(), TOOLS).env("PATH", PATH_WITH_TOOLS),
        None => base.env("PATH", PATH_IN_SANDBOX),
    };
    sandbox = sandbox
        .env("CARGO_HOME", format!("{BUILD}/cargo-home"))
        .env(target_var("RUSTFLAGS"), flags.join(" "))
        .env(
            format!(
                "CARGO_PROFILE_{}_STRIP",
                profile.to_ascii_uppercase().replace('-', "_")
            ),
            "symbols",
        )
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TERM_COLOR", "never")
        .env("SOURCE_DATE_EPOCH", "0")
        .cwd(SOURCE);
    if lld {
        sandbox = sandbox.env(target_var("LINKER"), "rust-lld");
    }
    // A crate that compiles C (through the `cc` crate) finds its compiler by
    // target: `cc` is a directory in the sandbox holding one program per
    // target, named for the triple (and `<triple>-ar`, when there is one).
    if let Some(dir) = cc.map(String::as_str).filter(|dir| !dir.is_empty()) {
        if !dir.starts_with('/') {
            return Err(fail(format!(
                "cc `{dir}` is not a path inside the sandbox: the source is at {SOURCE}"
            )));
        }
        let name = triple.replace(['-', '.'], "_");
        let dir = dir.trim_end_matches('/');
        sandbox = sandbox.env(format!("CC_{name}"), format!("{dir}/{triple}"));
        let ar = Path::new(dir)
            .strip_prefix(SOURCE)
            .ok()
            .map(|inside| source_dir.join(inside).join(format!("{triple}-ar")));
        if ar.is_some_and(|ar| ar.is_file()) {
            sandbox = sandbox.env(format!("AR_{name}"), format!("{dir}/{triple}-ar"));
        }
    }
    for (name, value) in &component_env {
        sandbox = sandbox.env(name, value);
    }
    // A build with no tools has the key it always had.
    let mut key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("rustc", rustc)
        .config("cargo", cargo)
        .config("package", package)
        .config("profile", profile)
        .config("features", features.join(","))
        .config("triple", triple)
        .config("cpu", cpu)
        .input("image", root_digest)
        .input("source", source_digest)
        .input("vendor", vendor_digest);
    if let Some((digest, _)) = &tools {
        key = key.input("tools", *digest);
    }
    let key = sandbox.key(key).finish();
    let manifest = format!("{SOURCE}/Cargo.toml");
    let target_dir = format!("{BUILD}/target");
    let cargo = format!("{TOOLCHAIN}/bin/cargo");
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
            let profile_dir = target.join(triple).join(profile);
            let build_with = |packages: &[&str]| {
                group::build_args(profile, triple, packages, &features, &manifest, &target_dir)
            };
            if let [only] = names[..] {
                sandboxed(work, &sandbox, &build, &cargo, &build_with(&[only]))?;
                collect(&profile_dir, &out)?;
                return Ok(());
            }
            // Several packages: one cargo run when no crate changes its
            // features, else one run per package in the same target directory.
            let metadata = sandboxed(
                work,
                &sandbox,
                &build,
                &cargo,
                &cargo_metadata_args(&manifest),
            )?;
            match merge_refusal(work, &sandbox, &build, &cargo, &names, triple, &manifest, &features)? {
                None => {
                    sandboxed(work, &sandbox, &build, &cargo, &build_with(&names))?;
                }
                Some(why) => {
                    eprintln!(
                        "gaia.build.rust: {} built one package at a time, not in one cargo run: {why}",
                        names.join(", ")
                    );
                    for package in &names {
                        sandboxed(work, &sandbox, &build, &cargo, &build_with(&[*package]))?;
                    }
                }
            }
            for package in &names {
                let files = group::built_files(&metadata, package).map_err(fail)?;
                collect_package(&profile_dir, &out.join(package), &files)?;
            }
            std::fs::create_dir_all(&out).map_err(|e| fail(format!("{}: {e}", out.display())))
        })?;
    let out = made.outputs["out"];
    let out_dir = store.path(&out);
    let mut artifacts = Vec::new();
    if let [only] = names[..] {
        artifacts = artifacts_in(&store, &out_dir)?;
        if artifacts.is_empty() {
            return Err(fail(format!(
                "{only} built no binary or library for {triple}"
            )));
        }
    } else {
        for package in &names {
            let found = artifacts_in(&store, &out_dir.join(package))?;
            if found.is_empty() {
                return Err(fail(format!(
                    "{package} built no binary or library for {triple}"
                )));
            }
            artifacts.extend(found);
        }
    }
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

/// Runs `program` (cargo) in the sandbox with `args`, the build directory
/// writable, and returns what it printed on its standard output, a line at
/// a time. Its standard error is the run log's.
fn sandboxed(
    work: &mut Work<'_>,
    sandbox: &Sandbox,
    build: &Path,
    program: &str,
    args: &[String],
) -> Result<String, StepError> {
    let captured = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&captured);
    let tool = sandbox
        .clone()
        .output(build, BUILD)
        .tool(program)
        .map_err(|e| StepError::body(NAME, format!("the sandbox: {e}")))?
        .args(args)
        .on_line(move |stream, line| {
            if let Stream::Stdout = stream {
                let mut text = sink.lock().unwrap_or_else(|e| e.into_inner());
                text.push_str(line);
                text.push('\n');
            }
        });
    work.run(&tool)?;
    let text = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(text)
}

/// `cargo metadata` for the packages' own targets: no dependencies, so it
/// needs no crate and gives the same answer in any build.
fn cargo_metadata_args(manifest: &str) -> Vec<String> {
    [
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--locked",
        "--offline",
        "--manifest-path",
        manifest,
    ]
    .map(String::from)
    .to_vec()
}

/// Why `packages` cannot share one cargo run (see [`group::refusal`]), or
/// `None` when they can: each package's own build is listed, and the build of
/// them together is listed, in the sandbox.
#[allow(clippy::too_many_arguments)]
fn merge_refusal(
    work: &mut Work<'_>,
    sandbox: &Sandbox,
    build: &Path,
    cargo: &str,
    packages: &[&str],
    triple: &str,
    manifest: &str,
    features: &[&str],
) -> Result<Option<String>, StepError> {
    let tree = |work: &mut Work<'_>, of: &[&str]| -> Result<group::Features, StepError> {
        let args = group::tree_args(triple, of, manifest);
        Ok(group::features_of(&sandboxed(
            work, sandbox, build, cargo, &args,
        )?))
    };
    // A set feature is refused before any listing is made.
    if !features.is_empty() {
        return Ok(group::refusal(features, &[], &group::Features::new()));
    }
    let merged = tree(work, packages)?;
    let mut solo = Vec::new();
    for package in packages {
        solo.push((*package, tree(work, &[*package])?));
    }
    Ok(group::refusal(features, &solo, &merged))
}

/// One package's files out of a profile directory that holds several
/// packages' builds: the ones `files` names, under `out/bin` or `out/lib`.
fn collect_package(profile: &Path, out: &Path, files: &[group::Built]) -> Result<(), StepError> {
    let fail = |e: std::io::Error| StepError::body(NAME, format!("{}: {e}", out.display()));
    for file in files {
        let from = profile.join(&file.file);
        if !from.is_file() {
            return Err(StepError::body(
                NAME,
                format!("the build left no {} in {}", file.file, profile.display()),
            ));
        }
        let dir = out.join(file.sub);
        std::fs::create_dir_all(&dir).map_err(fail)?;
        gaia_store::copy_file(&from, dir.join(&file.file)).map_err(fail)?;
    }
    std::fs::create_dir_all(out).map_err(fail)
}

/// The artifacts of an output tree: each file of `bin/` and `lib/`, in the
/// store, in name order (binaries first).
fn artifacts_in(store: &Store, out_dir: &Path) -> Result<Vec<Artifact>, StepError> {
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
    Ok(artifacts)
}

/// Cargo's config: the vendored tree's own source replacement (crates.io
/// and git dependencies) when `gaia.build.rust.vendor` made it; for a tree
/// made by hand, crates.io from it when it has crates. The tree is read
/// here and named by its path inside the sandbox.
fn vendor_config(vendor: &Path) -> String {
    if let Some(config) = vendor::config_in(vendor, VENDOR) {
        return config;
    }
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
        gaia_store::copy_file(entry.path(), dir.join(&name)).map_err(fail)?;
    }
    std::fs::create_dir_all(out).map_err(fail)
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 3] = [
    &BUILD_RUST_NODE,
    &vendor::VENDOR_NODE,
    &split::BUILD_RUST_PART_NODE,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo metadata --no-deps` for two packages: a binary each, and a
    /// shared and a static library in the second.
    const METADATA: &str = r#"{"packages":[
      {"name":"lemnosd","targets":[{"name":"lemnosd","kind":["bin"]}]},
      {"name":"lemnos-orion","targets":[{"name":"lemnos-orion","kind":["bin"]},{"name":"orion-ffi","kind":["cdylib"]},{"name":"orion-static","kind":["staticlib"]}]}
    ],"workspace_members":[]}"#;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        names
    }

    /// A profile directory as one cargo run of both packages leaves it: the
    /// files of each, and cargo's own files beside them.
    fn profile(dir: &Path) -> PathBuf {
        let profile = dir.join("target/aarch64-unknown-linux-musl/service");
        std::fs::create_dir_all(profile.join("deps")).expect("mkdir");
        for file in [
            "lemnosd",
            "lemnos-orion",
            "liborion_ffi.so",
            "liborion_static.a",
        ] {
            std::fs::write(profile.join(file), file).expect("write");
        }
        std::fs::write(profile.join("deps/lemnosd-0a1b2c.d"), "x").expect("write");
        std::fs::write(profile.join("lemnosd.d"), "x").expect("write");
        profile
    }

    #[test]
    fn a_package_takes_its_own_binaries_and_libraries_out_of_a_shared_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = profile(dir.path());
        let out = dir.path().join("out");
        let orion = group::built_files(METADATA, "lemnos-orion").expect("files");
        collect_package(&profile, &out.join("lemnos-orion"), &orion).expect("collected");
        assert_eq!(names(&out.join("lemnos-orion/bin")), ["lemnos-orion"]);
        assert_eq!(
            names(&out.join("lemnos-orion/lib")),
            ["liborion_ffi.so", "liborion_static.a"]
        );
        let lemnosd = group::built_files(METADATA, "lemnosd").expect("files");
        collect_package(&profile, &out.join("lemnosd"), &lemnosd).expect("collected");
        assert_eq!(names(&out.join("lemnosd/bin")), ["lemnosd"]);
        assert!(!out.join("lemnosd/lib").exists(), "no library of its own");
    }

    #[test]
    fn a_missing_file_of_a_package_is_an_error_naming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = profile(dir.path());
        std::fs::remove_file(profile.join("lemnosd")).expect("remove");
        let out = dir.path().join("out");
        let files = group::built_files(METADATA, "lemnosd").expect("files");
        let error =
            collect_package(&profile, &out.join("lemnosd"), &files).expect_err("a missing binary");
        assert!(error.to_string().contains("no lemnosd"), "{error}");
    }

    #[test]
    fn a_package_the_metadata_lacks_is_an_error() {
        assert!(group::built_files(METADATA, "lemnos-ctl").is_err());
    }
}
