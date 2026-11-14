//! The A2 product as descriptions: two components, each cloned at a pinned
//! commit, built with cargo and packed into a tar, as one planned graph.

use daedalus::layers::graph::Lit;
use daedalus::layers::planner::explain::explain_plan;
use daedalus::prelude::Registry;
use gaia_describe::Product;
use gaia_graph::{Builders, FUSION, STEP, Setup, build};
use gaia_lock::{Entry, Lock};
use gaia_recipe::Recipes;
use std::path::{Path, PathBuf};

const URL: &str = "https://github.com/vasilakisfil/hello.service";
const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// The image a builder runs in: debian:trixie-slim, pinned by its index digest.
const IMAGE: &str = "docker.io/library/debian:trixie-slim";
const IMAGE_DIGEST: &str =
    "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";

fn component(triple: &str, cpu: &str, name: &str) -> String {
    format!(
        r#"version = 1
name = "{name}"
[sources.src]
git = "{URL}"
ref = "master"
[sources.vendor]
path = "vendor"
[sources.root]
image = "{IMAGE}"
platform = "host"
[build]
builder = "rust"
source = "src"
[build.settings]
package = "hello-world"
features = ""
triple = "{triple}"
cpu = "{cpu}"
"#
    )
}

/// The descriptions, the recipe and the lock, under `dir`.
fn describe(dir: &Path) -> (PathBuf, PathBuf) {
    write(
        &dir.join("boards/pc/board.toml"),
        "version = 1\nname = \"pc\"\n[target]\narch = \"x86_64\"\nos = \"linux\"\n",
    );
    write(
        &dir.join("boards/a55/board.toml"),
        "version = 1\nname = \"a55\"\n[target]\narch = \"aarch64\"\ncpu = \"cortex-a55\"\nos = \"linux\"\n",
    );
    write(
        &dir.join("components/hello-a/component.toml"),
        &component("x86_64-unknown-linux-musl", "", "hello-a"),
    );
    write(
        &dir.join("components/hello-b/component.toml"),
        &component("aarch64-unknown-linux-musl", "cortex-a55", "hello-b"),
    );
    std::fs::create_dir_all(dir.join("components/hello-a/vendor")).expect("mkdir");
    std::fs::create_dir_all(dir.join("components/hello-b/vendor")).expect("mkdir");
    write(
        &dir.join("recipes/tarball.toml"),
        "version = 1\nname = \"tarball\"\ndescription = \"The built tree, as a sorted tar.\"\n[stages]\ntar = \"gaia.compress.tar\"\n[inputs]\nbuild = [\"tar.tree\"]\n[outputs]\ntar = \"tar.archive\"\n",
    );
    write(
        &dir.join("product.toml"),
        r#"version = 1
name = "hello"
[components]
hello-a = "components/hello-a/component.toml"
hello-b = "components/hello-b/component.toml"
[[images]]
name = "a"
board = "boards/pc/board.toml"
components = ["hello-a"]
recipe = "tarball"
[[images]]
name = "b"
board = "boards/a55/board.toml"
components = ["hello-b"]
recipe = "tarball"
"#,
    );
    (dir.join("product.toml"), dir.join("recipes"))
}

fn lock() -> Lock {
    let mut lock = Lock::default();
    for name in ["hello-a", "hello-b"] {
        lock.entries.insert(
            format!("{name}.src"),
            Entry::Git {
                url: URL.into(),
                reference: "master".into(),
                commit: COMMIT.into(),
            },
        );
        lock.entries.insert(
            format!("{name}.root"),
            Entry::Image {
                reference: IMAGE.into(),
                digest: IMAGE_DIGEST.into(),
            },
        );
    }
    lock
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register_all(&gaia_source_git::NODES).expect("git");
    registry
        .register_all(&gaia_source_files::NODES)
        .expect("files");
    registry
        .register_all(&gaia_build_rust::NODES)
        .expect("rust");
    registry
        .register_all(&gaia_compress::NODES)
        .expect("compress");
    registry.register_all(&gaia_source_oci::NODES).expect("oci");
    registry
}

fn setup() -> Setup {
    Setup {
        store: PathBuf::from("/var/lib/gaia/store"),
        tools: [
            ("git", "/usr/bin/git"),
            ("curl", "/usr/bin/curl"),
            ("bwrap", "/usr/bin/bwrap"),
            ("toolchain", "/opt/rust/1.99.0"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        buildroot: None,
        inputs: Default::default(),
    }
}

#[test]
fn the_a2_product_is_one_planned_graph() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = describe(dir.path());
    let loaded = Product::load(&product).expect("loads");
    loaded.wants();
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let registry = registry();
    let built = build(
        &loaded,
        &lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let text = built
        .doc
        .to_json()
        .expect("json")
        .replace(&dir.path().display().to_string(), "<descriptions>");
    println!("{text}");

    assert_eq!(built.doc.format, "daedalus.graph");
    let requires: Vec<&str> = built.doc.requires.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        requires,
        [
            "gaia.build.rust",
            "gaia.compress.tar",
            "gaia.source.git",
            "gaia.source.oci",
            "gaia.source.path"
        ]
    );
    let labels: Vec<&str> = built
        .doc
        .graph
        .nodes
        .iter()
        .map(|n| n.label.as_deref().unwrap_or("host"))
        .collect();
    assert_eq!(
        labels,
        [
            "host",
            "hello-a.build",
            "hello-a.src",
            "hello-a.root",
            "hello-a.vendor",
            "a::tar",
            "hello-b.build",
            "hello-b.src",
            "hello-b.root",
            "hello-b.vendor",
            "b::tar"
        ]
    );
    for node in &built.doc.graph.nodes[1..] {
        assert_eq!(
            node.metadata.get(FUSION),
            Some(&Lit::Bool(false)),
            "{:?}",
            node.label
        );
        assert_eq!(
            node.metadata.get(STEP),
            node.label.clone().map(Lit::Str).as_ref()
        );
    }
    // A builder's root is pulled for the machine running the build, also on
    // the aarch64 board.
    let host = match std::env::consts::ARCH {
        "x86_64" => "linux/amd64",
        "aarch64" => "linux/arm64",
        other => panic!("no platform for {other}"),
    };
    for root in [3, 8] {
        let node = &built.doc.graph.nodes[root];
        assert_eq!(
            node.consts.get("platform"),
            Some(&Lit::Str(host.into())),
            "{:?}",
            node.label
        );
    }
    let git = &built.doc.graph.nodes[2];
    assert_eq!(git.consts.get("commit"), Some(&Lit::Str(COMMIT.into())));

    // The planner takes it; both chains are their own steps.
    let planned = built.plan(&registry).expect("plans");
    let explain = explain_plan(&planned);
    println!("{}", serde_json::to_string_pretty(&explain).expect("json"));
    assert_eq!(explain.nodes.len(), 11);
}

/// A product cut down to one kind of step: those steps, what they need, and
/// nothing given back to the host.
#[test]
fn a_graph_is_cut_down_to_a_step_and_what_it_needs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = describe(dir.path());
    let loaded = Product::load(&product).expect("loads");
    loaded.wants();
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let registry = registry();
    let whole = build(
        &loaded,
        &lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let cut = whole.clone().until("gaia.build.rust").expect("cut");
    let labels: Vec<&str> = cut
        .doc
        .graph
        .nodes
        .iter()
        .map(|n| n.label.as_deref().unwrap_or("host"))
        .collect();
    // Each build and what it reads: no tar after it.
    assert_eq!(
        labels,
        [
            "host",
            "hello-a.build",
            "hello-a.src",
            "hello-a.root",
            "hello-a.vendor",
            "hello-b.build",
            "hello-b.src",
            "hello-b.root",
            "hello-b.vendor"
        ]
    );
    assert_eq!(cut.origins.len(), labels.len());
    let mut edges: Vec<(u32, u32)> = cut
        .doc
        .graph
        .edges
        .iter()
        .map(|e| (e.from.node, e.to.node))
        .collect();
    edges.sort_unstable();
    // Renumbered: every edge is into one of the two builds.
    assert_eq!(edges, [(2, 1), (3, 1), (4, 1), (6, 5), (7, 5), (8, 5)]);
    assert!(cut.doc.graph.nodes[0].port_types.is_empty(), "no output");
    let requires: Vec<&str> = cut.doc.requires.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        requires,
        [
            "gaia.build.rust",
            "gaia.source.git",
            "gaia.source.oci",
            "gaia.source.path"
        ]
    );
    // The planner takes what is left.
    let planned = cut.plan(&registry).expect("plans");
    assert_eq!(explain_plan(&planned).nodes.len(), 9);

    let why = whole.until("gaia.os.buildroot.host").expect_err("no OS");
    assert!(
        why.starts_with("the product has no step `gaia.os.buildroot.host`; its steps are gaia.build.rust, gaia.compress.tar"),
        "{why}"
    );
}

#[test]
fn errors_name_the_description() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes_dir) = describe(dir.path());
    let recipes = Recipes::load_dir(&recipes_dir).expect("recipes");
    let registry = registry();
    let component = dir.path().join("components/hello-b/component.toml");
    let base = std::fs::read_to_string(&component).expect("read");
    let try_with = |text: &str, lock: Lock| {
        write(&component, text);
        let loaded = Product::load(&product).expect("loads");
        let result = build(
            &loaded,
            &lock,
            &recipes,
            &registry,
            &setup(),
            &Builders::default(),
        )
        .and_then(|built| built.plan(&registry).map(drop));
        result.expect_err("fails").to_string()
    };
    let shown = |e: &str| e.replace(&dir.path().display().to_string(), "<descriptions>");

    // A setting that is not an input of the builder.
    let e = try_with(&base.replace("package =", "pakage ="), lock());
    println!("{}", shown(&e));
    assert!(
        e.contains(
            "hello-b/component.toml: build.settings.pakage: gaia.build.rust has no input `pakage`"
        ),
        "{e}"
    );

    // A source the lock does not pin.
    let mut partial = lock();
    partial.entries.remove("hello-b.src");
    let e = try_with(&base, partial);
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: sources.src: hello-b.src is not in the lock"),
        "{e}"
    );

    // An input nothing gives.
    let e = try_with(&base.replace("package = \"hello-world\"\n", ""), lock());
    println!("{}", shown(&e));
    assert!(
        e.contains(
            "hello-b/component.toml: step hello-b.build (gaia.build.rust) needs its input `package`"
        ),
        "{e}"
    );

    // A setting of the wrong type, and one Daedalus cannot take as a
    // constant.
    let e = try_with(
        &base.replace("package = \"hello-world\"", "package = 7"),
        lock(),
    );
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: build.settings.package: `package` takes a string, not the int 7"),
        "{e}"
    );

    // A port a recipe names that the node lacks names the recipe.
    write(&component, &base);
    let recipe = recipes_dir.join("tarball.toml");
    let text = std::fs::read_to_string(&recipe).expect("read");
    write(&recipe, &text.replace("tar.tree\"", "tar.trees\""));
    let recipes = Recipes::load_dir(&recipes_dir).expect("recipes");
    let loaded = Product::load(&product).expect("loads");
    let e = build(
        &loaded,
        &lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect_err("fails")
    .to_string();
    println!("{}", shown(&e));
    assert!(
        e.contains("recipes/tarball.toml: step a::tar (gaia.compress.tar) needs its input `tree`"),
        "{e}"
    );

    // What only the planner sees, a tree into an artifact input, names the
    // descriptions of the nodes it names.
    write(
        &recipe,
        "version = 1\nname = \"tarball\"\n[stages]\nunpack = \"gaia.source.archive\"\n[inputs]\nbuild = [\"unpack.archive\"]\n[outputs]\ntree = \"unpack.tree\"\n[consts]\n\"unpack.strip\" = 0\n",
    );
    let recipes = Recipes::load_dir(&recipes_dir).expect("recipes");
    let loaded = Product::load(&product).expect("loads");
    let e = build(
        &loaded,
        &lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .and_then(|built| built.plan(&registry).map(drop))
    .expect_err("fails")
    .to_string();
    println!("{}", shown(&e));
    assert!(e.contains("the planner refused the graph"), "{e}");
    assert!(e.contains("recipes/tarball.toml"), "{e}");
}

#[test]
fn a_git_source_subdir_is_its_node_setting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = describe(dir.path());
    // hello-a's source names a directory of its checkout.
    write(
        &dir.path().join("components/hello-a/component.toml"),
        &component("x86_64-unknown-linux-musl", "", "hello-a").replacen(
            "ref = \"master\"\n",
            "ref = \"master\"\nsubdir = \"a/b\"\n",
            1,
        ),
    );
    let loaded = Product::load(&product).expect("loads");
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let built = build(
        &loaded,
        &lock(),
        &recipes,
        &registry(),
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let node = |label: &str| {
        built
            .doc
            .graph
            .nodes
            .iter()
            .find(|n| n.label.as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no node {label}"))
    };
    assert_eq!(
        node("hello-a.src").consts.get("subdir"),
        Some(&Lit::Str("a/b".into()))
    );
    assert_eq!(node("hello-b.src").consts.get("subdir"), None);
}

#[test]
fn an_image_written_with_its_digest_hands_the_oci_node_the_bare_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = describe(dir.path());
    let digested = format!("{IMAGE}@{IMAGE_DIGEST}");
    write(
        &dir.path().join("components/hello-a/component.toml"),
        &component("x86_64-unknown-linux-musl", "", "hello-a").replacen(
            &format!("image = \"{IMAGE}\""),
            &format!("image = \"{digested}\""),
            1,
        ),
    );
    let mut lock = lock();
    lock.entries.insert(
        "hello-a.root".into(),
        Entry::Image {
            reference: digested.clone(),
            digest: IMAGE_DIGEST.into(),
        },
    );
    let loaded = Product::load(&product).expect("loads");
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let built = build(
        &loaded,
        &lock,
        &recipes,
        &registry(),
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let node = built
        .doc
        .graph
        .nodes
        .iter()
        .find(|n| n.label.as_deref() == Some("hello-a.root"))
        .expect("the image node");
    assert_eq!(node.consts.get("reference"), Some(&Lit::Str(IMAGE.into())));
    assert_eq!(
        node.consts.get("digest"),
        Some(&Lit::Str(IMAGE_DIGEST.into()))
    );
}
