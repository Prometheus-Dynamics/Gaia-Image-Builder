//! Every recipe in the repository's `recipes/` plans for the generic test
//! board, so a broken recipe fails the gate, not a board build.
//!
//! Each recipe gets its own product: one image, the test board, the test
//! component, and every parameter it has no default for given a value of
//! its type. Nothing is fetched or built.

use gaia_recipe::{ParamType, Recipes};
use std::path::{Path, PathBuf};
use std::process::Command;

const URL: &str = "https://github.com/vasilakisfil/hello.service";
const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";
const IMAGE: &str = "docker.io/library/debian:trixie-slim";
const IMAGE_DIGEST: &str =
    "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";

/// The tools the recipes' stages name, pinned to paths that need not exist:
/// planning never runs them.
const TOOLS: &[(&str, &str)] = &[
    ("toolchain", "/opt/toolchain"),
    ("mkfs_erofs", "/opt/tools/mkfs.erofs"),
    ("veritysetup", "/opt/tools/veritysetup"),
    ("mke2fs", "/opt/tools/mke2fs"),
    ("debugfs", "/opt/tools/debugfs"),
    ("git", "/usr/bin/git"),
    ("curl", "/usr/bin/curl"),
    ("bwrap", "/usr/bin/bwrap"),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root")
}

/// A TOML value of `kind` for a parameter with no default.
fn value(kind: ParamType) -> &'static str {
    match kind {
        ParamType::Bool => "false",
        ParamType::Int => "1",
        ParamType::Float => "1.0",
        ParamType::String => "\"test\"",
        ParamType::Strings => "[]",
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
    std::fs::copy(from, to).expect("copy");
}

/// The product for `recipe`, under `dir`, with its one image.
fn product(dir: &Path, examples: &Path, name: &str, recipe: &gaia_recipe::Recipe) {
    copy(
        &examples.join("boards/generic-test/board.toml"),
        &dir.join("boards/generic-test/board.toml"),
    );
    copy(
        &examples.join("components/test-tree/component.toml"),
        &dir.join("components/test-tree/component.toml"),
    );
    std::fs::write(
        dir.join("gaia.lock"),
        format!(
            "version = 1\n\n[source.\"test-tree.src\"]\nkind = \"git\"\nurl = \"{URL}\"\nref = \"master\"\ncommit = \"{COMMIT}\"\n[source.\"test-tree.root\"]\nkind = \"image\"\nreference = \"{IMAGE}\"\ndigest = \"{IMAGE_DIGEST}\"\n"
        ),
    )
    .expect("lock");
    // A recipe that takes `data` gets the test component as its data too.
    let data = if recipe.inputs.contains_key("data") {
        "data = [\"test-tree\"]\n"
    } else {
        ""
    };
    let mut settings = String::new();
    for (key, param) in &recipe.parameters {
        if param.default.is_none() {
            settings.push_str(&format!("{key} = {}\n", value(param.kind)));
        }
    }
    std::fs::write(
        dir.join("product.toml"),
        format!(
            "version = 1\nname = \"{name}\"\n[components]\ntest-tree = \"components/test-tree/component.toml\"\n\n[[images]]\nname = \"img\"\nboard = \"boards/generic-test/board.toml\"\ncomponents = [\"test-tree\"]\n{data}recipe = \"{name}\"\n\n[images.settings]\n{settings}"
        ),
    )
    .expect("product");
}

#[test]
fn every_recipe_plans_for_the_generic_test_board() {
    let repo = repo();
    let recipes_dir = repo.join("recipes");
    let examples = repo.join("examples");
    let recipes = Recipes::load_dir(&recipes_dir).expect("the recipes load");
    assert!(
        !recipes.by_name.is_empty(),
        "no recipes in {}",
        recipes_dir.display()
    );
    for (name, recipe) in &recipes.by_name {
        let dir = tempfile::tempdir().expect("tempdir");
        product(dir.path(), &examples, name, recipe);
        let mut args = vec![
            "plan".to_string(),
            "product.toml".to_string(),
            "--recipes".to_string(),
            recipes_dir.display().to_string(),
        ];
        for (tool, path) in TOOLS {
            args.push("--tool".into());
            args.push(format!("{tool}={path}"));
        }
        let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
            .args(&args)
            .current_dir(dir.path())
            .env("GAIA_STORE", dir.path().join("store"))
            .output()
            .expect("gaia runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "recipe {name} does not plan:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for stage in recipe.stages.keys() {
            assert!(
                text.contains(&format!("img::{stage}")),
                "recipe {name}: no stage {stage} in the plan:\n{text}"
            );
        }
        assert!(text.contains("\nplan: "), "recipe {name}: no plan:\n{text}");
        assert!(!dir.path().join("store").exists(), "plan touched the store");
    }
}

#[test]
fn a_data_recipe_without_a_data_component_names_images_data() {
    let repo = repo();
    let recipes_dir = repo.join("recipes");
    let recipes = Recipes::load_dir(&recipes_dir).expect("the recipes load");
    let recipe = &recipes.by_name["data-ext4"];
    let dir = tempfile::tempdir().expect("tempdir");
    product(dir.path(), &repo.join("examples"), "data-ext4", recipe);
    let text = std::fs::read_to_string(dir.path().join("product.toml")).expect("product");
    std::fs::write(
        dir.path().join("product.toml"),
        text.replace("data = [\"test-tree\"]\n", ""),
    )
    .expect("product");
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(["plan", "product.toml", "--recipes"])
        .arg(&recipes_dir)
        .current_dir(dir.path())
        .env("GAIA_STORE", dir.path().join("store"))
        .output()
        .expect("gaia runs");
    let error = String::from_utf8_lossy(&out.stderr);
    println!("{error}");
    assert!(!out.status.success());
    assert!(
        error.contains("images[0].data")
            && error.contains("takes one built component as `data`; this image has 0"),
        "{error}"
    );
}
