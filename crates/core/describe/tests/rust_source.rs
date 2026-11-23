//! A `rust` source: a Rust release and the targets whose std it pins, each
//! an official file the lock holds by digest.

use gaia_describe::{Component, Source, rust_host_triple};
use gaia_lock::Want;

const HEAD: &str = "version = 1\nname = \"c\"\n[sources.toolchain]\n";

fn read(toolchain: &str) -> Result<Component, gaia_describe::Error> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("component.toml");
    std::fs::write(&path, format!("{HEAD}{toolchain}")).expect("write");
    Component::read(&path)
}

#[test]
fn a_release_wants_rustc_cargo_and_the_std_of_each_target_and_the_host() {
    let component = read(
        "rust = \"1.99.0\"\ntargets = [\"riscv64gc-unknown-linux-musl\", \"aarch64-unknown-linux-musl\", \"aarch64-unknown-linux-musl\"]\n",
    )
    .expect("reads");
    // Sorted, once each.
    assert_eq!(
        component.sources["toolchain"],
        Source::Rust {
            version: "1.99.0".into(),
            targets: vec![
                "aarch64-unknown-linux-musl".into(),
                "riscv64gc-unknown-linux-musl".into()
            ],
        }
    );
    let host = rust_host_triple();
    let dist = "https://static.rust-lang.org/dist";
    let mut want = vec![
        (
            "c.toolchain.rustc".to_string(),
            format!("{dist}/rustc-1.99.0-{host}.tar.xz"),
        ),
        (
            "c.toolchain.cargo".to_string(),
            format!("{dist}/cargo-1.99.0-{host}.tar.xz"),
        ),
    ];
    let mut stds = vec![
        "aarch64-unknown-linux-musl".to_string(),
        "riscv64gc-unknown-linux-musl".to_string(),
        host.clone(),
    ];
    stds.sort();
    stds.dedup();
    for triple in stds {
        want.push((
            format!("c.toolchain.std.{triple}"),
            format!("{dist}/rust-std-1.99.0-{triple}.tar.xz"),
        ));
    }
    let wants: Vec<(String, String)> = component
        .wants()
        .into_iter()
        .map(|(name, want)| match want {
            Want::Download { url } => (name, url),
            other => panic!("{name} wants {other:?}, not a download"),
        })
        .collect();
    let mut sorted = want.clone();
    sorted.sort();
    let mut got = wants.clone();
    got.sort();
    assert_eq!(got, sorted);
}

#[test]
fn a_bad_release_or_triple_or_a_mixed_source_is_refused_by_name() {
    for (text, says) in [
        (
            "rust = \"1.99\"\ntargets = []\n",
            "`1.99` is not a Rust release",
        ),
        (
            "rust = \"stable\"\ntargets = []\n",
            "`stable` is not a Rust release",
        ),
        (
            "rust = \"1.99.x\"\ntargets = []\n",
            "`1.99.x` is not a Rust release",
        ),
        (
            "rust = \"1..0\"\ntargets = []\n",
            "`1..0` is not a Rust release",
        ),
        (
            "rust = \"1.99.0\"\ntargets = [\"Aarch64-linux\"]\n",
            "`Aarch64-linux` is not a target triple",
        ),
        (
            "rust = \"1.99.0\"\ntargets = [\"aarch64\"]\n",
            "`aarch64` is not a target triple",
        ),
        (
            "rust = \"1.99.0\"\ntargets = [\"aarch64--musl\"]\n",
            "`aarch64--musl` is not a target triple",
        ),
        ("rust = \"1.99.0\"\n", "a rust source needs `targets`"),
        (
            "rust = \"1.99.0\"\ntargets = []\ngit = \"https://example.org/x.git\"\nref = \"v1\"\n",
            "only one of `git`, `download`, `image`, `path` and `rust`",
        ),
        (
            "rust = \"1.99.0\"\ntargets = []\nref = \"v1\"\n",
            "a rust source has only `rust` and `targets`",
        ),
        (
            "path = \"src\"\ntargets = [\"aarch64-unknown-linux-musl\"]\n",
            "only a rust source has `targets`",
        ),
    ] {
        let error = read(text).expect_err(says);
        assert!(error.to_string().contains(says), "{says}: {error}");
        assert!(error.key.starts_with("sources.toolchain"), "{error}");
    }
    // A triple of letters, digits, `_` and `.`, in at least two parts.
    read("rust = \"1.99.0\"\ntargets = [\"thumbv7em-none-eabihf\", \"x86_64-unknown-linux-gnu.2.17\"]\n")
        .expect("good triples");
}

#[test]
fn a_rust_source_is_only_a_components_toolchain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).expect("write");
        path
    };
    let rust = "rust = \"1.99.0\"\ntargets = []\n";
    let path = write(
        "component.toml",
        &format!("version = 1\nname = \"c\"\n[sources.rust]\n{rust}"),
    );
    let error = Component::read(&path).expect_err("not named toolchain");
    assert_eq!(error.key, "sources.rust", "{error}");
    assert!(
        error
            .to_string()
            .contains("a component's, named `toolchain`"),
        "{error}"
    );
    let path = write(
        "board.toml",
        &format!(
            "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.toolchain]\n{rust}"
        ),
    );
    let error = gaia_describe::Board::read(&path).expect_err("a board's");
    assert_eq!(error.key, "sources.toolchain", "{error}");
    let path = write(
        "product.toml",
        &format!(
            "version = 1\nname = \"p\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\nrecipe = \"r\"\n[sources.toolchain]\n{rust}"
        ),
    );
    let error = gaia_describe::Product::read(&path).expect_err("a product's");
    assert_eq!(error.key, "sources.toolchain", "{error}");
}
