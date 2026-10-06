//! Build groups: every member's cargo invocation selects the whole group.

use super::*;

/// Members of build group `group`, completed the way config compilation
/// completes them: shared sorted packages and union features.
fn group_member(
    id: &str,
    package: &str,
    target_name: &str,
    output: &Path,
    group_packages: &[&str],
    union_features: &[&str],
) -> ArtifactSpec {
    let mut artifact = with_features(
        rust_artifact(id, package, output),
        union_features,
        false,
        false,
    );
    if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
        rust.target_name = Some(target_name.into());
        rust.build_group = Some("engine".into());
        rust.group_packages = group_packages
            .iter()
            .map(|package| package.to_string())
            .collect();
    }
    artifact
}

fn command_args(artifact: &ArtifactSpec, package: &str, source: &Path) -> Vec<String> {
    let contract = nested_contract(artifact, source);
    cargo_build_command(
        &source.display().to_string(),
        &cargo_packages(artifact, package),
        &CargoFeatureFlags::of(artifact),
        &contract,
    )
    .get_args()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect()
}

#[test]
fn group_member_cargo_command_selects_every_group_package_and_union_features() {
    let source = temp_path("gaia-rust-group-command");
    let packages = ["engine", "plugin"];
    let features = ["engine/fast", "plugin/simd"];
    let engine = group_member(
        "engine",
        "engine",
        "engine",
        &source.join("out/engine"),
        &packages,
        &features,
    );
    let plugin = group_member(
        "plugin",
        "plugin",
        "libplugin.so",
        &source.join("out/libplugin.so"),
        &packages,
        &features,
    );

    let engine_args = command_args(&engine, "engine", &source);
    assert_eq!(
        &engine_args[..7],
        [
            "build",
            "-p",
            "engine",
            "-p",
            "plugin",
            "--features",
            "engine/fast,plugin/simd"
        ]
    );
    assert_eq!(
        engine_args,
        command_args(&plugin, "plugin", &source),
        "every member runs the identical invocation"
    );

    let alone = rust_artifact("alone", "alone", &source.join("out/alone"));
    assert_eq!(
        &command_args(&alone, "alone", &source)[..3],
        ["build", "-p", "alone"]
    );
}

#[test]
fn group_members_batch_only_with_their_own_group() {
    let source = temp_path("gaia-rust-group-batch-key");
    let packages = ["engine", "plugin"];
    let engine = group_member(
        "engine",
        "engine",
        "engine",
        &source.join("out/engine"),
        &packages,
        &[],
    );
    let plugin = group_member(
        "plugin",
        "plugin",
        "libplugin.so",
        &source.join("out/p"),
        &packages,
        &[],
    );
    let alone = rust_artifact("alone", "alone", &source.join("out/alone"));

    let key = |artifact: &ArtifactSpec| {
        RustProvider.batch_key(artifact, &nested_contract(artifact, &source))
    };
    assert!(key(&engine).is_some());
    assert_eq!(key(&engine), key(&plugin));
    assert_ne!(
        key(&engine),
        key(&alone),
        "non-members never join a group's invocation"
    );
    let mut other_group = plugin.clone();
    if let ArtifactDefinition::Rust(rust) = &mut other_group.definition {
        rust.build_group = Some("other".into());
    }
    assert_ne!(key(&engine), key(&other_group));
}

/// An engine binary and a cdylib plugin whose sources only compile with
/// their own feature enabled, so a build succeeds only when the union
/// features reach cargo. The cdylib output is collected via
/// `target_name = "libplugin.so"`.
fn engine_and_plugin_workspace(prefix: &str) -> PathBuf {
    let root = temp_path(prefix);
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("write");
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"engine\", \"plugin\"]\nresolver = \"2\"\n",
    );
    write(
        "engine/Cargo.toml",
        "[package]\nname = \"engine\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nfast = []\n",
    );
    write(
        "engine/src/main.rs",
        "#[cfg(not(feature = \"fast\"))]\ncompile_error!(\"fast missing\");\nfn main() {}\n",
    );
    write(
        "plugin/Cargo.toml",
        "[package]\nname = \"plugin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[features]\nsimd = []\n",
    );
    write(
        "plugin/src/lib.rs",
        "#[cfg(not(feature = \"simd\"))]\ncompile_error!(\"simd missing\");\n#[no_mangle]\npub extern \"C\" fn plugin_abi() -> u32 { 1 }\n",
    );
    root
}

#[test]
fn group_builds_engine_binary_and_cdylib_plugin() {
    let workspace = engine_and_plugin_workspace("gaia-rust-group-cdylib");
    let packages = ["engine", "plugin"];
    let features = ["engine/fast", "plugin/simd"];
    let engine = group_member(
        "engine",
        "engine",
        "engine",
        &workspace.join("out/engine"),
        &packages,
        &features,
    );
    let plugin = group_member(
        "plugin",
        "plugin",
        "libplugin.so",
        &workspace.join("out/libplugin.so"),
        &packages,
        &features,
    );
    let engine_contract = nested_contract(&engine, &workspace);
    let plugin_contract = nested_contract(&plugin, &workspace);

    let results = RustProvider.execute_artifact_batch(
        &[
            ArtifactBatchItem {
                artifact: &engine,
                contract: &engine_contract,
                log_sink: None,
            },
            ArtifactBatchItem {
                artifact: &plugin,
                contract: &plugin_contract,
                log_sink: None,
            },
        ],
        None,
    );
    for result in &results {
        assert!(result.is_ok(), "{result:?}");
    }
    assert!(workspace.join("out/engine").is_file());
    assert!(workspace.join("out/libplugin.so").is_file());

    // A member built on its own runs the same group invocation.
    fs::remove_file(workspace.join("out/libplugin.so")).expect("remove plugin output");
    RustProvider
        .execute_artifact(&plugin, &plugin_contract, None, None)
        .expect("plugin builds alone with the group's invocation");
    assert!(workspace.join("out/libplugin.so").is_file());
    let _ = fs::remove_dir_all(workspace);
}
