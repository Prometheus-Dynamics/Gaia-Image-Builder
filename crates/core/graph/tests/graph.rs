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
[sources.toolchain]
rust = "{RUST}"
targets = ["{triple}"]
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

/// The Rust release the components build with.
const RUST: &str = "1.99.0";

/// The lock entries of `name`'s toolchain for `triple`: each release file,
/// with a digest of its own.
fn toolchain_entries(lock: &mut Lock, name: &str, triple: &str) {
    let pieces = gaia_describe::rust_pieces(RUST, &[triple.to_string()]);
    for (n, piece) in pieces.into_iter().enumerate() {
        lock.entries.insert(
            format!("{name}.toolchain.{}", piece.suffix),
            Entry::Download {
                url: piece.url,
                digest: format!("sha256:{:064x}", n + 1),
            },
        );
    }
}

/// The steps that make `name`'s toolchain for `triple`: the sysroot, then
/// each release file and its unpacking.
fn toolchain_labels(name: &str, triple: &str) -> Vec<String> {
    let host = gaia_describe::rust_host_triple();
    let mut labels = vec![format!("{name}.toolchain")];
    for piece in [
        "rustc".into(),
        "cargo".into(),
        format!("std.{host}"),
        format!("std.{triple}"),
    ] {
        labels.push(format!("{name}.toolchain.{piece}"));
        labels.push(format!("{name}.toolchain.{piece}.unpacked"));
    }
    labels
}

/// `head`, then `name`'s toolchain steps for `triple`, then `tail`.
fn with_toolchain(head: &[&str], name: &str, triple: &str, tail: &[&str]) -> Vec<String> {
    let mut labels: Vec<String> = head.iter().map(|l| l.to_string()).collect();
    labels.extend(toolchain_labels(name, triple));
    labels.extend(tail.iter().map(|l| l.to_string()));
    labels
}

fn lock() -> Lock {
    let mut lock = Lock::default();
    toolchain_entries(&mut lock, "hello-a", "x86_64-unknown-linux-musl");
    toolchain_entries(&mut lock, "hello-b", "aarch64-unknown-linux-musl");
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
    // The copies a shared tree takes: the product's registry has them too.
    registry.register_all(&gaia_types::NODES).expect("copies");
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
            "gaia.build.rust.toolchain",
            "gaia.compress.tar",
            "gaia.source.archive",
            "gaia.source.download",
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
    let mut want = with_toolchain(
        &[
            "host",
            "hello-a.build",
            "hello-a.src",
            "hello-a.root",
            "hello-a.vendor",
        ],
        "hello-a",
        "x86_64-unknown-linux-musl",
        &["a::tar"],
    );
    want.extend(with_toolchain(
        &[
            "hello-b.build",
            "hello-b.src",
            "hello-b.root",
            "hello-b.vendor",
        ],
        "hello-b",
        "aarch64-unknown-linux-musl",
        &["b::tar"],
    ));
    assert_eq!(labels, want);
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
    for root in [3, 17] {
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
    assert_eq!(explain.nodes.len(), 29);
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
    let mut want = with_toolchain(
        &[
            "host",
            "hello-a.build",
            "hello-a.src",
            "hello-a.root",
            "hello-a.vendor",
        ],
        "hello-a",
        "x86_64-unknown-linux-musl",
        &[],
    );
    want.extend(with_toolchain(
        &[
            "hello-b.build",
            "hello-b.src",
            "hello-b.root",
            "hello-b.vendor",
        ],
        "hello-b",
        "aarch64-unknown-linux-musl",
        &[],
    ));
    assert_eq!(labels, want);
    assert_eq!(cut.origins.len(), labels.len());
    let mut edges: Vec<(u32, u32)> = cut
        .doc
        .graph
        .edges
        .iter()
        .map(|e| (e.from.node, e.to.node))
        .collect();
    edges.sort_unstable();
    // Renumbered: each build takes its source, root, vendor and toolchain;
    // each toolchain takes its four unpacked files, each unpacked from its
    // download.
    let mut want = Vec::new();
    for build in [1, 14] {
        let toolchain = build + 4;
        want.extend((1..=4).map(|n| (build + n, build)));
        for n in 0..4 {
            let download = toolchain + 1 + 2 * n;
            want.push((download, download + 1));
            want.push((download + 1, toolchain));
        }
    }
    want.sort_unstable();
    assert_eq!(edges, want);
    assert!(cut.doc.graph.nodes[0].port_types.is_empty(), "no output");
    let requires: Vec<&str> = cut.doc.requires.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        requires,
        [
            "gaia.build.rust",
            "gaia.build.rust.toolchain",
            "gaia.source.archive",
            "gaia.source.download",
            "gaia.source.git",
            "gaia.source.oci",
            "gaia.source.path"
        ]
    );
    // The planner takes what is left.
    let planned = cut.plan(&registry).expect("plans");
    assert_eq!(explain_plan(&planned).nodes.len(), 27);

    // One step by its name: that build and what it reads, not the other.
    let one = whole.clone().until("hello-b.build").expect("cut by name");
    let labels: Vec<&str> = one
        .doc
        .graph
        .nodes
        .iter()
        .map(|n| n.label.as_deref().unwrap_or("host"))
        .collect();
    let want = with_toolchain(
        &[
            "host",
            "hello-b.build",
            "hello-b.src",
            "hello-b.root",
            "hello-b.vendor",
        ],
        "hello-b",
        "aarch64-unknown-linux-musl",
        &[],
    );
    assert_eq!(labels, want);

    let why = whole.until("gaia.os.buildroot.host").expect_err("no OS");
    assert!(
        why.starts_with("the product has no step `gaia.os.buildroot.host`; its steps are gaia.build.rust, gaia.build.rust.toolchain, gaia.compress.tar"),
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

    // The kept cargo target of `gaia build --dev-reuse`, set by hand: an
    // input of the node, and still refused, so a description can never
    // make a clean, release or signed build start from one.
    let e = try_with(
        &base.replace("features = \"\"", "features = \"\"\ndev = \"/kept\""),
        lock(),
    );
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: build.settings.dev: `dev` is set by `gaia build --dev-reuse` only"),
        "{e}"
    );

    // A Rust build with no toolchain of its own: the build machine's is
    // not taken in its place.
    let toolchain = format!(
        "[sources.toolchain]\nrust = \"{RUST}\"\ntargets = [\"aarch64-unknown-linux-musl\"]\n"
    );
    assert!(base.contains(&toolchain));
    let e = try_with(&base.replace(&toolchain, ""), lock());
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: sources.toolchain: hello-b's Rust build takes its toolchain from a source: `[sources.toolchain]` with `rust = \"<release>\"` and `targets = [\"aarch64-unknown-linux-musl\"]`; the build machine's Rust is not used"),
        "{e}"
    );

    // The same for a build for the build machine itself (a generator's
    // tool): the description is not asked to name the machine's triple.
    let host = gaia_describe::rust_host_triple();
    let e = try_with(
        &base.replace(&toolchain, "").replace(
            "triple = \"aarch64-unknown-linux-musl\"",
            &format!("triple = \"{host}\""),
        ),
        lock(),
    );
    println!("{}", shown(&e));
    assert!(
        e.contains("`rust = \"<release>\"` and `targets = []` (it is built for the build machine, whose std is always pinned); the build machine's Rust is not used"),
        "{e}"
    );
    assert!(!e.contains(&host), "{e}");

    // A toolchain with no std for the build's target, refused by name when
    // the graph is laid out, before anything runs.
    let other = toolchain.replace("aarch64-unknown-linux-musl", "riscv64gc-unknown-linux-musl");
    let e = try_with(&base.replace(&toolchain, &other), lock());
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: sources.toolchain.targets: hello-b's build is for aarch64-unknown-linux-musl, and its toolchain (Rust 1.99.0) pins no std for it: add \"aarch64-unknown-linux-musl\" to `targets`, then run `gaia lock`"),
        "{e}"
    );

    // A release file the lock does not pin.
    let mut partial = lock();
    partial
        .entries
        .remove("hello-b.toolchain.std.aarch64-unknown-linux-musl");
    let e = try_with(&base, partial);
    println!("{}", shown(&e));
    assert!(
        e.contains("hello-b/component.toml: sources.toolchain: hello-b.toolchain.std.aarch64-unknown-linux-musl is not in the lock"),
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

/// The Raze's Lemnos packages: one source and root, one target and profile,
/// one image each on one board. `odd` gives one of them other features.
const LEMNOS: [&str; 4] = ["lemnosd", "lemnos-ctl", "lemnos-orion", "lemnos-slot-init"];

fn lemnos(dir: &Path, odd: Option<(&str, &str)>) -> (PathBuf, PathBuf) {
    write(
        &dir.join("boards/a55/board.toml"),
        "version = 1\nname = \"a55\"\n[target]\narch = \"aarch64\"\ncpu = \"cortex-a55\"\nos = \"linux\"\n",
    );
    write(
        &dir.join("recipes/tarball.toml"),
        "version = 1\nname = \"tarball\"\ndescription = \"The built tree, as a sorted tar.\"\n[stages]\ntar = \"gaia.compress.tar\"\n[inputs]\nbuild = [\"tar.tree\"]\n[outputs]\ntar = \"tar.archive\"\n",
    );
    let mut components = String::new();
    let mut images = String::new();
    for name in LEMNOS {
        let features = match odd {
            Some((odd_name, features)) if odd_name == name => features,
            _ => "",
        };
        write(
            &dir.join(format!("components/{name}/component.toml")),
            &format!(
                r#"version = 1
name = "{name}"
[sources.src]
git = "{URL}"
ref = "master"
[sources.root]
image = "{IMAGE}"
platform = "host"
[sources.toolchain]
rust = "{RUST}"
targets = ["aarch64-unknown-linux-musl"]
[build]
builder = "rust"
source = "src"
[build.settings]
package = "{name}"
features = "{features}"
triple = "aarch64-unknown-linux-musl"
profile = "service"
"#
            ),
        );
        components.push_str(&format!("{name} = \"components/{name}/component.toml\"\n"));
        images.push_str(&format!(
            "[[images]]\nname = \"{name}\"\nboard = \"boards/a55/board.toml\"\ncomponents = [\"{name}\"]\nrecipe = \"tarball\"\n\n"
        ));
    }
    write(
        &dir.join("product.toml"),
        &format!("version = 1\nname = \"lemnos\"\n[components]\n{components}\n{images}"),
    );
    (dir.join("product.toml"), dir.join("recipes"))
}

fn lemnos_lock() -> Lock {
    lemnos_lock_at(None)
}

/// The lock of the Lemnos product; `other` is one package whose source is at
/// another commit.
fn lemnos_lock_at(other: Option<(&str, &str)>) -> Lock {
    let mut lock = Lock::default();
    for name in LEMNOS {
        toolchain_entries(&mut lock, name, "aarch64-unknown-linux-musl");
        let commit = match other {
            Some((odd, commit)) if odd == name => commit,
            _ => COMMIT,
        };
        lock.entries.insert(
            format!("{name}.src"),
            Entry::Git {
                url: URL.into(),
                reference: "master".into(),
                commit: commit.into(),
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

/// The four Lemnos packages plan one build node, and each component takes
/// its own package's part of it.
#[test]
fn the_lemnos_packages_plan_one_build_and_a_part_each() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = lemnos(dir.path(), None);
    let loaded = Product::load(&product).expect("loads");
    loaded.wants();
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let registry = registry();
    let built = build(
        &loaded,
        &lemnos_lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let nodes = &built.doc.graph.nodes;
    let builds: Vec<usize> = (0..nodes.len())
        .filter(|&i| nodes[i].id == "gaia.build.rust")
        .collect();
    assert_eq!(builds.len(), 1, "one cargo run for the four packages");
    assert_eq!(
        nodes[builds[0]].consts.get("package"),
        Some(&Lit::Str(
            "lemnos-ctl,lemnos-orion,lemnos-slot-init,lemnosd".into()
        ))
    );
    let parts: Vec<usize> = (0..nodes.len())
        .filter(|&i| nodes[i].id == "gaia.build.rust.part")
        .collect();
    assert_eq!(parts.len(), 4);
    for name in LEMNOS {
        let label = format!("{name}.part");
        let part = parts
            .iter()
            .copied()
            .find(|&i| nodes[i].label.as_deref() == Some(label.as_str()))
            .unwrap_or_else(|| panic!("no part {label}"));
        assert_eq!(
            nodes[part].consts.get("package"),
            Some(&Lit::Str(name.into())),
            "{label}"
        );
        let fed_by_group = built
            .doc
            .graph
            .edges
            .iter()
            .any(|e| e.to.node as usize == part && e.from.node as usize == builds[0]);
        assert!(fed_by_group, "{label} is not fed by the group build");
    }
    // The planner takes it.
    built.plan(&registry).expect("plans");
}

/// A package whose features differ is built in its own cargo run; the other
/// three still share one.
#[test]
fn a_package_with_other_features_is_not_in_the_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = lemnos(dir.path(), Some(("lemnos-ctl", "fake-devices")));
    let loaded = Product::load(&product).expect("loads");
    loaded.wants();
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let registry = registry();
    let built = build(
        &loaded,
        &lemnos_lock(),
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let nodes = &built.doc.graph.nodes;
    let packages: Vec<String> = (0..nodes.len())
        .filter(|&i| nodes[i].id == "gaia.build.rust")
        .map(|i| match nodes[i].consts.get("package") {
            Some(Lit::Str(text)) => text.clone(),
            other => panic!("no package: {other:?}"),
        })
        .collect();
    assert_eq!(
        packages,
        ["lemnos-orion,lemnos-slot-init,lemnosd", "lemnos-ctl"]
    );
    let parts = (0..nodes.len())
        .filter(|&i| nodes[i].id == "gaia.build.rust.part")
        .count();
    assert_eq!(parts, 3, "the three share a group; the other builds alone");
}

/// A package whose source is another commit is built on its own.
#[test]
fn a_package_at_another_commit_is_not_in_the_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (product, recipes) = lemnos(dir.path(), None);
    let loaded = Product::load(&product).expect("loads");
    loaded.wants();
    let recipes = Recipes::load_dir(&recipes).expect("recipes");
    let registry = registry();
    let lock = lemnos_lock_at(Some((
        "lemnos-orion",
        "455a956e13e62c97081d66e0e8f3bf2aced1eac8",
    )));
    let built = build(
        &loaded,
        &lock,
        &recipes,
        &registry,
        &setup(),
        &Builders::default(),
    )
    .expect("builds");
    let nodes = &built.doc.graph.nodes;
    let packages: Vec<String> = (0..nodes.len())
        .filter(|&i| nodes[i].id == "gaia.build.rust")
        .map(|i| match nodes[i].consts.get("package") {
            Some(Lit::Str(text)) => text.clone(),
            other => panic!("no package: {other:?}"),
        })
        .collect();
    assert_eq!(
        packages,
        ["lemnos-ctl,lemnos-slot-init,lemnosd", "lemnos-orion"]
    );
}
