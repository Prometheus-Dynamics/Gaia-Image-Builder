//! `gaia.build.java`: Java sources compiled into a jar, in a pinned glibc
//! image, through bubblewrap.
//!
//! - **Sandboxed:** `javac` and `jar` run by [`gaia_sandbox`], with the
//!   image's root filesystem (`root`, a pulled image tree) as the root.
//!   Nothing of the host is there but what is bound: the JDK at
//!   `/toolchain`, the source at `/src` and the dependency jars at `/libs`
//!   (all read-only), and the step's directory at `/build`, the only
//!   writable path. The network is off.
//! - **Pinned JDK:** the JDK at the path given (`bin/javac`, `bin/jar`), a
//!   self-contained one such as a Temurin or upstream OpenJDK build;
//!   `javac -version` and `java -version`, run in the same sandbox, are in
//!   the key.
//! - **Offline, from the lock:** dependencies are the jars in the `libs`
//!   tree (what the lock pins, fetched by a source node). They are the
//!   whole class path; nothing is resolved or downloaded.
//! - **Reproducible:** sources are compiled in path order, and the jar's
//!   entries are added in path order with one fixed timestamp
//!   (`jar --date`), so the same inputs give the same bytes in any store.
//!
//! The output tree is `lib/<name>.jar` with the dependency jars beside it,
//! the set a `java -cp 'lib/*'` runs; the built jar is also an artifact.
//! Sources are every `*.java` under `src/main/java` when it exists (the
//! Maven and Gradle layout), else under the tree's root; resources are
//! the files under `src/main/resources`.

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.java";
/// The probe that reads a tool's version inside the sandbox.
const PROBE: &str = "gaia.build.java.version";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 2;
/// The earliest time a jar entry can carry.
const JAR_DATE: &str = "1980-01-01T00:00:02Z";

/// Where the sandbox binds each input and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const SOURCE: &str = "/src";
const LIBS: &str = "/libs";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";

/// Builds `<name>.jar` from `source` with the JDK at `jdk` (a directory on
/// this host, bound read-only) and the jars in `libs`, for Java `release`
/// (0: the JDK's own), with `main_class` as its entry point when not
/// empty, in the image `root` (a stored root filesystem tree). `bwrap` is
/// the bubblewrap program that runs every tool.
#[node(
    id = "gaia.build.java",
    inputs(store, bwrap, root, jdk, source, libs, name, release, main_class),
    outputs(tree, jar)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_java(
    store: &str,
    bwrap: &str,
    root: Tree,
    jdk: &str,
    source: Tree,
    libs: Tree,
    name: &str,
    release: i64,
    main_class: &str,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |message: String| StepError::body(NAME, message);
    if name.is_empty() || name.contains('/') {
        return Err(fail(format!("{name:?} is not a jar name")));
    }
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| fail(format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (root_digest, root_dir) = dir_of(&root, "image")?;
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (libs_digest, libs_dir) = dir_of(&libs, "dependency jars tree")?;
    // What every tool of this build sees: the image, the JDK and the inputs.
    // The writable step directory is added for the build itself.
    let base = Sandbox::new(bwrap, root_dir)
        .input(PathBuf::from(jdk), TOOLCHAIN)
        .input(source_dir.clone(), SOURCE)
        .input(libs_dir.clone(), LIBS)
        .env("PATH", PATH_IN_SANDBOX)
        .env("JAVA_HOME", TOOLCHAIN)
        .env("LC_ALL", "C")
        .env("SOURCE_DATE_EPOCH", "0")
        .env("TZ", "UTC");
    let javac = probe(&store, cx, &base, "javac", &["-version"])?;
    let java = probe(&store, cx, &base, "java", &["-version"])?;
    let key = base
        .key(
            KeyBuilder::new(NAME, RECIPE_VERSION)
                .config("javac", javac)
                .config("java", java)
                .config("name", name)
                .config("release", release.to_string())
                .config("main class", main_class)
                .input("image", root_digest)
                .input("source", source_digest)
                .input("libs", libs_digest),
        )
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let io = |e: std::io::Error| fail(e.to_string());
            let java_rel = if source_dir.join("src/main/java").is_dir() {
                Path::new("src/main/java")
            } else {
                Path::new("")
            };
            let java_root = source_dir.join(java_rel);
            let sources =
                files(&java_root, |p| p.extension().is_some_and(|x| x == "java")).map_err(io)?;
            if sources.is_empty() {
                return Err(fail(format!(
                    "no .java files under {}",
                    rel(&java_root, &source_dir)
                )));
            }
            let jars =
                files(&libs_dir, |p| p.extension().is_some_and(|x| x == "jar")).map_err(io)?;
            let build = work.dir().to_path_buf();
            let classes = build.join("classes");
            let lib = build.join("out/lib");
            for dir in [&classes, &lib] {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            let sandbox = base.clone().output(&build, BUILD);
            let tool = |program: &str, cwd: &Path| {
                sandbox
                    .clone()
                    .cwd(Path::new(SOURCE).join(cwd).display().to_string())
                    .tool(&format!("{TOOLCHAIN}/bin/{program}"))
                    .map_err(|e| fail(format!("the sandbox: {e}")))
            };
            let mut javac = tool("javac", java_rel)?
                .args(["-encoding", "UTF-8", "-nowarn", "-implicit:none"])
                .arg("-d")
                .arg(format!("{BUILD}/classes"));
            if release > 0 {
                javac = javac.args(["--release", &release.to_string()]);
            }
            if !jars.is_empty() {
                let path: Vec<String> = jars
                    .iter()
                    .map(|j| Path::new(LIBS).join(j).display().to_string())
                    .collect();
                javac = javac.arg("-classpath").arg(path.join(":"));
            }
            work.run(&javac.args(&sources))?;
            let resources = source_dir.join("src/main/resources");
            if resources.is_dir() {
                for file in files(&resources, |_| true).map_err(io)? {
                    let to = classes.join(&file);
                    std::fs::create_dir_all(to.parent().unwrap_or(&classes)).map_err(io)?;
                    std::fs::copy(resources.join(&file), to).map_err(io)?;
                }
            }
            let mut jar = tool("jar", Path::new(""))?
                .arg("--create")
                .arg(format!("--date={JAR_DATE}"))
                .arg("--file")
                .arg(format!("{BUILD}/out/lib/{name}.jar"));
            if !main_class.is_empty() {
                jar = jar.arg("--main-class").arg(main_class);
            }
            // `-C` applies to the one path after it.
            for entry in files(&classes, |_| true).map_err(io)? {
                jar = jar.arg("-C").arg(format!("{BUILD}/classes")).arg(entry);
            }
            work.run(&jar)?;
            for dep in &jars {
                let file = dep.file_name().unwrap_or(dep.as_os_str());
                std::fs::copy(libs_dir.join(dep), lib.join(file)).map_err(io)?;
            }
            Ok(())
        })?;
    let out = made.outputs["out"];
    let built = store.path(&out).join(format!("lib/{name}.jar"));
    let object = store.put_file(&built).map_err(StepError::store(NAME))?;
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        Artifact {
            kind: ArtifactKind::parse("jar"),
            object: object.into(),
        },
    ))
}

/// A JDK tool's version (what it prints on either stream), read by a step
/// of its own in the sandbox.
fn probe(
    store: &Store,
    cx: &Ctx,
    base: &Sandbox,
    program: &str,
    args: &[&str],
) -> Result<String, StepError> {
    let fail = |e: String| StepError::body(NAME, format!("{program} {}: {e}", args.join(" ")));
    let key = base
        .key(
            KeyBuilder::new(PROBE, 1)
                .config("program", program)
                .config("args", args.join(" ")),
        )
        .finish();
    let made = Step::new(store, cx, PROBE, key).look_up_or_make(
        &[("version", Path::new("version"))],
        |work| {
            // The image's shell sends both streams to the file: the JDK's
            // tools print their version on stderr.
            let tool = base
                .clone()
                .output(work.dir(), BUILD)
                .tool("/bin/sh")
                .map_err(|e| fail(e.to_string()))?
                .args(["-c", "exec \"$0\" \"$@\" >/build/version 2>&1"])
                .arg(format!("{TOOLCHAIN}/bin/{program}"))
                .args(args);
            work.run(&tool).map(|_| ())
        },
    )?;
    std::fs::read_to_string(store.path(&made.outputs["version"])).map_err(|e| fail(e.to_string()))
}

fn rel(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The files under `root` that `keep` takes, relative and sorted.
fn files(root: &Path, keep: impl Fn(&Path) -> bool + Copy) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, Path::new(""), keep, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(
    root: &Path,
    prefix: &Path,
    keep: impl Fn(&Path) -> bool + Copy,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root.join(prefix))? {
        let entry = entry?;
        let path = prefix.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &path, keep, out)?;
        } else if kind.is_file() && keep(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_JAVA_NODE];
