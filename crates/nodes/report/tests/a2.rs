//! The A2 product's two images, reported, and the delta of a changed run
//! against the first. Clones a public repository and builds it with a Rust
//! toolchain, so it is ignored by default:
//!
//! ```text
//! GAIA_TOOLCHAIN=$(rustc --print sysroot) \
//!   cargo test -p gaia-report --test a2 -- --ignored --nocapture
//! ```
//!
//! `GAIA_E14_STORE` names the store (a temporary one when not set).
//!
//! The changed run adds one component, `motd`, with one file, to image a's
//! merged tree. The delta must name exactly that path.

use gaia_build_rust::build_rust;
use gaia_compress::tar_tree;
use gaia_lock::{Entry, Lock};
use gaia_report::{Report, render_text, report_layers};
use gaia_source_files::source_path;
use gaia_source_git::source as source_git;
use gaia_source_oci::source as source_oci;
use gaia_step::{Ctx, Store};
use gaia_tree::merge_layers;
use gaia_types::Layer;
use gaia_types::{Artifact, Tree};
use std::path::Path;

const URL: &str = "https://github.com/vasilakisfil/hello.service";
const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";
/// The image a builder runs in, pinned by its index digest.
const IMAGE: &str = "docker.io/library/debian:trixie-slim";
const IMAGE_DIGEST: &str =
    "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";

/// One component built from the pinned commit for its target.
fn component(
    store: &str,
    scratch: &Path,
    toolchain: &str,
    name: &str,
    triple: &str,
    cpu: &str,
) -> Layer {
    let ctx = Ctx::new();
    let vendor = scratch.join(format!("{name}-vendor"));
    std::fs::create_dir_all(&vendor).expect("mkdir");
    let git = which_git();
    let src: Tree = source_git(store, &git, URL, COMMIT, &ctx).expect("source");
    let vendor: Tree = source_path(store, &vendor.display().to_string(), &ctx).expect("vendor");
    // The builder's root is pulled for the machine running the build: the
    // tools run here and only the target is cross-compiled.
    let platform = match std::env::consts::ARCH {
        "x86_64" => "linux/amd64",
        "aarch64" => "linux/arm64",
        other => panic!("no image platform for host arch {other}"),
    };
    let (root, _) = source_oci(
        store,
        "/usr/bin/curl",
        IMAGE,
        IMAGE_DIGEST,
        platform,
        "",
        &ctx,
    )
    .expect("root image");
    let (tree, _) = build_rust(
        store,
        "/usr/bin/bwrap",
        root,
        toolchain,
        src,
        vendor,
        "hello-world",
        "",
        triple,
        cpu,
        &ctx,
    )
    .expect("build");
    Layer {
        owner: name.into(),
        tree,
        replaces: Vec::new(),
    }
}

/// A component that is only a file, stored as a tree.
fn files_layer(store: &str, scratch: &Path, owner: &str, path: &str, text: &str) -> Layer {
    let dir = scratch.join(format!("{owner}-files"));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, text).expect("write");
    let store = Store::open(store).expect("store");
    Layer {
        owner: owner.into(),
        tree: Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        },
        replaces: Vec::new(),
    }
}

/// The image archive of `layers` merged, as `gaia.tree.merge` and
/// `gaia.compress.tar` would make it.
fn image(store: &str, layers: &[Layer]) -> Artifact {
    let ctx = Ctx::new();
    let (merged, _owners) = merge_layers(store, layers, &ctx).expect("merge");
    tar_tree(store, merged, &ctx).expect("tar")
}

fn which_git() -> String {
    for candidate in ["/usr/bin/git", "/bin/git"] {
        if Path::new(candidate).exists() {
            return candidate.into();
        }
    }
    panic!("no git at /usr/bin/git or /bin/git");
}

fn lock() -> String {
    let mut lock = Lock::default();
    for name in ["hello-a", "hello-b"] {
        lock.insert(
            &format!("{name}.src"),
            Entry::Git {
                url: URL.into(),
                reference: "master".into(),
                commit: COMMIT.into(),
            },
        );
        lock.insert(
            &format!("{name}.root"),
            Entry::Image {
                reference: IMAGE.into(),
                digest: IMAGE_DIGEST.into(),
            },
        );
    }
    lock.render()
}

fn read(store: &str, artifact: &Artifact) -> Report {
    let store = Store::open(store).expect("store");
    let text =
        std::fs::read(store.path(&artifact.object.object_ref().expect("ref"))).expect("read");
    serde_json::from_slice(&text).expect("json")
}

#[test]
#[ignore = "clones hello.service and builds it: run with --ignored"]
fn the_a2_product_report_and_a_changed_runs_delta() {
    let toolchain = std::env::var("GAIA_TOOLCHAIN").unwrap_or_else(|_| {
        let out = std::process::Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .expect("rustc");
        String::from_utf8(out.stdout)
            .expect("utf-8")
            .trim()
            .to_string()
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let store = std::env::var("GAIA_E14_STORE")
        .unwrap_or_else(|_| dir.path().join("store").display().to_string());
    let scratch = dir.path();
    let ctx = Ctx::new();

    // Run one: image a is hello-a for x86_64, image b is hello-b for aarch64.
    let a1 = component(
        &store,
        scratch,
        &toolchain,
        "hello-a",
        "x86_64-unknown-linux-musl",
        "",
    );
    let b1 = component(
        &store,
        scratch,
        &toolchain,
        "hello-b",
        "aarch64-unknown-linux-musl",
        "cortex-a55",
    );
    let img_a1 = image(&store, std::slice::from_ref(&a1));
    let img_b1 = image(&store, std::slice::from_ref(&b1));
    let report_a1 =
        report_layers(&store, "a", &[a1], &img_a1, &lock(), "", &ctx).expect("report a");
    let report_b1 =
        report_layers(&store, "b", &[b1], &img_b1, &lock(), "", &ctx).expect("report b");
    println!(
        "== run 1: image a ==\n{}",
        render_text(&read(&store, &report_a1))
    );
    println!(
        "== run 1: image b ==\n{}",
        render_text(&read(&store, &report_b1))
    );

    // Run two: image a gains the `motd` component; hello-b is unchanged.
    let a2 = component(
        &store,
        scratch,
        &toolchain,
        "hello-a",
        "x86_64-unknown-linux-musl",
        "",
    );
    let motd = files_layer(&store, scratch, "motd", "etc/motd", "welcome\n");
    let b2 = component(
        &store,
        scratch,
        &toolchain,
        "hello-b",
        "aarch64-unknown-linux-musl",
        "cortex-a55",
    );
    let img_a2 = image(&store, &[a2.clone(), motd.clone()]);
    let img_b2 = image(&store, std::slice::from_ref(&b2));
    let previous_a = report_a1.object.object_ref().expect("ref").to_string();
    let previous_b = report_b1.object.object_ref().expect("ref").to_string();
    let report_a2 = report_layers(
        &store,
        "a",
        &[a2, motd],
        &img_a2,
        &lock(),
        &previous_a,
        &ctx,
    )
    .expect("report a");
    let report_b2 =
        report_layers(&store, "b", &[b2], &img_b2, &lock(), &previous_b, &ctx).expect("report b");
    println!(
        "== run 2: image a (motd added) ==\n{}",
        render_text(&read(&store, &report_a2))
    );
    println!(
        "== run 2: image b, unchanged ==\n{}",
        render_text(&read(&store, &report_b2))
    );

    assert_eq!(img_b1, img_b2, "image b is the same archive in both runs");
    let delta_b = read(&store, &report_b2).delta.expect("delta");
    assert!(delta_b.changed.is_empty() && delta_b.added.is_empty() && delta_b.removed.is_empty());
    assert!(delta_b.components.is_empty() && delta_b.sources.is_empty());

    let delta_a = read(&store, &report_a2).delta.expect("delta");
    assert_eq!(
        delta_a.added,
        ["etc/motd"],
        "the delta names the added file"
    );
    assert!(delta_a.removed.is_empty() && delta_a.changed.is_empty());
    assert!(delta_a.sources.is_empty(), "the same locked sources");
    assert_eq!(delta_a.components.len(), 1, "only motd's owner changed");
    assert_eq!(delta_a.components[0].owner, "motd");
    assert_eq!(
        (delta_a.components[0].before, delta_a.components[0].after),
        (0, 8)
    );
    assert!(delta_a.image_change > 0, "the image grew");
}
