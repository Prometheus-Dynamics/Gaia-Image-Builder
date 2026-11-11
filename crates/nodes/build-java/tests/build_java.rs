//! A library jar and an app using it, each built twice in two stores: the
//! same bytes, and the app runs from the output tree.

use gaia_build_java::build_java;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// The JDK whose `javac` is on `PATH`, or `JAVA_HOME`'s.
fn jdk() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("JAVA_HOME").map(PathBuf::from)
        && home.join("bin/javac").is_file()
        && home.join("bin/jar").is_file()
    {
        return Some(home);
    }
    let path = std::env::var_os("PATH")?;
    let javac = std::env::split_paths(&path)
        .map(|d| d.join("javac"))
        .find(|p| p.is_file())?;
    let home = std::fs::canonicalize(javac)
        .ok()?
        .parent()?
        .parent()?
        .to_path_buf();
    home.join("bin/jar").is_file().then_some(home)
}

fn stored(store: &Path, dir: &Path) -> Tree {
    std::fs::create_dir_all(dir).expect("mkdir");
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Builds the library, then the app against it, in a fresh store under
/// `root`. Gives each built jar's digest and size, and the app's tree.
fn build(jdk: &Path, root: &Path) -> (Vec<(String, u64, String)>, PathBuf) {
    let store = root.join("store");
    let s = store.display().to_string();
    let lib_src = root.join("greet");
    write(
        &lib_src.join("src/main/java/dev/gaia/greet/Greet.java"),
        "package dev.gaia.greet;\n\npublic final class Greet {\n    public static String hello(String who) { return \"hello, \" + who; }\n}\n",
    );
    write(
        &lib_src.join("src/main/resources/greet.properties"),
        "greeting=hello\n",
    );
    let app_src = root.join("app");
    write(
        &app_src.join("src/main/java/dev/gaia/app/Main.java"),
        "package dev.gaia.app;\n\nimport dev.gaia.greet.Greet;\n\npublic final class Main {\n    public static void main(String[] args) { System.out.println(Greet.hello(\"image\")); }\n}\n",
    );
    let ctx = Ctx::new();
    let j = jdk.display().to_string();
    let (_, greet) = build_java(
        &s,
        &j,
        stored(&store, &lib_src),
        stored(&store, &root.join("none")),
        "greet",
        17,
        "",
        &ctx,
    )
    .expect("library");
    // The app's dependency jars: what the lock would pin, here the library.
    let libs = root.join("libs");
    std::fs::create_dir_all(&libs).expect("mkdir");
    let handle = Store::open(&store).expect("store");
    std::fs::copy(
        greet.object.path_in(&handle).expect("in"),
        libs.join("greet.jar"),
    )
    .expect("copy");
    let (tree, app) = build_java(
        &s,
        &j,
        stored(&store, &app_src),
        stored(&store, &libs),
        "app",
        17,
        "dev.gaia.app.Main",
        &ctx,
    )
    .expect("app");
    let list = [("greet.jar", &greet), ("app.jar", &app)]
        .map(|(n, a)| {
            let p = a.object.path_in(&handle).expect("in");
            (
                n.to_string(),
                std::fs::metadata(&p).expect("size").len(),
                file_digest(&p).expect("digest").to_string(),
            )
        })
        .to_vec();
    (list, tree.object.path_in(&handle).expect("in"))
}

#[test]
fn a_small_project_builds_offline_twice_to_the_same_digest() {
    let Some(jdk) = jdk() else {
        println!("SKIPPED: no JDK (javac and jar) here");
        return;
    };
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (first, tree) = build(&jdk, a.path());
    let (second, _) = build(&jdk, b.path());
    println!("JDK {}", jdk.display());
    for (name, size, digest) in &first {
        println!("  {name:<10} {size:>6} bytes  {digest}");
    }
    assert_eq!(first, second, "two stores, the same bytes");

    // The output tree runs: the app jar and its dependency side by side.
    let mut listed: Vec<String> = std::fs::read_dir(tree.join("lib"))
        .expect("lib")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    listed.sort();
    assert_eq!(listed, ["app.jar", "greet.jar"]);
    let out = std::process::Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}/lib/*", tree.display()))
        .arg("dev.gaia.app.Main")
        .output()
        .expect("java");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "hello, image");
    // The manifest names the entry point.
    let manifest = std::process::Command::new(jdk.join("bin/jar"))
        .arg("--extract")
        .arg("--file")
        .arg(tree.join("lib/app.jar"))
        .arg("META-INF/MANIFEST.MF")
        .current_dir(a.path())
        .status()
        .expect("jar");
    assert!(manifest.success());
    let text = std::fs::read_to_string(a.path().join("META-INF/MANIFEST.MF")).expect("manifest");
    assert!(text.contains("Main-Class: dev.gaia.app.Main"), "{text}");
}

#[test]
fn a_missing_class_is_a_compile_error_with_its_output() {
    let Some(jdk) = jdk() else {
        println!("SKIPPED: no JDK (javac and jar) here");
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store");
    let src = root.path().join("src");
    write(
        &src.join("Main.java"),
        "public final class Main { Missing m; }\n",
    );
    let error = build_java(
        &store.display().to_string(),
        &jdk.display().to_string(),
        stored(&store, &src),
        stored(&store, &root.path().join("none")),
        "main",
        0,
        "",
        &Ctx::new(),
    )
    .expect_err("fails");
    let text = format!("{error} {}", error.stderr_tail().join("\n"));
    println!("{text}");
    assert!(text.contains("Missing"), "{text}");
}
