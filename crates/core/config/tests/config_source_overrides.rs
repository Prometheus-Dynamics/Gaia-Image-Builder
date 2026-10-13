//! `--set sources.<id>.path=...`: the id must be declared by the config the
//! resolution selects, which may be an import gated on an input that a
//! preset, a variant or `--set` selects.

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
    let scratch = TempDir::new("gaia-source-overrides");
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

/// A build whose `eidos` source is declared only by an import gated on
/// `input.backend = styx`. The input defaults to `libcamera`.
const BUILD: &str = r#"
imports = [
  { path = "backend-styx.toml", when = { input = { backend = "styx" } } },
]

build_name = "backend"
version = "1.0.0"

[inputs.backend]
kind = "string"
default = "libcamera"

[workspace]
root_dir = "ROOT"
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "seed"

[presets.styx]
overrides = [["input.backend", "styx"]]
"#;

const STYX: &str = r#"
[[sources]]
id = "eidos"
kind = "path"
path = "eidos-original"
"#;

fn tree(build: &str) -> Tree {
    write_tree(&[
        ("build.toml", build),
        ("backend-styx.toml", STYX),
        ("seed/placeholder", "x"),
    ])
}

fn with(overrides: &[(&str, &str)]) -> ResolveOptions {
    ResolveOptions {
        explicit_overrides: overrides
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        ..ResolveOptions::default()
    }
}

fn resolve(path: &Path, options: &ResolveOptions) -> Result<(), ConfigError> {
    try_resolve_config_with_options(path.to_str().expect("utf-8"), options).map(|_| ())
}

fn eidos_path(path: &Path, options: &ResolveOptions) -> String {
    let spec =
        try_resolve_config_with_options(path.to_str().expect("utf-8"), options).expect("resolves");
    let source = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "eidos")
        .expect("eidos is declared");
    format!("{:?}", source.definition)
}

#[test]
fn an_override_names_a_source_declared_by_an_input_gated_import() {
    let path = tree(BUILD);
    let override_dir = "/abs/eidos-override";
    let options = with(&[
        ("input.backend", "styx"),
        ("sources.eidos.path", override_dir),
    ]);
    assert!(eidos_path(&path, &options).contains(override_dir));
}

#[test]
fn an_override_names_a_source_declared_once_a_preset_selects_the_import() {
    let path = tree(BUILD);
    let options = ResolveOptions {
        preset: Some("styx".into()),
        explicit_overrides: vec![("sources.eidos.path".into(), "/abs/eidos-override".into())],
        ..ResolveOptions::default()
    };
    assert!(eidos_path(&path, &options).contains("/abs/eidos-override"));
}

#[test]
fn an_override_of_an_undeclared_source_is_an_error() {
    let path = tree(BUILD);
    // libcamera selects no import that declares eidos.
    let error = resolve(&path, &with(&[("sources.eidos.path", "/abs/x")]))
        .expect_err("eidos is not declared by this selection");
    let message = error.to_string();
    assert!(message.contains("'eidos'"), "{message}");
    assert!(message.contains("declared"), "{message}");
    assert!(
        matches!(&error, ConfigError::UndeclaredSourceOverride { source_id, .. } if source_id == "eidos"),
        "{error:?}"
    );

    let unknown = resolve(
        &path,
        &with(&[("input.backend", "styx"), ("sources.nope.path", "/x")]),
    )
    .expect_err("nope is declared nowhere");
    assert!(unknown.to_string().contains("'nope'"), "{unknown}");
}

#[test]
fn a_variant_override_applies_to_the_variant_declaring_the_source() {
    let build = format!(
        "{BUILD}\n[[variants]]\nid = \"styx\"\npreset = \"styx\"\n\n[[variants]]\nid = \"plain\"\n"
    );
    let path = tree(&build);
    let options = with(&[("sources.eidos.path", "/abs/eidos-override")]);
    let specs = resolve_variant_specs(path.to_str().expect("utf-8"), &options)
        .expect("the styx variant declares eidos, so the override is accepted");
    let definition = |index: usize| {
        specs[index]
            .sources
            .iter()
            .find(|source| source.id.as_str() == "eidos")
            .map(|source| format!("{:?}", source.definition))
    };
    assert!(
        definition(1)
            .expect("styx declares eidos")
            .contains("/abs/eidos-override")
    );
    assert!(
        definition(2).is_none(),
        "variant 'plain' does not declare eidos"
    );
}

#[test]
fn a_variant_override_is_an_error_when_no_selected_variant_declares_the_source() {
    let build = format!("{BUILD}\n[[variants]]\nid = \"plain\"\n");
    let path = tree(&build);
    let error = resolve_variant_specs(
        path.to_str().expect("utf-8"),
        &with(&[("sources.eidos.path", "/abs/eidos-override")]),
    )
    .expect_err("no variant declares eidos");
    assert!(
        matches!(&error, ConfigError::UndeclaredSourceOverride { source_id, .. } if source_id == "eidos"),
        "{error:?}"
    );
}

#[test]
fn a_cli_override_wins_over_the_preset_that_selects_the_import() {
    let path = tree(BUILD);
    let options = ResolveOptions {
        preset: Some("styx".into()),
        explicit_overrides: vec![("sources.eidos.path".into(), "/abs/cli".into())],
        ..ResolveOptions::default()
    };
    assert!(eidos_path(&path, &options).contains("/abs/cli"));
}

#[test]
fn a_preset_that_declares_the_source_is_not_undone_by_a_variant_that_does_not() {
    // `--preset styx` selects the declaring base; the variant `plain` keeps
    // libcamera and does not declare eidos. It must not reject the override.
    let build = format!(
        "{BUILD}\n[[variants]]\nid = \"plain\"\npreset = \"libcamera\"\n\n[presets.libcamera]\noverrides = [[\"input.backend\", \"libcamera\"]]\n"
    );
    let path = tree(&build);
    let options = ResolveOptions {
        preset: Some("styx".into()),
        explicit_overrides: vec![("sources.eidos.path".into(), "/abs/eidos-override".into())],
        ..ResolveOptions::default()
    };
    let specs = resolve_variant_specs(path.to_str().expect("utf-8"), &options)
        .expect("the base declares eidos, so the override is accepted");
    assert_eq!(specs.len(), 2);
    assert!(
        specs[0]
            .sources
            .iter()
            .any(|source| source.id.as_str() == "eidos"),
        "the base spec declares eidos"
    );
    assert!(specs[1].sources.is_empty(), "plain declares no source");
}

#[test]
fn a_variant_selection_must_include_a_variant_declaring_the_source() {
    let build = format!(
        "{BUILD}\n[[variants]]\nid = \"styx\"\npreset = \"styx\"\n\n[[variants]]\nid = \"plain\"\n"
    );
    let path = tree(&build);
    let options = ResolveOptions {
        selected_variants: vec!["plain".into()],
        explicit_overrides: vec![("sources.eidos.path".into(), "/abs/eidos-override".into())],
        ..ResolveOptions::default()
    };
    let error = resolve_variant_specs(path.to_str().expect("utf-8"), &options)
        .expect_err("the only selected variant does not declare eidos");
    assert!(
        matches!(&error, ConfigError::UndeclaredSourceOverride { scope, declared, .. }
            if scope == "any selected variant" && declared.is_empty()),
        "{error}"
    );
    let selected = ResolveOptions {
        selected_variants: vec!["styx".into()],
        ..options
    };
    assert!(resolve_variant_specs(path.to_str().expect("utf-8"), &selected).is_ok());
}
