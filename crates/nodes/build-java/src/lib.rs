//! `gaia.build.java`: Java sources compiled into a jar.
//!
//! - **Pinned JDK:** the JDK at the path given (`bin/javac`, `bin/jar`);
//!   `javac -version` and `java -version` are in the key. The tools run
//!   with a cleared environment.
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
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.java";
const RECIPE_VERSION: u32 = 1;
/// The earliest time a jar entry can carry.
const JAR_DATE: &str = "1980-01-01T00:00:02Z";

/// Builds `<name>.jar` from `source` with the JDK at `jdk` and the jars in
/// `libs`, for Java `release` (0: the JDK's own), with `main_class` as its
/// entry point when not empty.
#[node(
    id = "gaia.build.java",
    inputs(store, jdk, source, libs, name, release, main_class),
    outputs(tree, jar)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_java(
    store: &str,
    jdk: &str,
    source: Tree,
    libs: Tree,
    name: &str,
    release: i64,
    main_class: &str,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let jdk = Path::new(jdk);
    if name.is_empty() || name.contains('/') {
        return Err(StepError::body(NAME, format!("{name:?} is not a jar name")));
    }
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| StepError::body(NAME, format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (libs_digest, libs_dir) = dir_of(&libs, "dependency jars tree")?;
    let version = |program: &str| {
        std::process::Command::new(jdk.join("bin").join(program))
            .arg("-version")
            .env_clear()
            .output()
            .map_err(|e| StepError::body(NAME, format!("{program} -version: {e}")))
            .map(|o| [o.stdout, o.stderr].concat())
    };
    let key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("javac", version("javac")?)
        .config("java", version("java")?)
        .config("name", name)
        .config("release", release.to_string())
        .config("main class", main_class)
        .input("source", source_digest)
        .input("libs", libs_digest)
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let fail = |e: std::io::Error| StepError::body(NAME, e);
            let java_root = if source_dir.join("src/main/java").is_dir() {
                source_dir.join("src/main/java")
            } else {
                source_dir.clone()
            };
            let sources =
                files(&java_root, |p| p.extension().is_some_and(|x| x == "java")).map_err(fail)?;
            if sources.is_empty() {
                return Err(StepError::body(
                    NAME,
                    format!("no .java files under {}", rel(&java_root, &source_dir)),
                ));
            }
            let jars =
                files(&libs_dir, |p| p.extension().is_some_and(|x| x == "jar")).map_err(fail)?;
            let classes = work.dir().join("classes");
            std::fs::create_dir_all(&classes).map_err(fail)?;
            let env = |tool: Tool| {
                tool.env("JAVA_HOME", jdk)
                    .env("LC_ALL", "C")
                    .env("SOURCE_DATE_EPOCH", "0")
                    .env("TZ", "UTC")
            };
            let mut javac = env(Tool::new(jdk.join("bin/javac")))
                .args(["-encoding", "UTF-8", "-nowarn", "-implicit:none"])
                .arg("-d")
                .arg(&classes)
                .cwd(&java_root);
            if release > 0 {
                javac = javac.args(["--release", &release.to_string()]);
            }
            if !jars.is_empty() {
                let path: Vec<String> = jars
                    .iter()
                    .map(|j| libs_dir.join(j).display().to_string())
                    .collect();
                javac = javac.arg("-classpath").arg(path.join(":"));
            }
            work.run(&javac.args(&sources))?;
            let resources = source_dir.join("src/main/resources");
            if resources.is_dir() {
                for file in files(&resources, |_| true).map_err(fail)? {
                    let to = classes.join(&file);
                    std::fs::create_dir_all(to.parent().unwrap_or(&classes)).map_err(fail)?;
                    std::fs::copy(resources.join(&file), to).map_err(fail)?;
                }
            }
            let lib = work.dir().join("out/lib");
            std::fs::create_dir_all(&lib).map_err(fail)?;
            let mut jar = env(Tool::new(jdk.join("bin/jar")))
                .arg("--create")
                .arg(format!("--date={JAR_DATE}"))
                .arg("--file")
                .arg(lib.join(format!("{name}.jar")));
            if !main_class.is_empty() {
                jar = jar.arg("--main-class").arg(main_class);
            }
            // `-C` applies to the one path after it.
            for entry in files(&classes, |_| true).map_err(fail)? {
                jar = jar.arg("-C").arg(&classes).arg(entry);
            }
            work.run(&jar)?;
            for dep in &jars {
                let file = dep.file_name().unwrap_or(dep.as_os_str());
                std::fs::copy(libs_dir.join(dep), lib.join(file)).map_err(fail)?;
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
