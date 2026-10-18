pub mod support;

use gaia_config::{ConfigError, ResolveOptions, resolve_variant_specs};
use gaia_spec::{ArtifactDefinition, ResolvedBuildSpec};
use support::write_temp_config;

/// A build with one input, one artifact that takes its package from it, and
/// the starting-point image. `variants` is appended as the TOML tail.
fn build_with_variants(variants: &str) -> String {
    let root = std::env::temp_dir().join("gaia-config-variants-root");
    format!(
        r#"
build_name = "rel"
version = "1.0.0"

[inputs.rel]
kind = "string"
default = "2027"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app-source"
kind = "path"
path = "app"

[[artifacts]]
id = "jar"
kind = "rust"
package = "app-${{input.rel}}"
source = "app-source"
profile = "release"
target = "x86_64-unknown-linux-musl"
install_name = "app"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/app.bin"

[image]
kind = "starting-point"
rootfs_path = "seed/base.img"

{variants}
"#,
        root.display()
    )
}

/// The cargo package the build's artifact builds.
fn package(spec: &ResolvedBuildSpec) -> String {
    match &spec.artifacts[0].definition {
        ArtifactDefinition::Rust(rust) => rust.package.clone(),
        other => panic!("expected a rust artifact, got {other:?}"),
    }
}

fn resolve(
    contents: &str,
    options: &ResolveOptions,
) -> Result<Vec<gaia_spec::ResolvedBuildSpec>, ConfigError> {
    let path = write_temp_config(contents);
    resolve_variant_specs(path.to_str().expect("utf-8 temp path"), options)
}

#[test]
fn a_build_without_variants_resolves_to_one_spec_with_no_variant() {
    let specs = resolve(&build_with_variants(""), &ResolveOptions::default()).expect("resolves");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].selection.variant, None);
    assert!(specs[0].selection.variants.is_empty());
}

#[test]
fn each_declared_variant_gets_its_overrides_and_the_base_stays_default() {
    let contents = build_with_variants(
        r#"
[[variants]]
id = "rel2026"
description = "MyApp 2026"
overrides = [["input.rel", "2026"]]

[[variants]]
id = "rel2025"
overrides = [["input.rel", "2025"]]
"#,
    );
    let specs = resolve(&contents, &ResolveOptions::default()).expect("resolves");
    let ids = specs
        .iter()
        .map(|spec| spec.selection.variant.clone().expect("variant set"))
        .collect::<Vec<_>>();
    assert_eq!(ids, ["default", "rel2026", "rel2025"]);
    assert_eq!(package(&specs[0]), "app-2027");
    assert_eq!(package(&specs[1]), "app-2026");
    assert_eq!(package(&specs[2]), "app-2025");
    assert_eq!(specs[1].selection.variants.len(), 2);
}

#[test]
fn cli_overrides_win_over_variant_overrides() {
    let contents = build_with_variants(
        r#"
[[variants]]
id = "rel2026"
overrides = [["input.rel", "2026"]]
"#,
    );
    let options = ResolveOptions {
        explicit_overrides: vec![("input.rel".into(), "2030".into())],
        ..ResolveOptions::default()
    };
    let specs = resolve(&contents, &options).expect("resolves");
    assert_eq!(package(&specs[0]), "app-2030");
    assert_eq!(package(&specs[1]), "app-2030");
}

#[test]
fn invalid_variant_declarations_are_rejected() {
    let cases = [
        (
            r#"[[variants]]
id = "REL2026"
"#,
            "lowercase letters",
        ),
        (
            r#"[[variants]]
id = "default"
"#,
            "reserved",
        ),
        (
            r#"[[variants]]
id = "a"
[[variants]]
id = "a"
"#,
            "more than once",
        ),
        (
            r#"[[variants]]
id = "a"
overrides = [["input.nope", "1"]]
"#,
            "does not declare",
        ),
        (
            r#"[[variants]]
id = "a"
overrides = [["bogus.key", "1"]]
"#,
            "unknown override key",
        ),
    ];
    for (declaration, expected) in cases {
        let error = match resolve(
            &build_with_variants(declaration),
            &ResolveOptions::default(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("declaration should be rejected: {declaration}"),
        };
        assert!(
            error.to_string().contains(expected),
            "expected '{expected}' in: {error}"
        );
    }
}

#[test]
fn a_variant_preset_applies_before_the_variant_overrides() {
    let contents = build_with_variants(
        r#"
[presets.rel2026]
overrides = [["input.rel", "2026"]]

[presets.rel2029]
overrides = [["input.rel", "2029"]]

[[variants]]
id = "by-preset"
preset = "rel2026"

[[variants]]
id = "preset-then-override"
preset = "rel2026"
overrides = [["input.rel", "2028"]]

[[variants]]
id = "preset-list"
presets = ["rel2026", "rel2029"]
"#,
    );
    let specs = resolve(&contents, &ResolveOptions::default()).expect("resolves");
    let package_of = |id: &str| {
        let spec = specs
            .iter()
            .find(|spec| spec.selection.variant.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no spec for {id}"));
        package(spec)
    };
    assert_eq!(package_of("default"), "app-2027", "the base config stays");
    assert_eq!(package_of("by-preset"), "app-2026");
    assert_eq!(package_of("preset-then-override"), "app-2028");
    assert_eq!(package_of("preset-list"), "app-2029", "later presets win");
    let listed = specs[0]
        .selection
        .variants
        .iter()
        .find(|variant| variant.id == "preset-list")
        .expect("declared variant");
    assert_eq!(listed.presets, ["rel2026", "rel2029"]);
}

#[test]
fn a_variant_naming_an_undefined_preset_is_rejected() {
    let contents = build_with_variants(
        r#"
[[variants]]
id = "rel2026"
preset = "missing"
"#,
    );
    let error = resolve(&contents, &ResolveOptions::default()).expect_err("rejected");
    assert!(
        error
            .to_string()
            .contains("names preset 'missing', which the build does not define"),
        "{error}"
    );
}

#[test]
fn a_variant_scoped_source_token_names_the_variants_own_checkout() {
    // A git source: its checkout is `build/sources/<id>` (a path source's
    // directory is its own, so there is nothing to scope).
    let contents = build_with_variants(
        r#"
[[variants]]
id = "rel2026"
overrides = [["input.rel", "2026"]]

[[stage.env_sets]]
id = "paths"
name = "paths"
entries = [["SRC", "${source.app-source.path}"]]
"#,
    )
    .replace(
        "kind = \"path\"\npath = \"app\"",
        "kind = \"git\"\nrepo = \"file:///nonexistent-repo\"\nrev = \"0123456789abcdef0123456789abcdef01234567\"",
    );
    let scoped = std::collections::BTreeSet::from(["app-source".to_string()]);
    let config = write_temp_config(&contents);
    let specs = gaia_config::resolve_variant_specs_scoped(
        config.to_str().expect("utf-8 temp path"),
        &ResolveOptions::default(),
        &scoped,
    )
    .expect("resolves");
    let src = |spec: &ResolvedBuildSpec| spec.stage.env_sets[0].entries[0].1.clone();
    assert!(
        src(&specs[0]).ends_with("build/sources/variants/default/app-source"),
        "{}",
        src(&specs[0])
    );
    assert!(
        src(&specs[1]).ends_with("build/sources/variants/rel2026/app-source"),
        "{}",
        src(&specs[1])
    );

    // Without the scope the token names the shared checkout, as before.
    let shared = resolve(&contents, &ResolveOptions::default()).expect("resolves");
    assert!(
        src(&shared[1]).ends_with("build/sources/app-source"),
        "{}",
        src(&shared[1])
    );
}

#[test]
fn a_java_artifacts_build_env_names_the_variants_checkout() {
    let contents = build_with_variants(
        r#"
[[variants]]
id = "rel2026"
overrides = [["input.rel", "2026"]]

[[artifacts]]
id = "sensor-jar"
kind = "java"
source = "app-source"
build_target = "build/libs/app.jar"
build_command = ["./build.sh", "--corelib", "${source.app-source.path}"]
build_env = [["CORELIB_PATH", "${source.app-source.path}"], ["CORELIB_REV", "${source.app-source.commit}"]]
output_path = "out/app.jar"
"#,
    )
    .replace(
        "kind = \"path\"\npath = \"app\"",
        "kind = \"git\"\nrepo = \"file:///nonexistent-repo\"\nrev = \"0123456789abcdef0123456789abcdef01234567\"",
    );
    let java = |spec: &ResolvedBuildSpec| {
        let artifact = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id.as_str() == "sensor-jar")
            .expect("sensor-jar");
        match &artifact.definition {
            gaia_spec::ArtifactDefinition::Java(java) => java.clone(),
            other => panic!("not java: {other:?}"),
        }
    };

    let scoped = std::collections::BTreeSet::from(["app-source".to_string()]);
    let config = write_temp_config(&contents);
    let specs = gaia_config::resolve_variant_specs_scoped(
        config.to_str().expect("utf-8 temp path"),
        &ResolveOptions::default(),
        &scoped,
    )
    .expect("resolves");
    let rel2026 = java(&specs[1]);
    let corelib = &rel2026.build_env[0].1;
    assert!(
        corelib.ends_with("build/sources/variants/rel2026/app-source"),
        "{corelib}"
    );
    assert_eq!(rel2026.build_command[2], *corelib);
    assert_eq!(
        rel2026.build_env[1].1,
        "0123456789abcdef0123456789abcdef01234567"
    );

    // Shared (no scope): the shared checkout.
    let shared = resolve(&contents, &ResolveOptions::default()).expect("resolves");
    assert!(
        java(&shared[0]).build_env[0]
            .1
            .ends_with("build/sources/app-source"),
        "{:?}",
        java(&shared[0]).build_env
    );
}
