//! `when.input`: imports, stage items and per-variant selection keyed on the
//! values of declared inputs.

use gaia_config::{
    ConfigError, ResolveOptions, resolve_variant_specs, try_resolve_config_with_options,
};
use gaia_testkit::TempDir;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A build tree written to its own scratch directory. Derefs to the tree's
/// `build.toml`; the directory is removed when this value is dropped.
struct Tree {
    build: PathBuf,
    _scratch: TempDir,
}

impl Deref for Tree {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.build
    }
}

/// Writes a build tree (`files` are relative path and contents) and returns
/// it. `ROOT` in contents is the tree's root.
fn write_tree(files: &[(&str, &str)]) -> Tree {
    let scratch = TempDir::new("gaia-when-inputs");
    let root = scratch.path().to_path_buf();
    for (path, contents) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().expect("parent")).expect("dir");
        let contents = contents.replace("ROOT", &root.display().to_string());
        fs::write(full, contents).expect("write");
    }
    Tree {
        build: root.join("build.toml"),
        _scratch: scratch,
    }
}

const BUILD: &str = r#"
imports = [
  { path = "backend-styx.toml", when = { input = { backend = "styx" } } },
]

build_name = "backend"
version = "1.0.0"

[inputs.backend]
kind = "string"
default = "styx"

[workspace]
root_dir = "ROOT"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "seed"
"#;

const STYX: &str = r#"
[[stage.env_sets]]
id = "styx-env"
name = "styx"
entries = [["BACKEND", "styx"]]
"#;

fn tree(build: &str) -> Tree {
    write_tree(&[
        ("build.toml", build),
        ("backend-styx.toml", STYX),
        ("seed/placeholder", "x"),
    ])
}

fn env_set_ids(path: &Path, options: &ResolveOptions) -> Vec<String> {
    let spec =
        try_resolve_config_with_options(path.to_str().expect("utf-8"), options).expect("resolves");
    spec.stage
        .env_sets
        .iter()
        .map(|set| set.id.as_str().to_string())
        .collect()
}

fn with_input(name: &str, value: &str) -> ResolveOptions {
    ResolveOptions {
        explicit_overrides: vec![(format!("input.{name}"), value.to_string())],
        ..ResolveOptions::default()
    }
}

#[test]
fn an_import_follows_the_input_default_and_the_set_value() {
    let path = tree(BUILD);
    assert_eq!(env_set_ids(&path, &ResolveOptions::default()), ["styx-env"]);
    assert!(env_set_ids(&path, &with_input("backend", "other")).is_empty());
}

#[test]
fn an_import_follows_an_input_set_by_a_preset() {
    let build = format!("{BUILD}\n[presets.other]\noverrides = [[\"input.backend\", \"other\"]]\n");
    let path = tree(&build);
    let preset = ResolveOptions {
        preset: Some("other".into()),
        ..ResolveOptions::default()
    };
    assert!(env_set_ids(&path, &preset).is_empty());
    // A `--set` beats the preset, as for every other override.
    let beaten = ResolveOptions {
        preset: Some("other".into()),
        explicit_overrides: vec![("input.backend".into(), "styx".into())],
        ..ResolveOptions::default()
    };
    assert_eq!(env_set_ids(&path, &beaten), ["styx-env"]);
}

#[test]
fn each_variant_imports_with_its_own_inputs() {
    let build = format!(
        "{BUILD}\n[[variants]]\nid = \"other\"\noverrides = [[\"input.backend\", \"other\"]]\n\n[[variants]]\nid = \"styx-preset\"\npreset = \"styx\"\n\n[presets.styx]\noverrides = [[\"input.backend\", \"styx\"]]\n"
    );
    let path = tree(&build);
    let specs = resolve_variant_specs(path.to_str().expect("utf-8"), &ResolveOptions::default())
        .expect("variants resolve");
    let ids = |index: usize| {
        specs[index]
            .stage
            .env_sets
            .iter()
            .map(|set| set.id.as_str().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(specs[0].selection.variant.as_deref(), Some("default"));
    assert_eq!(ids(0), ["styx-env"]);
    assert_eq!(specs[1].selection.variant.as_deref(), Some("other"));
    assert!(ids(1).is_empty(), "variant 'other' must not import styx");
    assert_eq!(specs[2].selection.variant.as_deref(), Some("styx-preset"));
    assert_eq!(ids(2), ["styx-env"]);
}

#[test]
fn a_stage_item_can_be_keyed_on_an_input() {
    let build = format!(
        "{BUILD}\n[[stage.env_sets]]\nid = \"other-env\"\nname = \"other\"\nentries = [[\"BACKEND\", \"other\"]]\nwhen = {{ input = {{ backend = \"other\" }} }}\n"
    );
    let path = tree(&build);
    assert_eq!(env_set_ids(&path, &ResolveOptions::default()), ["styx-env"]);
    assert_eq!(
        env_set_ids(&path, &with_input("backend", "other")),
        ["other-env"]
    );
}

#[test]
fn an_undeclared_input_name_in_a_when_is_rejected() {
    let build = format!(
        "{BUILD}\n[[stage.env_sets]]\nid = \"typo-env\"\nname = \"typo\"\nentries = []\nwhen = {{ input = {{ backnd = \"styx\" }} }}\n"
    );
    let path = tree(&build);
    let error =
        try_resolve_config_with_options(path.to_str().expect("utf-8"), &ResolveOptions::default())
            .expect_err("rejected");
    assert!(
        matches!(&error, ConfigError::ConfigShape { message, .. } if message.contains("input 'backnd'")),
        "{error}"
    );
}

#[test]
fn a_boolean_input_value_compares_as_its_string_form() {
    let build = BUILD
        .replace(
            "default = \"styx\"",
            "default = \"styx\"\n[inputs.flag]\nkind = \"boolean\"\ndefault = \"true\"",
        )
        .replace(
            "{ path = \"backend-styx.toml\", when = { input = { backend = \"styx\" } } },",
            "{ path = \"backend-styx.toml\", when = { input = { flag = true } } },",
        );
    let path = tree(&build);
    assert_eq!(env_set_ids(&path, &ResolveOptions::default()), ["styx-env"]);
    let off = ResolveOptions {
        explicit_overrides: vec![("input.flag".into(), "false".into())],
        ..ResolveOptions::default()
    };
    assert!(env_set_ids(&path, &off).is_empty());
}
